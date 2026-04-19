//! Solver implementations for chaser-cf

use super::BrowserManager;
use crate::error::{ChaserError, ChaserResult};
use crate::models::{Cookie, ProxyConfig, WafSession};

use chaser_oxide::auth::Credentials;
use std::collections::HashMap;
use std::time::Duration;

const FAKE_PAGE_HTML: &str = include_str!("../resources/fake_page.html");

/// Get page source from a Cloudflare-protected URL.
pub async fn get_source(
    manager: &BrowserManager,
    url: &str,
    proxy: Option<ProxyConfig>,
) -> ChaserResult<String> {
    let _permit = manager.acquire_permit().await?;
    let ctx_id = manager.create_context(proxy.as_ref()).await?;
    let (page, chaser) = manager.new_page(ctx_id, "about:blank").await?;

    setup_proxy_auth(&page, proxy.as_ref()).await?;

    chaser
        .goto(url)
        .await
        .map_err(|e| ChaserError::NavigationFailed(e.to_string()))?;

    wait_for_clearance(&page, &chaser, 30).await;

    page.content()
        .await
        .map_err(|e| ChaserError::Internal(e.to_string()))
}

/// Navigate to a Cloudflare-protected URL with a stealth browser, solve any interactive
/// challenge (including Turnstile managed challenges via CDP shadow-root click), and
/// return the resulting cookies + User-Agent for use in subsequent HTTP requests.
pub async fn solve_waf_session(
    manager: &BrowserManager,
    url: &str,
    proxy: Option<ProxyConfig>,
) -> ChaserResult<WafSession> {
    let _permit = manager.acquire_permit().await?;
    let ctx_id = manager.create_context(proxy.as_ref()).await?;
    let (page, chaser) = manager.new_page(ctx_id, "about:blank").await?;

    setup_proxy_auth(&page, proxy.as_ref()).await?;

    chaser
        .goto(url)
        .await
        .map_err(|e| ChaserError::NavigationFailed(e.to_string()))?;

    wait_for_clearance(&page, &chaser, 30).await;

    let raw_cookies = page
        .get_cookies()
        .await
        .map_err(|e| ChaserError::CookieExtractionFailed(e.to_string()))?;

    let cookies: Vec<Cookie> = raw_cookies
        .into_iter()
        .map(|c| Cookie {
            name: c.name,
            value: c.value,
            domain: Some(c.domain),
            path: Some(c.path),
            expires: Some(c.expires),
            http_only: Some(c.http_only),
            secure: Some(c.secure),
            same_site: c.same_site.map(|s| format!("{s:?}")),
        })
        .collect();

    let user_agent = chaser
        .evaluate("navigator.userAgent")
        .await
        .ok()
        .and_then(|v| v?.as_str().map(str::to_owned))
        .unwrap_or_default();

    let mut headers = HashMap::new();
    headers.insert("user-agent".to_string(), user_agent);

    Ok(WafSession::new(cookies, headers))
}

/// Solve Turnstile with full page load.
pub async fn solve_turnstile_max(
    manager: &BrowserManager,
    url: &str,
    proxy: Option<ProxyConfig>,
) -> ChaserResult<String> {
    let _permit = manager.acquire_permit().await?;
    let ctx_id = manager.create_context(proxy.as_ref()).await?;
    let (page, chaser) = manager.new_page(ctx_id, "about:blank").await?;

    setup_proxy_auth(&page, proxy.as_ref()).await?;

    page.evaluate_on_new_document(TURNSTILE_EXTRACTOR_SCRIPT)
        .await
        .map_err(|e| ChaserError::Internal(e.to_string()))?;

    chaser
        .goto(url)
        .await
        .map_err(|e| ChaserError::NavigationFailed(e.to_string()))?;

    wait_for_turnstile_token(&page, 60).await
}

