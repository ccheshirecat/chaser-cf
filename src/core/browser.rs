//! Browser management for chaser-cf

use crate::error::{ChaserError, ChaserResult};
#[cfg(target_os = "linux")]
use crate::models::Profile;
use crate::models::ProxyConfig;

use chaser_oxide::cdp::browser_protocol::target::CreateTargetParams;
#[cfg(target_os = "linux")]
use chaser_oxide::ChaserProfile;
use chaser_oxide::{Browser, BrowserConfig, ChaserPage};
use futures::StreamExt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Normalize a Chrome-style flag for chaser-oxide's `ArgsBuilder`.
///
/// chaser-oxide stores arg keys WITHOUT the leading `--` and prepends
/// `--` itself at command-build time. Passing pre-formatted strings
/// (`"--no-sandbox"`) produces `"----no-sandbox"`, which Chrome silently
/// ignores. This helper strips any leading dash chars so the rendered
/// command-line argument comes out correctly:
///
///   "--no-sandbox"           -> "no-sandbox"           -> "--no-sandbox"
///   "--key=value"            -> "key=value"            -> "--key=value"
///   "no-sandbox"             -> "no-sandbox"           -> "--no-sandbox"
///
/// Both `--key=value` and `--key value` chrome-flag forms are supported
/// (the renderer just emits the stored key verbatim with a `--` prefix).
pub(crate) fn normalize_chrome_flag(raw: &str) -> String {
    raw.trim_start_matches('-').to_string()
}

pub struct BrowserManager {
    browser: Browser,
    context_semaphore: Arc<Semaphore>,
    active_contexts: Arc<AtomicUsize>,
    max_contexts: usize,
    healthy: Arc<AtomicBool>,
    #[cfg(target_os = "linux")]
    profile: Profile,
}

