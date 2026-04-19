//! chaser-cf HTTP Server
//!
//! A REST API server for chaser-cf operations.
//! Enable with `cargo build --features http-server`

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use chaser_cf::{ChaserCF, ChaserConfig, ProxyConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

/// Server state
struct AppState {
    chaser: ChaserCF,
    auth_token: Option<String>,
}

/// Request body
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScraperRequest {
    mode: String,
    url: String,
    site_key: Option<String>,
    proxy: Option<ProxyConfigRequest>,
    auth_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ProxyConfigRequest {
    host: String,
    port: u16,
    username: Option<String>,
    password: Option<String>,
}

impl From<ProxyConfigRequest> for ProxyConfig {
    fn from(p: ProxyConfigRequest) -> Self {
        let mut proxy = ProxyConfig::new(p.host, p.port);
        if let (Some(u), Some(pw)) = (p.username, p.password) {
            proxy = proxy.with_auth(u, pw);
        }
        proxy
    }
}

/// Response body
#[derive(Debug, Serialize)]
struct ScraperResponse {
    code: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cookies: Option<Vec<chaser_cf::Cookie>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    headers: Option<HashMap<String, String>>,
}

impl ScraperResponse {
    fn success_source(source: String) -> Self {
        Self {
            code: 200,
            message: None,
            source: Some(source),
            token: None,
            cookies: None,
            headers: None,
        }
    }

    fn success_token(token: String) -> Self {
        Self {
            code: 200,
            message: None,
            source: None,
            token: Some(token),
            cookies: None,
            headers: None,
        }
    }

    fn success_waf(cookies: Vec<chaser_cf::Cookie>, headers: HashMap<String, String>) -> Self {
        Self {
            code: 200,
            message: None,
            source: None,
            token: None,
            cookies: Some(cookies),
            headers: Some(headers),
        }
    }

    fn error(code: u16, message: impl Into<String>) -> Self {
        Self {
            code,
            message: Some(message.into()),
            source: None,
            token: None,
            cookies: None,
            headers: None,
        }
    }
}

impl IntoResponse for ScraperResponse {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(self)).into_response()
    }
}

/// Main handler
async fn scraper_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ScraperRequest>,
) -> ScraperResponse {
    // Check auth token
    if let Some(ref expected) = state.auth_token {
        match &req.auth_token {
            Some(token) if token == expected => {}
            _ => return ScraperResponse::error(401, "Unauthorized"),
        }
    }

    let proxy = req.proxy.map(Into::into);

    match req.mode.as_str() {
        "source" => match state.chaser.get_source(&req.url, proxy).await {
            Ok(source) => ScraperResponse::success_source(source),
            Err(e) => ScraperResponse::error(500, e.to_string()),
        },
        "waf-session" => match state.chaser.solve_waf_session(&req.url, proxy).await {
            Ok(session) => ScraperResponse::success_waf(session.cookies, session.headers),
            Err(e) => ScraperResponse::error(500, e.to_string()),
        },
        "turnstile-max" => match state.chaser.solve_turnstile(&req.url, proxy).await {
            Ok(token) => ScraperResponse::success_token(token),
            Err(e) => ScraperResponse::error(500, e.to_string()),
        },
        "turnstile-min" => {
            let site_key = match req.site_key {
                Some(key) => key,
                None => return ScraperResponse::error(400, "siteKey required for turnstile-min"),
            };
            match state
                .chaser
                .solve_turnstile_min(&req.url, &site_key, proxy)
                .await
            {
                Ok(token) => ScraperResponse::success_token(token),
                Err(e) => ScraperResponse::error(500, e.to_string()),
            }
        }
        _ => ScraperResponse::error(400, format!("Unknown mode: {}", req.mode)),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("chaser_cf=info".parse()?),
        )
        .init();

    // Load config from env
    let config = ChaserConfig::from_env();
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);
    let auth_token = std::env::var("AUTH_TOKEN").ok().filter(|s| !s.is_empty());

    tracing::info!("Initializing chaser-cf...");
    let chaser = ChaserCF::new(config).await?;
    tracing::info!("chaser-cf initialized");

    let state = Arc::new(AppState { chaser, auth_token });

    let app = Router::new()
        .route("/solve", post(scraper_handler))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    tracing::info!("Server running on port {}", port);

    axum::serve(listener, app).await?;

    Ok(())
}
