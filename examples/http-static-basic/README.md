# http-static-basic example

Minimal static file serving using the `http-static` scheme. Shows how to serve
files from a directory with no additional route processing.

## Layout

```
http-static-basic/
├── Camel.toml              # Context configuration
├── public/
│   ├── index.html          # Sample HTML page
│   └── style.css           # Sample CSS
└── routes/
    └── static.yaml         # Route definition
```

## Running

First build the CLI:

```bash
# from the workspace root
cargo build -p camel-cli
```

Then run from this directory:

```bash
cd examples/http-static-basic
../../target/debug/camel run
```

Open http://localhost:8080 in your browser.

## How it works

- `[default.components.http-static]` in Camel.toml sets the directory (`public/`) and port.
- `from: "http-static:/"` in the route definition mounts files at the root URL path.
- The consumer registers into a per-port axum server and stays idle — no exchange pipeline needed.
