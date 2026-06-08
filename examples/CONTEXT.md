## Log-level policy

The three example apps (container-hot-reload, hot-reload, hot-reload-yaml) each have a single `error!` site at startup (bootstrap failure — the demo cannot proceed). All sites: (c) system-broken. Each keeps `error!` with `// log-policy: system-broken`. No metric call (the demo is exiting).

Sites:
- `examples/container-hot-reload/src/main.rs:L105`
- `examples/hot-reload-yaml/src/main.rs:L142`
- `examples/hot-reload/src/main.rs:L102`
