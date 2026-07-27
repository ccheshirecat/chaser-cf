# chaser-cf

[![crates.io](https://img.shields.io/crates/v/chaser-cf.svg)](https://crates.io/crates/chaser-cf)
[![docs.rs](https://docs.rs/chaser-cf/badge.svg)](https://docs.rs/chaser-cf)
[![license](https://img.shields.io/crates/l/chaser-cf.svg)](LICENSE-MIT)

Librería para bypass de Cloudflare impulsada por automatización de navegador stealth. No se requieren tokens de API de captcha: resolución de desafíos basada puramente en el navegador con bindings C FFI para su uso desde cualquier lenguaje.

## Cómo funciona

- Lanza una instancia real de Chrome con una huella digital nativa (SO, RAM, UA, Client Hints, todo consistente).
- Para desafíos WAF/gestionados: busca `cf_clearance` y hace clic en la casilla de Turnstile mediante el recorrido del shadow-root de CDP — el widget de Cloudflare reside dentro de un shadow root cerrado al que JS no puede acceder, pero CDP sí.
- Para tokens de Turnstile: inyecta un script extractor y, opcionalmente, intercepta la solicitud de la página para servir un stub HTML mínimo, reduciendo el tiempo de carga y el ruido.
- Construido sobre [chaser-oxide](https://github.com/ccheshirecat/chaser-oxide), un fork stealth de chromiumoxide.

## Características

- **WAF Session** — extrae `cf_clearance` + `user-agent` para su uso en solicitudes HTTP posteriores.
- **Turnstile max** — resuelve Turnstile con carga completa de la página (no requiere site key).
- **Turnstile min** — resuelve Turnstile con interceptación de solicitudes, mucho más rápido (requiere site key).
- **Page source** — devuelve el HTML después de que el desafío haya sido superado.
- **Soporte de Proxy** — proxy por solicitud con autenticación opcional.
- **Pantalla virtual** — Chrome con interfaz (headed) respaldado por Xvfb en servidores Linux (requerido para desafíos gestionados de CF).
- **C FFI** — uso desde Python, Go, Node.js, C/C++, etc.
- **Servidor HTTP** — API REST opcional (activable mediante feature-flag).

## Instalación

```toml
[dependencies]
chaser-cf = { version = "0.2.1" }
```

Requiere que Chrome o Chromium estén instalados en el sistema.

## Uso

### Rust

```rust
use chaser_cf::{ChaserCF, ChaserConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let chaser = ChaserCF::new(ChaserConfig::default()).await?;

    // WAF session — devuelve cookies + user-agent para usar en reqwest/ureq/etc.
    let session = chaser.solve_waf_session("https://example.com", None).await?;
    println!("cf_clearance: {}", session.cookies_string());
    println!("user-agent: {}", session.headers["user-agent"]);

    // Código fuente de la página después de superar el desafío
    let html = chaser.get_source("https://example.com", None).await?;

    // Token de Turnstile (página completa, no requiere site key)
    let token = chaser.solve_turnstile("https://example.com", None).await?;

    // Token de Turnstile (rápido, interceptación de solicitud — requiere site key)
    let token = chaser.solve_turnstile_min(
        "https://example.com",
        "0x4AAAAAAxxxxx",
        None,
    ).await?;

    chaser.shutdown().await;
    Ok(())
}
```

Con proxy:

```rust
use chaser_cf::ProxyConfig;

let proxy = ProxyConfig::new("1.2.3.4", 8080)
    .with_auth("user", "pass");

let session = chaser.solve_waf_session("https://example.com", Some(proxy)).await?;
```

### Configuración

```rust
let config = ChaserConfig::default()
    .with_context_limit(10)       // max contextos de navegador concurrentes
    .with_timeout_ms(60_000)      // timeout por operación
    .with_lazy_init(true)         // no lanzar el navegador hasta el primer uso
    .with_headless(true)          // modo headless (ver nota abajo)
    .with_virtual_display(true)   // Linux: iniciar Xvfb, ejecutar Chrome headed dentro de él
    .with_chrome_path("/usr/bin/chromium")
    .add_extra_arg("--no-sandbox");  // requerido al ejecutar como root
```

| Opción | Predeterminado | Descripción |
|--------|----------------|-------------|
| `context_limit` | 20 | Máx. de contextos de navegador concurrentes |
| `timeout_ms` | 60000 | Timeout por operación (ms) |
| `lazy_init` | false | Diferir el lanzamiento del navegador hasta el primer uso |
| `headless` | false | Ejecutar el navegador en modo headless |
| `virtual_display` | false | Linux: usar pantalla virtual Xvfb (anula `headless`) |
| `extra_args` | `[]` | Flags adicionales de Chrome (ej. `--no-sandbox`, `--disable-gpu`) |
| `chrome_path` | auto | Ruta al binario de Chrome/Chromium |

### Despliegue en servidor Linux

El desafío gestionado/interactivo de Cloudflare detecta Chrome headless a nivel de binario. En servidores Linux, use `--virtual-display` para ejecutar Chrome headed dentro de un framebuffer virtual Xvfb — este es el mismo enfoque utilizado por cf-clearance-scraper y otras herramientas de bypass públicas.

```bash
apt install xvfb
```

```rust
let config = ChaserConfig::default()
    .with_virtual_display(true)
    .add_extra_arg("--no-sandbox");  // si se ejecuta como root
```

O mediante variables de entorno:

```bash
CHASER_VIRTUAL_DISPLAY=true CHASER_EXTRA_ARGS="--no-sandbox" cargo run ...
```

El modo headless (`with_headless(true)`) funciona para flujos exclusivos de Turnstile (min/max) pero fallará en los desafíos WAF/gestionados de CF — use `virtual_display` para esos casos.

### Pruebas

```bash
cargo run --example test_turnstile -- <url> [options]

Options:
  --headless                   Ejecutar en modo headless
  --virtual-display            Iniciar Xvfb, ejecutar Chrome headed (Linux, requiere xvfb)
  --no-sandbox                 Ejecutar Chrome sin sandbox (requerido al ejecutar como root)
  --proxy <host:port>          Dirección del proxy
  --proxy-auth <user:pass>     Credenciales del proxy
  --site-key <key>             Site key de Turnstile (habilita el modo min)
  --timeout <ms>               Timeout en ms (predeterminado: 120000)
  --mode <waf|min|max|all>     Qué resolver (predeterminado: waf)

# Ejemplos
cargo run --example test_turnstile -- https://stake.com
cargo run --example test_turnstile -- https://stake.com --virtual-display --mode waf
cargo run --example test_turnstile -- https://stake.com --virtual-display --no-sandbox --mode waf
cargo run --example test_turnstile -- https://example.com --site-key 0x4AA... --mode min
cargo run --example test_turnstile -- https://stake.com --proxy 1.2.3.4:8080 --proxy-auth user:pass
```

## Servidor HTTP

```bash
cargo run --release --features http-server --bin chaser-cf-server
```

```bash
# WAF session
curl -X POST http://localhost:3000/solve \
  -H "Content-Type: application/json" \
  -d '{"mode": "waf-session", "url": "https://example.com"}'

# Código fuente de la página
curl -X POST http://localhost:3000/solve \
  -H "Content-Type: application/json" \
  -d '{"mode": "source", "url": "https://example.com"}'

# Turnstile (página completa)
curl -X POST http://localhost:3000/solve \
  -H "Content-Type: application/json" \
  -d '{"mode": "turnstile-max", "url": "https://example.com"}'

# Turnstile (mínimo)
curl -X POST http://localhost:3000/solve \
  -H "Content-Type: application/json" \
  -d '{"mode": "turnstile-min", "url": "https://example.com", "siteKey": "0x4AAA..."}'

# Con proxy
curl -X POST http://localhost:3000/solve \
  -H "Content-Type: application/json" \
  -d '{"mode": "waf-session", "url": "https://example.com", "proxy": {"host": "1.2.3.4", "port": 8080}}'
```

### Variables de entorno

| Variable | Predeterminado | Descripción |
|----------|----------------|-------------|
| `PORT` | `3000` | Puerto del servidor HTTP |
| `CHASER_CONTEXT_LIMIT` | `20` | Máx. de contextos concurrentes |
| `CHASER_TIMEOUT` | `60000` | Timeout (ms) |
| `CHASER_LAZY_INIT` | `false` | Inicialización perezosa del navegador |
| `CHASER_HEADLESS` | `false` | Modo headless |
| `CHASER_VIRTUAL_DISPLAY` | `false` | Pantalla virtual Xvfb (Linux) |
| `CHASER_EXTRA_ARGS` | ninguno | Flags de Chrome separados por espacios (ej. `--no-sandbox --disable-gpu`) |
| `CHROME_BIN` | auto | Ruta al binario de Chrome |
| `AUTH_TOKEN` | ninguno | Token de autenticación de API opcional |

## C FFI

Construya la librería compartida:

```bash
cargo build --release
# Los headers se escriben en include/chaser_cf.h
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

## Formato de respuesta

```json
{ "type": "WafSession", "data": { "cookies": [{"name": "cf_clearance", "value": "..."}], "headers": {"user-agent": "..."} } }
{ "type": "Token",      "data": "0.abc123..." }
{ "type": "Source",     "data": "<html>..." }
{ "type": "Error",      "data": {"code": 6, "message": "timed out"} }
```

## Licencia

MIT OR Apache-2.0