/// Solve Turnstile with minimal resource usage (request interception mode).
pub async fn solve_turnstile_min(
    manager: &BrowserManager,
    url: &str,
    site_key: &str,
    proxy: Option<ProxyConfig>,
) -> ChaserResult<String> {
    use chaser_oxide::cdp::browser_protocol::fetch::EventRequestPaused;
    use chaser_oxide::cdp::browser_protocol::network::ResourceType;
    use futures::StreamExt;

    let _permit = manager.acquire_permit().await?;
    let ctx_id = manager.create_context(proxy.as_ref()).await?;
    let (page, chaser) = manager.new_page(ctx_id, "about:blank").await?;

    setup_proxy_auth(&page, proxy.as_ref()).await?;

    let fake_html = FAKE_PAGE_HTML.replace("<site-key>", site_key);

    chaser
        .enable_request_interception("*", Some(ResourceType::Document))
        .await
        .map_err(|e| ChaserError::Internal(format!("enable interception: {e}")))?;

    let mut request_events = page
        .event_listener::<EventRequestPaused>()
        .await
        .map_err(|e| ChaserError::Internal(format!("request listener: {e}")))?;

    let url_str = url.to_string();
    let fake_html_clone = fake_html.clone();
    let chaser_clone = chaser.clone();

    let intercept_handle = tokio::spawn(async move {
        while let Some(event) = request_events.next().await {
            let req_url = &event.request.url;
            let is_target = req_url == &url_str
                || req_url == &format!("{}/", url_str)
                || req_url.starts_with(&url_str);

            if is_target && event.resource_type == ResourceType::Document {
                let _ = chaser_clone
                    .fulfill_request_html(event.request_id.clone(), &fake_html_clone, 200)
                    .await;
            } else {
                let _ = chaser_clone
                    .continue_request(event.request_id.clone())
                    .await;
            }
        }
    });

    chaser
        .goto(url)
        .await
        .map_err(|e| ChaserError::NavigationFailed(e.to_string()))?;

    let token = wait_for_turnstile_token(&page, 60).await?;

    intercept_handle.abort();
    let _ = chaser.disable_request_interception().await;

    Ok(token)
}

// ─── helpers ────────────────────────────────────────────────────────────────

async fn setup_proxy_auth(
    page: &chaser_oxide::Page,
    proxy: Option<&ProxyConfig>,
) -> ChaserResult<()> {
    if let Some(p) = proxy {
        if let (Some(username), Some(password)) = (&p.username, &p.password) {
            page.authenticate(Credentials {
                username: username.clone(),
                password: password.clone(),
            })
            .await
            .map_err(|e| ChaserError::Internal(format!("proxy auth: {e}")))?;
        }
    }
    Ok(())
}