impl BrowserManager {
    pub async fn new(config: &super::ChaserConfig) -> ChaserResult<Self> {
        // Baseline flags chaser-cf always sets, plus any extras the caller
        // configured via ChaserConfig::with_extra_args / add_extra_arg /
        // CHASER_EXTRA_ARGS env var. Common extras: --no-sandbox (when the
        // host process runs as root), --disable-gpu, --disable-dev-shm-usage.
        //
        // Every flag goes through normalize_chrome_flag, which strips the
        // leading `--` so chaser-oxide's ArgsBuilder doesn't double-render
        // it as `----flag`. The original chaser-cf 0.1.0..0.1.4 baseline
        // flags hit this exact bug and were silently ignored by Chrome
        // for the entire lifetime of those releases.
        let mut chrome_args: Vec<String> = vec![
            normalize_chrome_flag("--disable-blink-features=AutomationControlled"),
            normalize_chrome_flag("--disable-infobars"),
        ];
        chrome_args.extend(config.extra_args.iter().map(|a| normalize_chrome_flag(a)));

        let mut builder = BrowserConfig::builder().viewport(None).args(chrome_args);

        if let Some(ref path) = config.chrome_path {
            builder = builder.chrome_executable(path.clone());
        }

        if !config.headless {
            builder = builder.with_head();
        } else {
            builder = builder.new_headless_mode();
        }

        let browser_config = builder
            .build()
            .map_err(|e| ChaserError::InitFailed(e.to_string()))?;

        let (browser, mut handler) = Browser::launch(browser_config)
            .await
            .map_err(|e| ChaserError::InitFailed(e.to_string()))?;

        let healthy = Arc::new(AtomicBool::new(true));
        let healthy_clone = healthy.clone();
        tokio::spawn(async move {
            loop {
                match handler.next().await {
                    Some(_) => {}
                    None => {
                        healthy_clone.store(false, Ordering::SeqCst);
                        break;
                    }
                }
            }
        });

        Ok(Self {
            browser,
            context_semaphore: Arc::new(Semaphore::new(config.context_limit)),
            active_contexts: Arc::new(AtomicUsize::new(0)),
            max_contexts: config.context_limit,
            healthy,
            #[cfg(target_os = "linux")]
            profile: config.profile,
        })
    }

    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::SeqCst)
    }

    pub fn active_contexts(&self) -> usize {
        self.active_contexts.load(Ordering::SeqCst)
    }

    pub fn max_contexts(&self) -> usize {
        self.max_contexts
    }

    pub async fn acquire_permit(&self) -> ChaserResult<ContextPermit> {
        let permit = self
            .context_semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ChaserError::ContextFailed("Semaphore closed".to_string()))?;

        self.active_contexts.fetch_add(1, Ordering::SeqCst);

        Ok(ContextPermit {
            _permit: permit,
            active_contexts: self.active_contexts.clone(),
        })
    }

    pub fn try_acquire_permit(&self) -> Option<ContextPermit> {
        let permit = self.context_semaphore.clone().try_acquire_owned().ok()?;
        self.active_contexts.fetch_add(1, Ordering::SeqCst);
        Some(ContextPermit {
            _permit: permit,
            active_contexts: self.active_contexts.clone(),
        })
    }

    pub async fn create_context(
        &self,
        proxy: Option<&ProxyConfig>,
    ) -> ChaserResult<Option<chaser_oxide::cdp::browser_protocol::browser::BrowserContextId>> {
        match proxy {
            Some(p) => {
                let ctx_id = self
                    .browser
                    .create_incognito_context_with_proxy(p.to_url())
                    .await
                    .map_err(|e| ChaserError::ContextFailed(e.to_string()))?;
                Ok(Some(ctx_id))
            }
            None => Ok(None),
        }
    }

    /// Open a blank page, apply the native profile (OS + real Chrome version), then
    /// navigate to `url`. Proxy auth is handled by the caller before navigation.
    pub async fn new_page(
        &self,
        ctx_id: Option<chaser_oxide::cdp::browser_protocol::browser::BrowserContextId>,
        url: &str,
    ) -> ChaserResult<(chaser_oxide::Page, ChaserPage)> {
        let mut params = CreateTargetParams::new("about:blank");
        if let Some(id) = ctx_id {
            params.browser_context_id = Some(id);
        }

        let page = self
            .browser
            .new_page(params)
            .await
            .map_err(|e| ChaserError::PageFailed(e.to_string()))?;

        let chaser = ChaserPage::new(page.clone());

        // On macOS/Windows use native profile — Chrome version, RAM, and GPU all
        // match the real host, which is always the most convincing fingerprint.
        // On Linux only, override with the configured profile (default: Windows)
        // because native Linux leaks Os::Linux into UA + Sec-CH-UA-Platform-Version.
        #[cfg(not(target_os = "linux"))]
        chaser
            .apply_native_profile()
            .await
            .map_err(|e| ChaserError::PageFailed(format!("apply_native_profile: {e}")))?;

        #[cfg(target_os = "linux")]
        {
            let chrome_ver = chaser_oxide::detect_chrome_version().unwrap_or(131);
            let memory_gb = chaser_oxide::detect_system_memory_gb();
            let fingerprint = match self.profile {
                Profile::Windows => ChaserProfile::windows()
                    .chrome_version(chrome_ver)
                    .memory_gb(memory_gb)
                    .build(),
                Profile::Macos => ChaserProfile::macos_arm()
                    .chrome_version(chrome_ver)
                    .memory_gb(memory_gb)
                    .build(),
                Profile::Linux => ChaserProfile::linux()
                    .chrome_version(chrome_ver)
                    .memory_gb(memory_gb)
                    .build(),
            };
            chaser
                .apply_profile(&fingerprint)
                .await
                .map_err(|e| ChaserError::PageFailed(format!("apply_profile: {e}")))?;
        }

        if url != "about:blank" {
            chaser
                .goto(url)
                .await
                .map_err(|e| ChaserError::NavigationFailed(e.to_string()))?;
        }

        Ok((page, chaser))
    }

    pub async fn shutdown(self) {
        self.healthy.store(false, Ordering::SeqCst);
    }
}

pub struct ContextPermit {
    _permit: tokio::sync::OwnedSemaphorePermit,
    active_contexts: Arc<AtomicUsize>,
}

impl Drop for ContextPermit {
    fn drop(&mut self) {
        self.active_contexts.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_chrome_flag;

    #[test]
    fn normalize_strips_double_dash_keys() {
        assert_eq!(normalize_chrome_flag("--no-sandbox"), "no-sandbox");
        assert_eq!(normalize_chrome_flag("--disable-gpu"), "disable-gpu");
    }

    #[test]
    fn normalize_strips_double_dash_keyvalue() {
        assert_eq!(
            normalize_chrome_flag("--disable-blink-features=AutomationControlled"),
            "disable-blink-features=AutomationControlled"
        );
    }

    #[test]
    fn normalize_passes_through_already_clean() {
        assert_eq!(normalize_chrome_flag("no-sandbox"), "no-sandbox");
        assert_eq!(normalize_chrome_flag("key=value"), "key=value");
    }

    #[test]
    fn normalize_handles_single_dash_too() {
        // Some legacy chrome flags use a single dash; trim_start_matches('-')
        // strips any number, so both forms normalize identically.
        assert_eq!(normalize_chrome_flag("-no-sandbox"), "no-sandbox");
        assert_eq!(normalize_chrome_flag("---no-sandbox"), "no-sandbox");
    }
}
