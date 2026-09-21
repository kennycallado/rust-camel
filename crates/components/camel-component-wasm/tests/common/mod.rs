//! Shared helpers for the camel-component-wasm integration binaries.

/// Install a bare registry as the process-global tracing default, once
/// per test binary. Guards capture tests in this binary against
/// callsite-interest poisoning: `tracing` caches each callsite's
/// `Interest` process-wide from its FIRST macro execution, evaluated
/// against the executing thread's dispatcher. A subscriber-less sibling
/// test that hits a shared `warn!` callsite first caches
/// `Interest::never`, so a later thread-local `set_default` capture
/// silently drops events. The global registry heals prior poison and
/// floors future rebuilds at `sometimes` (fix pattern: c3853198; bd
/// rc-img5; convention: docs/testing/tracing-capture-guards.md).
///
/// Only `tests/source_bind_gate.rs` calls this today; the allow covers
/// the other common-consuming binaries that never reference it.
#[allow(dead_code)]
pub fn ensure_global_tracing_default() {
    static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if INIT.set(()).is_ok() {
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
    }
}

/// Bind a tokio listener on `{host}:0` and stage it under its exact
/// `(host, port)` key, returning the actual port.
///
/// Mirrors `stage_http_listener`/`stage_ws_listener` in camel-test: the
/// helper HOLDS the staged socket (inside the crate's staged-listener
/// map) until the source consumer consumes it at its bind site, so a
/// route that skips staging fails its fresh bind with `EADDRINUSE`
/// instead of silently serving on a different socket.
pub async fn stage_wasm_source_listener(host: &str) -> u16 {
    let listener = tokio::net::TcpListener::bind(format!("{host}:0"))
        .await
        .unwrap_or_else(|e| panic!("stage_wasm_source_listener: bind {host}:0 failed: {e}"));
    let addr = listener
        .local_addr()
        .unwrap_or_else(|e| panic!("stage_wasm_source_listener: local_addr for {host}: {e}"));
    camel_component_wasm::staged_listener::stage_listener(listener).unwrap_or_else(|e| {
        panic!(
            "stage_wasm_source_listener: staging {}:{} failed: {e}",
            addr.ip(),
            addr.port()
        )
    });
    addr.port()
}