/// Poll until `cf_clearance` appears (meaning the challenge was solved) or the
/// timeout expires. If a challenge is still active, try clicking the Turnstile
/// checkbox via CDP shadow-root traversal every ~1.5 seconds.
async fn wait_for_clearance(
    page: &chaser_oxide::Page,
    chaser: &chaser_oxide::ChaserPage,
    timeout_seconds: u64,
) {
    let started = std::time::Instant::now();
    let timeout = Duration::from_secs(timeout_seconds);
    let mut last_click = started - Duration::from_secs(30);

    loop {
        if has_clearance_cookie(page).await {
            // Small settle delay so Set-Cookie propagates fully.
            tokio::time::sleep(Duration::from_millis(500)).await;
            return;
        }

        if started.elapsed() >= timeout {
            return;
        }

        if last_click.elapsed() >= Duration::from_millis(800) {
            try_click_challenge(chaser).await;
            last_click = std::time::Instant::now();
        }

        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// Return true if the browser has a `cf_clearance` cookie for any domain.
async fn has_clearance_cookie(page: &chaser_oxide::Page) -> bool {
    page.get_cookies()
        .await
        .map(|cookies| cookies.iter().any(|c| c.name == "cf_clearance"))
        .unwrap_or(false)
}

/// Click the Turnstile challenge element by traversing its closed shadow root via CDP.
///
/// Cloudflare's Turnstile widget lives inside a CLOSED shadow root. JS's
/// `element.shadowRoot` returns null for these, but CDP's `DOM.getDocument` with
/// `pierce: true` exposes them as `node.shadow_roots` — identical to what the Python
/// CF-Clearance-Scraper does with `parent.shadow_roots[0]`.
async fn try_click_challenge(chaser: &chaser_oxide::ChaserPage) {
    use chaser_oxide::cdp::browser_protocol::dom::{GetBoxModelParams, GetDocumentParams};

    let page = chaser.raw_page();

    let doc = match page
        .execute(GetDocumentParams {
            depth: Some(-1),
            pierce: Some(true),
        })
        .await
    {
        Ok(r) => r,
        Err(_) => return,
    };

    let Some(target_id) = find_shadow_challenge_node(&doc.result.root) else {
        return;
    };

    let box_model = match page
        .execute(GetBoxModelParams {
            node_id: Some(target_id),
            backend_node_id: None,
            object_id: None,
        })
        .await
    {
        Ok(r) => r,
        Err(_) => return,
    };

    let content = box_model.result.model.content.inner();
    if content.len() < 8 {
        return;
    }

    let cx = (content[0] + content[2]) / 2.0;
    let cy = (content[1] + content[5]) / 2.0;

    // Small random jitter so we never click the exact center every time.
    let (jx, jy, pre_ms) = {
        use rand::Rng as _;
        let mut rng = rand::thread_rng();
        (
            rng.gen_range(-4.0..=4.0_f64),
            rng.gen_range(-3.0..=3.0_f64),
            rng.gen_range(60..180_u64),
        )
    };
    let tx = cx + jx;
    let ty = cy + jy;

    // Curved approach from slightly above-left.
    let sx = tx - 80.0;
    let sy = ty - 35.0;
    for i in 1..=5_u8 {
        let t = i as f64 / 5.0;
        let arc = (std::f64::consts::PI * t).sin() * 8.0;
        let _ = page
            .move_mouse(chaser_oxide::layout::Point::new(
                sx + (tx - sx) * t + arc,
                sy + (ty - sy) * t,
            ))
            .await;
        let step_ms = {
            use rand::Rng as _;
            rand::thread_rng().gen_range(25..70_u64)
        };
        tokio::time::sleep(Duration::from_millis(step_ms)).await;
    }

    tokio::time::sleep(Duration::from_millis(pre_ms)).await;
    let _ = page.click(chaser_oxide::layout::Point::new(tx, ty)).await;
}

/// Walk the CDP DOM tree (shadow roots included via `pierce: true`) and return the
/// `NodeId` of the first visible child inside any shadow root — the Turnstile widget.
fn find_shadow_challenge_node(
    node: &chaser_oxide::cdp::browser_protocol::dom::Node,
) -> Option<chaser_oxide::cdp::browser_protocol::dom::NodeId> {
    if let Some(shadow_roots) = &node.shadow_roots {
        for sr in shadow_roots {
            if let Some(children) = &sr.children {
                for child in children {
                    let attrs = child.attributes.as_deref().unwrap_or(&[]);
                    let hidden = attrs
                        .chunks(2)
                        .any(|p| p.len() == 2 && p[0] == "style" && p[1].contains("display: none"));
                    if !hidden {
                        return Some(child.node_id);
                    }
                }
            }
        }
    }
    for child in node.children.as_deref().unwrap_or(&[]) {
        if let Some(id) = find_shadow_challenge_node(child) {
            return Some(id);
        }
    }
    None
}

const TURNSTILE_EXTRACTOR_SCRIPT: &str = r#"
    (function() {
        let token = null;
        async function waitForToken() {
            while (!token) {
                try { token = window.turnstile.getResponse(); } catch(e) {}
                await new Promise(r => setTimeout(r, 500));
            }
            var c = document.createElement("input");
            c.type = "hidden"; c.name = "cf-response"; c.value = token;
            document.body.appendChild(c);
        }
        waitForToken();
    })();
"#;

async fn wait_for_turnstile_token(
    page: &chaser_oxide::Page,
    timeout_seconds: u64,
) -> ChaserResult<String> {
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(timeout_seconds);
    let chaser = chaser_oxide::ChaserPage::new(page.clone());

    loop {
        if start.elapsed() > timeout {
            return Err(ChaserError::CaptchaFailed(
                "timeout waiting for token".into(),
            ));
        }

        let result = chaser
            .evaluate(
                r#"(function() {
                    if (window.turnstile && typeof window.turnstile.getResponse === 'function') {
                        var t = window.turnstile.getResponse();
                        if (t) return t;
                    }
                    var el = document.querySelector('[name="cf-response"]');
                    return el ? el.value : null;
                })()"#,
            )
            .await;

        if let Ok(Some(v)) = result {
            if let Some(t) = v.as_str() {
                if t.len() > 10 {
                    return Ok(t.to_string());
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
