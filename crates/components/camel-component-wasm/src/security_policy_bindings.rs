wasmtime::component::bindgen!({
    path: "wit",
    world: "authorization-policy",
    imports: {
        // Only the genuinely-async host functions use the `async` convention;
        // the rest stay sync. A global `default: async | store` would register
        // sync WIT funcs (e.g. get-property) with an async ABI, which fails to
        // match guests that import them as sync.
        "camel:plugin/host.camel-call": async | store,
        "camel:plugin/host.camel-poll": async | store,
        default: store,
    },
    exports: {
        default: async | store,
    },
});
