//! Browser management for chaser-cf

use crate::error::{ChaserError, ChaserResult};
use crate::models::ProxyConfig;

use chaser_oxide::cdp::browser_protocol::target::CreateTargetParams;
use chaser_oxide::{Browser, BrowserConfig, ChaserPage};
use futures::StreamExt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Semaphore;

pub struct BrowserManager {
    browser: Browser,
    context_semaphore: Arc<Semaphore>,
    active_contexts: Arc<AtomicUsize>,
    max_contexts: usize,
    healthy: Arc<AtomicBool>,
}

impl BrowserManager {
    pub async fn new(config: &super::ChaserConfig) -> ChaserResult<Self> {
        // Baseline flags chaser-cf always sets, plus any extras the caller
        // configured via ChaserConfig::with_extra_args / add_extra_arg /
        // CHASER_EXTRA_ARGS env var. Common extras: --no-sandbox (when the
        // host process runs as root), --disable-gpu, --disable-dev-shm-usage.
        let mut chrome_args: Vec<String> = vec![
            "--disable-blink-features=AutomationControlled".to_string(),
            "--disable-infobars".to_string(),
        ];
        chrome_args.extend(config.extra_args.iter().cloned());

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

        // apply_native_profile reads the real Chrome version from the live browser UA
        // and pairs it with the host OS + RAM, then issues Emulation.setUserAgentOverride
        // with full userAgentMetadata so Sec-CH-UA-Platform/version are self-consistent.
        chaser
            .apply_native_profile()
            .await
            .map_err(|e| ChaserError::PageFailed(format!("apply_native_profile: {e}")))?;

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
