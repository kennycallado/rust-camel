//! Shared helpers for the camel-component-wasm integration binaries.

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
