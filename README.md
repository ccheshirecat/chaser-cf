# chaser-cf

[![crates.io](https://img.shields.io/crates/v/chaser-cf.svg)](https://crates.io/crates/chaser-cf)
[![docs.rs](https://docs.rs/chaser-cf/badge.svg)](https://docs.rs/chaser-cf)
[![license](https://img.shields.io/crates/l/chaser-cf.svg)](LICENSE-MIT)

Cloudflare bypass library powered by stealth browser automation. No captcha API tokens needed — pure browser-based challenge solving with C FFI bindings for use from any language.

## How it works

- Launches a real Chrome instance with a native fingerprint (OS, RAM, UA, Client Hints all consistent)
- For WAF/managed challenges: polls for `cf_clearance` and clicks the Turnstile checkbox via CDP shadow-root traversal — Cloudflare's widget lives inside a closed shadow root that JS can't reach, but CDP can
- For Turnstile tokens: injects an extractor script and optionally intercepts the page request to serve a minimal HTML stub, reducing load time and noise
- Built on [chaser-oxide](https://github.com/ccheshirecat/chaser-oxide), a stealth fork of chromiumoxide

## Features

- **WAF Session** — extracts `cf_clearance` + `user-agent` for use in subsequent HTTP requests
- **Turnstile max** — solves Turnstile with full page load (no site key needed)
- **Turnstile min** — solves Turnstile with request interception, much faster (site key required)
- **Page source** — returns HTML after challenge is cleared
- **Proxy support** — per-request proxy with optional auth
- **C FFI** — use from Python, Go, Node.js, C/C++, etc.
- **HTTP server** — optional REST API (feature-flagged)

## Installation

```toml
[dependencies]
chaser-cf = { version = "0.1.8" }
```

Requires Chrome or Chromium installed on the system.

## Usage

### Rust

```rust
use chaser_cf::{ChaserCF, ChaserConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let chaser = ChaserCF::new(ChaserConfig::default()).await?;

    // WAF session — returns cookies + user-agent for use in reqwest/ureq/etc.
    let session = chaser.solve_waf_session("https://example.com", None).await?;
    println!("cf_clearance: {}", session.cookies_string());
    println!("user-agent: {}", session.headers["user-agent"]);

    // Page source after challenge cleared
    let html = chaser.get_source("https://example.com", None).await?;

    // Turnstile token (full page, no site key needed)
    let token = chaser.solve_turnstile("https://example.com", None).await?;

    // Turnstile token (fast, request interception — needs site key)
    let token = chaser.solve_turnstile_min(
        "https://example.com",
        "0x4AAAAAAxxxxx",
        None,
    ).await?;

    chaser.shutdown().await;
    Ok(())
}
```

With proxy:

```rust
use chaser_cf::ProxyConfig;

let proxy = ProxyConfig::new("1.2.3.4", 8080)
    .with_auth("user", "pass");

let session = chaser.solve_waf_session("https://example.com", Some(proxy)).await?;
```

### Configuration

```rust
let config = ChaserConfig::default()
    .with_context_limit(10)       // max concurrent browser contexts
    .with_timeout_ms(60_000)      // per-operation timeout
    .with_lazy_init(true)         // don't launch browser until first use
    .with_headless(true)          // headless mode
    .with_chrome_path("/usr/bin/chromium");
```

| Option | Default | Description |
|--------|---------|-------------|
| `context_limit` | 20 | Max concurrent browser contexts |
| `timeout_ms` | 60000 | Per-operation timeout (ms) |
| `lazy_init` | false | Defer browser launch until first use |
| `headless` | false | Run browser headless |
| `chrome_path` | auto | Path to Chrome/Chromium binary |

### Testing

```bash
cargo run --example test_turnstile -- <url> [options]

Options:
  --headless                   Run headless
  --proxy <host:port>          Proxy address
  --proxy-auth <user:pass>     Proxy credentials
  --site-key <key>             Turnstile site key (enables min mode)
  --timeout <ms>               Timeout in ms (default: 120000)
  --mode <waf|min|max|all>     What to solve (default: waf)

# Examples
cargo run --example test_turnstile -- https://stake.com
cargo run --example test_turnstile -- https://stake.com --headless --mode waf
cargo run --example test_turnstile -- https://example.com --site-key 0x4AA... --mode min
cargo run --example test_turnstile -- https://stake.com --proxy 1.2.3.4:8080 --proxy-auth user:pass
```

## HTTP Server

```bash
cargo run --release --features http-server --bin chaser-cf-server
```

```bash
# WAF session
curl -X POST http://localhost:3000/solve \
  -H "Content-Type: application/json" \
  -d '{"mode": "waf-session", "url": "https://example.com"}'

# Page source
curl -X POST http://localhost:3000/solve \
  -H "Content-Type: application/json" \
  -d '{"mode": "source", "url": "https://example.com"}'

# Turnstile (full page)
curl -X POST http://localhost:3000/solve \
  -H "Content-Type: application/json" \
  -d '{"mode": "turnstile-max", "url": "https://example.com"}'

# Turnstile (minimal)
curl -X POST http://localhost:3000/solve \
  -H "Content-Type: application/json" \
  -d '{"mode": "turnstile-min", "url": "https://example.com", "siteKey": "0x4AAA..."}'

# With proxy
curl -X POST http://localhost:3000/solve \
  -H "Content-Type: application/json" \
  -d '{"mode": "waf-session", "url": "https://example.com", "proxy": {"host": "1.2.3.4", "port": 8080}}'
```

### Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `3000` | HTTP server port |
| `CHASER_CONTEXT_LIMIT` | `20` | Max concurrent contexts |
| `CHASER_TIMEOUT` | `60000` | Timeout (ms) |
| `CHASER_LAZY_INIT` | `false` | Lazy browser init |
| `CHASER_HEADLESS` | `false` | Headless mode |
| `CHROME_BIN` | auto | Chrome binary path |
| `AUTH_TOKEN` | none | Optional API auth token |

## C FFI

Build the shared library:

```bash
cargo build --release
# headers written to include/chaser_cf.h
```

```c
#include "chaser_cf.h"

void on_result(const char* json, void* ctx) {
    printf("%s\n", json);
    chaser_free_string((char*)json);
}

int main() {
    ChaserConfig cfg = chaser_config_default();
    chaser_init(&cfg);
    chaser_solve_waf_async("https://example.com", NULL, NULL, on_result);
    sleep(30);
    chaser_shutdown();
}
```

```bash
gcc example.c -L./target/release -lchaser_cf -lpthread -ldl -lm -o example
```

### Python (ctypes)

```python
import ctypes, json, time
from ctypes import c_char_p, c_void_p, CFUNCTYPE

lib = ctypes.CDLL('./target/release/libchaser_cf.so')
CALLBACK = CFUNCTYPE(None, c_char_p, c_void_p)

result = []

@CALLBACK
def on_result(json_bytes, _):
    result.append(json.loads(json_bytes))
    lib.chaser_free_string(json_bytes)

lib.chaser_init(None)
lib.chaser_solve_waf_async(b"https://example.com", None, None, on_result)

while not result:
    time.sleep(0.1)

print(result[0])
lib.chaser_shutdown()
```

## Response format

```json
{ "type": "WafSession", "data": { "cookies": [{"name": "cf_clearance", "value": "..."}], "headers": {"user-agent": "..."} } }
{ "type": "Token",      "data": "0.abc123..." }
{ "type": "Source",     "data": "<html>..." }
{ "type": "Error",      "data": {"code": 6, "message": "timed out"} }
```

## License

MIT OR Apache-2.0
