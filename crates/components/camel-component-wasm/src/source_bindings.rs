// Synchronous bindings.
//
// The source world's host imports (`accept-http`, `submit-exchange`) block the
// guest thread on tokio channels via `blocking_recv`/`blocking_send`. That is
// only sound when the guest runs on a thread that is NOT a tokio runtime
// worker. Async exports would require driving the guest future with
// `block_on`, which marks the thread as a runtime context and makes
// `blocking_*` panic. Sync exports let the guest run on a plain OS thread
// where blocking is legal, so the whole guest lifecycle is synchronous.
wasmtime::component::bindgen!({
    path: "wit",
    world: "source",
    with: {
        "camel:plugin/source-host.http-listener": crate::source_host::HttpListenerHandle,
    },
});
