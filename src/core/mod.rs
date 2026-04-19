//! Core chaser-cf implementation
//!
//! This module provides the main `ChaserCF` API for browser automation
//! with stealth capabilities.

mod browser;
mod config;
mod solver;

pub use browser::BrowserManager;
pub use config::ChaserConfig;

use crate::error::{ChaserError, ChaserResult};
use crate::models::{ProxyConfig, WafSession};

use std::sync::Arc;
use tokio::sync::RwLock;

/// chaser-cf - High-level API for Cloudflare bypass operations
///
/// # Example
///
/// ```rust,no_run
/// use chaser_cf::{ChaserCF, ChaserConfig};
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let chaser = ChaserCF::new(ChaserConfig::default()).await?;
///
///     let session = chaser.solve_waf_session("https://example.com", None).await?;
///     println!("Got {} cookies", session.cookies.len());
///
///     chaser.shutdown().await;
///     Ok(())
/// }
/// ```
pub struct ChaserCF {
    config: ChaserConfig,
    browser: Arc<RwLock<Option<BrowserManager>>>,
    initialized: Arc<RwLock<bool>>,
}

impl ChaserCF {
    /// Create a new ChaserCF instance with the given configuration.
    ///
    /// This will initialize the browser immediately unless `lazy_init` is enabled
    /// in the configuration.
    pub async fn new(config: ChaserConfig) -> ChaserResult<Self> {
        let suite = Self {
            config: config.clone(),
            browser: Arc::new(RwLock::new(None)),
            initialized: Arc::new(RwLock::new(false)),
        };

        if !config.lazy_init {
            suite.init().await?;
        }

        Ok(suite)
    }

    /// Initialize the browser explicitly.
    ///
    /// This is called automatically on first use if `lazy_init` is enabled,
    /// or during construction if `lazy_init` is disabled.
    pub async fn init(&self) -> ChaserResult<()> {
        let mut initialized = self.initialized.write().await;
        if *initialized {
            return Ok(());
        }

        tracing::info!("Initializing chaser-cf browser...");

        let manager = BrowserManager::new(&self.config).await?;

        let mut browser = self.browser.write().await;
        *browser = Some(manager);
        *initialized = true;

        tracing::info!("chaser-cf browser initialized");
        Ok(())
    }

    /// Ensure browser is initialized (for lazy init)
    async fn ensure_init(&self) -> ChaserResult<()> {
        if !*self.initialized.read().await {
            self.init().await?;
        }
        Ok(())
    }

    /// Get browser manager, initializing if needed
    async fn browser(
        &self,
    ) -> ChaserResult<tokio::sync::RwLockReadGuard<'_, Option<BrowserManager>>> {
        self.ensure_init().await?;
        let guard = self.browser.read().await;
        if guard.is_none() {
            return Err(ChaserError::NotInitialized);
        }
        Ok(guard)
    }

    /// Shutdown the browser and release resources.
    pub async fn shutdown(&self) {
        let mut browser = self.browser.write().await;
        if let Some(manager) = browser.take() {
            manager.shutdown().await;
        }
        *self.initialized.write().await = false;
        tracing::info!("chaser-cf shutdown complete");
    }

    /// Check if the browser is initialized and healthy
    pub async fn is_ready(&self) -> bool {
        let initialized = *self.initialized.read().await;
        if !initialized {
            return false;
        }

        let browser = self.browser.read().await;
        browser.as_ref().map(|b| b.is_healthy()).unwrap_or(false)
    }

    /// Get page source from a Cloudflare-protected URL
    ///
    /// # Arguments
    ///
    /// * `url` - Target URL to scrape
    /// * `proxy` - Optional proxy configuration
    ///
    /// # Returns
    ///
    /// The HTML source of the page after bypassing Cloudflare protection.
    pub async fn get_source(&self, url: &str, proxy: Option<ProxyConfig>) -> ChaserResult<String> {
        let browser = self.browser().await?;
        let manager = browser.as_ref().ok_or(ChaserError::NotInitialized)?;

        tokio::time::timeout(
            self.config.timeout(),
            solver::get_source(manager, url, proxy),
        )
        .await
        .map_err(|_| ChaserError::Timeout(self.config.timeout_ms))?
    }

    /// Create a WAF session with cookies and headers for authenticated requests
    ///
    /// # Arguments
    ///
    /// * `url` - Target URL to create session for
    /// * `proxy` - Optional proxy configuration
    ///
    /// # Returns
    ///
    /// A `WafSession` containing cookies and headers that can be used for
    /// subsequent requests to the same site.
    pub async fn solve_waf_session(
        &self,
        url: &str,
        proxy: Option<ProxyConfig>,
    ) -> ChaserResult<WafSession> {
        let browser = self.browser().await?;
        let manager = browser.as_ref().ok_or(ChaserError::NotInitialized)?;

        tokio::time::timeout(
            self.config.timeout(),
            solver::solve_waf_session(manager, url, proxy),
        )
        .await
        .map_err(|_| ChaserError::Timeout(self.config.timeout_ms))?
    }

    /// Solve a Turnstile captcha with full page load
    ///
    /// # Arguments
    ///
    /// * `url` - URL containing the Turnstile widget
    /// * `proxy` - Optional proxy configuration
    ///
    /// # Returns
    ///
    /// The Turnstile token string.
    pub async fn solve_turnstile(
        &self,
        url: &str,
        proxy: Option<ProxyConfig>,
    ) -> ChaserResult<String> {
        let browser = self.browser().await?;
        let manager = browser.as_ref().ok_or(ChaserError::NotInitialized)?;

        tokio::time::timeout(
            self.config.timeout(),
            solver::solve_turnstile_max(manager, url, proxy),
        )
        .await
        .map_err(|_| ChaserError::Timeout(self.config.timeout_ms))?
    }

    /// Solve a Turnstile captcha with minimal resource usage
    ///
    /// This mode intercepts the page request and serves a minimal HTML page
    /// that only loads the Turnstile widget. Requires the site key.
    ///
    /// # Arguments
    ///
    /// * `url` - URL to use as the Turnstile origin
    /// * `site_key` - The Turnstile site key
    /// * `proxy` - Optional proxy configuration
    ///
    /// # Returns
    ///
    /// The Turnstile token string.
    pub async fn solve_turnstile_min(
        &self,
        url: &str,
        site_key: &str,
        proxy: Option<ProxyConfig>,
    ) -> ChaserResult<String> {
        let browser = self.browser().await?;
        let manager = browser.as_ref().ok_or(ChaserError::NotInitialized)?;

        tokio::time::timeout(
            self.config.timeout(),
            solver::solve_turnstile_min(manager, url, site_key, proxy),
        )
        .await
        .map_err(|_| ChaserError::Timeout(self.config.timeout_ms))?
    }

    /// Get configuration
    pub fn config(&self) -> &ChaserConfig {
        &self.config
    }
}

impl Drop for ChaserCF {
    fn drop(&mut self) {
        // Note: async drop is not supported, so we just log a warning
        // if the browser wasn't explicitly shut down
        if let Ok(guard) = self.browser.try_read() {
            if guard.is_some() {
                tracing::warn!(
                    "ChaserCF dropped without explicit shutdown(). \
                     Call shutdown() for clean resource release."
                );
            }
        }
    }
}
