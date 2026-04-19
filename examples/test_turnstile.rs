use chaser_cf::{ChaserCF, ChaserConfig, ProxyConfig};
use std::env;

fn usage() {
    eprintln!("Usage: test_turnstile <url> [options]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --proxy <host:port>         Proxy address");
    eprintln!("  --proxy-auth <user:pass>    Proxy credentials");
    eprintln!("  --headless                  Run headless");
    eprintln!("  --site-key <key>            Turnstile site key (enables min mode)");
    eprintln!("  --timeout <ms>              Timeout in ms (default: 120000)");
    eprintln!("  --mode <waf|min|max|all>    What to solve (default: waf)");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  test_turnstile https://stake.com");
    eprintln!("  test_turnstile https://stake.com --headless --mode waf");
    eprintln!("  test_turnstile https://winna.com --site-key 0x4AA... --mode min");
    eprintln!("  test_turnstile https://stake.com --proxy 1.2.3.4:8080 --proxy-auth user:pass");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 || args[1] == "--help" || args[1] == "-h" {
        usage();
        return Ok(());
    }

    let url = args[1].clone();

    let mut headless = false;
    let mut proxy_addr: Option<String> = None;
    let mut proxy_auth: Option<(String, String)> = None;
    let mut site_key: Option<String> = None;
    let mut timeout_ms: u64 = 120_000;
    let mut mode = "waf".to_string();

    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--headless" => headless = true,
            "--proxy" => {
                i += 1;
                proxy_addr = Some(args[i].clone());
            }
            "--proxy-auth" => {
                i += 1;
                let parts: Vec<&str> = args[i].splitn(2, ':').collect();
                if parts.len() == 2 {
                    proxy_auth = Some((parts[0].to_string(), parts[1].to_string()));
                } else {
                    eprintln!("Invalid proxy-auth format, expected user:pass");
                    return Ok(());
                }
            }
            "--site-key" => {
                i += 1;
                site_key = Some(args[i].clone());
            }
            "--timeout" => {
                i += 1;
                timeout_ms = args[i].parse().unwrap_or(120_000);
            }
            "--mode" => {
                i += 1;
                mode = args[i].clone();
            }
            other => {
                eprintln!("Unknown argument: {other}");
                usage();
                return Ok(());
            }
        }
        i += 1;
    }

    tracing_subscriber::fmt()
        .with_env_filter("chaser_cf=debug,chaser_oxide=info")
        .init();

    let proxy = proxy_addr.map(|addr| {
        let parts: Vec<&str> = addr.rsplitn(2, ':').collect();
        let port: u16 = parts[0].parse().unwrap_or(8080);
        let host = parts.get(1).copied().unwrap_or(&addr).to_string();
        let mut p = ProxyConfig::new(host, port);
        if let Some((user, pass)) = proxy_auth {
            p = p.with_auth(user, pass);
        }
        p
    });

    let config = ChaserConfig::default()
        .with_timeout_ms(timeout_ms)
        .with_headless(headless);

    println!("Target : {url}");
    println!("Mode   : {mode}");
    println!("Headless: {headless}");
    if let Some(ref p) = proxy {
        println!("Proxy  : {}", p.to_url());
    }
    println!();

    let chaser = ChaserCF::new(config).await?;

    match mode.as_str() {
        "min" => {
            let key = site_key.as_deref().unwrap_or_else(|| {
                eprintln!("--site-key required for min mode");
                std::process::exit(1);
            });
            run_turnstile_min(&chaser, &url, key, proxy).await;
        }
        "max" => run_turnstile_max(&chaser, &url, proxy).await,
        "waf" => run_waf_session(&chaser, &url, proxy).await,
        "all" => {
            if let Some(ref key) = site_key {
                run_turnstile_min(&chaser, &url, key, proxy.clone()).await;
            }
            run_turnstile_max(&chaser, &url, proxy.clone()).await;
            run_waf_session(&chaser, &url, proxy).await;
        }
        other => {
            eprintln!("Unknown mode: {other}. Use waf, min, max, or all.");
        }
    }

    chaser.shutdown().await;
    Ok(())
}

async fn run_waf_session(chaser: &ChaserCF, url: &str, proxy: Option<ProxyConfig>) {
    println!("── WAF session ──");
    match chaser.solve_waf_session(url, proxy).await {
        Ok(session) => {
            println!("OK — {} cookies", session.cookies.len());
            for c in &session.cookies {
                let preview = &c.value[..c.value.len().min(40)];
                println!("  {} = {}…", c.name, preview);
            }
            for (k, v) in &session.headers {
                println!("  {k}: {v}");
            }
        }
        Err(e) => println!("FAIL — {e}"),
    }
    println!();
}

async fn run_turnstile_max(chaser: &ChaserCF, url: &str, proxy: Option<ProxyConfig>) {
    println!("── Turnstile max ──");
    match chaser.solve_turnstile(url, proxy).await {
        Ok(token) => {
            println!("OK — {} chars", token.len());
            println!("  {}…", &token[..token.len().min(60)]);
        }
        Err(e) => println!("FAIL — {e}"),
    }
    println!();
}

async fn run_turnstile_min(chaser: &ChaserCF, url: &str, site_key: &str, proxy: Option<ProxyConfig>) {
    println!("── Turnstile min (key: {site_key}) ──");
    match chaser.solve_turnstile_min(url, site_key, proxy).await {
        Ok(token) => {
            println!("OK — {} chars", token.len());
            println!("  {}…", &token[..token.len().min(60)]);
        }
        Err(e) => println!("FAIL — {e}"),
    }
    println!();
}
