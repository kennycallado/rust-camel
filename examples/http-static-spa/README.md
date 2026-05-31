# http-static-spa example

SPA (Single Page Application) example with API routes, static file serving,
SPA fallback, and custom error pages — all on one port.

## Layout

```
http-static-spa/
├── Camel.toml              # Context configuration
├── public/
│   ├── index.html          # SPA entry point
│   └── 404.html            # Custom error page
└── routes/
    ├── api.yaml            # API route definition
    └── static.yaml         # Static file route definition
```

## Running

```bash
# from the workspace root
cargo build -p camel-cli

cd examples/http-static-spa
../../target/debug/camel run
```

## Endpoints

| Path | Handler | Response |
|------|---------|----------|
| `/` | Static file | `index.html` |
| `/api/hello` | API route | Log-only (TBD) |
| `/login` | SPA fallback | `index.html` (SPA mode) |
| `/missing.html` | Custom error | `404.html` with 404 status |

## How it works

- `[default.components.http]` and `[default.components.http-static]` share port 8080.
- Two routes: one `http:` API route and one `http-static:` SPA mount.
- SPA fallback (`spaFallback = true`): unmatched GET/HEAD requests with `Accept: text/html`
  and no file extension are served `index.html`.
- Custom 404 page loads `404.html` with status 404.
- Routing precedence: API exact match → static file (longest prefix) → SPA fallback → error page → 404.
