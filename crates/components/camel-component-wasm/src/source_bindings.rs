// Async bindings.
//
// The source world's `run` export and `accept-http`/`submit-exchange` imports
// are async. The guest runs as a tokio task driving `run` (via
// `store.run_concurrent`); host imports receive an `&Accessor`
// (HostWithStore) and use async channel ops (`.recv().await`/`.send().await`).
// `is-cancelled` stays sync (a `with` peek that must not yield). See
// docs/superpowers/specs/2026-07-08-wasm-source-async-stream-design.md.
wasmtime::component::bindgen!({
    path: "wit",
    world: "source",
    with: {
        "camel:plugin/source-host.http-listener": crate::source_host::HttpListenerHandle,
    },
});
