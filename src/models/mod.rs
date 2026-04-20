//! Data models for chaser-cf

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Stealth profile for browser fingerprinting
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[repr(C)]
pub enum Profile {
    /// Windows fingerprint (most common, default)
    #[default]
    Windows,
    /// Linux fingerprint
    Linux,
    /// macOS fingerprint
    Macos,
}

impl Profile {
    /// Parse profile from string (for FFI)
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "windows" | "win" => Some(Profile::Windows),
            "linux" => Some(Profile::Linux),
            "macos" | "mac" | "darwin" => Some(Profile::Macos),
            _ => None,
        }
    }
}

/// Proxy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// Proxy host
    pub host: String,
    /// Proxy port
    pub port: u16,
    /// Optional username for authentication
    pub username: Option<String>,
    /// Optional password for authentication
    pub password: Option<String>,
    /// Optional URL scheme. Defaults to "http" for backward compatibility.
    /// Chrome's `--proxy-server` natively supports `http`, `https`,
    /// `socks4`, `socks5`. Use `socks5h` for remote DNS resolution.
    /// Set via [`ProxyConfig::with_scheme`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
}

impl ProxyConfig {
    /// Create new proxy config
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            username: None,
            password: None,
            scheme: None,
        }
    }

    /// Add authentication credentials
    pub fn with_auth(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self.password = Some(password.into());
        self
    }

    /// Override the URL scheme. Default is "http". Common values:
    /// - "http" — HTTP CONNECT (default; Chrome's classic proxy mode)
    /// - "https" — HTTPS CONNECT
    /// - "socks5" — SOCKS5 with local DNS
    /// - "socks5h" — SOCKS5 with remote (proxy-side) DNS
    /// - "socks4" — SOCKS4
    pub fn with_scheme(mut self, scheme: impl Into<String>) -> Self {
        self.scheme = Some(scheme.into());
        self
    }

    /// Get proxy URL in the form `<scheme>://host:port`. Defaults the
    /// scheme to `http` when none has been explicitly set.
    /// Note: Chrome/CDP doesn't support inline authentication in proxy URLs.
    pub fn to_url(&self) -> String {
        let scheme = self.scheme.as_deref().unwrap_or("http");
        format!("{scheme}://{}:{}", self.host, self.port)
    }
}

/// Browser cookie
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cookie {
    /// Cookie name
    pub name: String,
    /// Cookie value
    pub value: String,
    /// Cookie domain
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Cookie path
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Expiration timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<f64>,
    /// HTTP only flag
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_only: Option<bool>,
    /// Secure flag
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secure: Option<bool>,
    /// SameSite attribute
    #[serde(skip_serializing_if = "Option::is_none")]
    pub same_site: Option<String>,
}

impl Cookie {
    /// Create a simple cookie with name and value
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            domain: None,
            path: None,
            expires: None,
            http_only: None,
            secure: None,
            same_site: None,
        }
    }
}

/// WAF session data containing cookies and headers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WafSession {
    /// Extracted cookies
    pub cookies: Vec<Cookie>,
    /// Extracted headers (cleaned)
    pub headers: HashMap<String, String>,
}

impl WafSession {
    /// Create new WAF session
    pub fn new(cookies: Vec<Cookie>, headers: HashMap<String, String>) -> Self {
        Self { cookies, headers }
    }

    /// Get cookies as a single cookie header string
    pub fn cookies_string(&self) -> String {
        self.cookies
            .iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// Result of a chaser-cf operation (for FFI serialization)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ChaserResult {
    /// Page source HTML
    Source(String),
    /// Turnstile token
    Token(String),
    /// WAF session
    WafSession(WafSession),
    /// Error
    Error { code: i32, message: String },
}

impl ChaserResult {
    /// Create success result with source
    pub fn source(html: String) -> Self {
        ChaserResult::Source(html)
    }

    /// Create success result with token
    pub fn token(token: String) -> Self {
        ChaserResult::Token(token)
    }

    /// Create success result with WAF session
    pub fn waf_session(session: WafSession) -> Self {
        ChaserResult::WafSession(session)
    }

    /// Create error result
    pub fn error(code: i32, message: impl Into<String>) -> Self {
        ChaserResult::Error {
            code,
            message: message.into(),
        }
    }

    /// Check if result is success
    pub fn is_success(&self) -> bool {
        !matches!(self, ChaserResult::Error { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_url_without_auth() {
        let proxy = ProxyConfig::new("proxy.example.com", 8080);
        assert_eq!(proxy.to_url(), "http://proxy.example.com:8080");
    }

    #[test]
    fn test_proxy_url_with_auth() {
        let proxy = ProxyConfig::new("proxy.example.com", 8080).with_auth("user", "pass");
        assert_eq!(proxy.to_url(), "http://proxy.example.com:8080");
    }

    #[test]
    fn test_proxy_url_with_socks5_scheme() {
        let proxy = ProxyConfig::new("127.0.0.1", 1080).with_scheme("socks5");
        assert_eq!(proxy.to_url(), "socks5://127.0.0.1:1080");
    }

    #[test]
    fn test_proxy_url_with_socks5h_scheme_remote_dns() {
        let proxy = ProxyConfig::new("127.0.0.1", 1080)
            .with_scheme("socks5h")
            .with_auth("u", "p");
        assert_eq!(proxy.to_url(), "socks5h://127.0.0.1:1080");
    }

    #[test]
    fn test_proxy_url_default_scheme_is_http() {
        // Backward compat: ProxyConfig::new() with no with_scheme()
        // must continue to emit http:// URLs, otherwise existing
        // callers break silently.
        let proxy = ProxyConfig::new("h", 1);
        assert!(proxy.scheme.is_none());
        assert_eq!(proxy.to_url(), "http://h:1");
    }

    #[test]
    fn test_waf_session_cookies_string() {
        let session = WafSession::new(
            vec![
                Cookie::new("cf_clearance", "abc123"),
                Cookie::new("session", "xyz789"),
            ],
            HashMap::new(),
        );
        assert_eq!(
            session.cookies_string(),
            "cf_clearance=abc123; session=xyz789"
        );
    }

    #[test]
    fn test_profile_from_str() {
        assert_eq!(Profile::parse("windows"), Some(Profile::Windows));
        assert_eq!(Profile::parse("LINUX"), Some(Profile::Linux));
        assert_eq!(Profile::parse("darwin"), Some(Profile::Macos));
        assert_eq!(Profile::parse("invalid"), None);
    }
}
