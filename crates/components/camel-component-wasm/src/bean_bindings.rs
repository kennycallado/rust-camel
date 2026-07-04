wasmtime::component::bindgen!({
    path: "wit",
    world: "bean",
    imports: {
        // Only genuinely-async host funcs use `async`; sync funcs stay sync so
        // the registered import ABI matches guests that import them as sync.
        "camel:plugin/host.camel-call": async | store,
        "camel:plugin/host.camel-poll": async | store,
        default: store,
    },
    exports: {
        default: async | store,
    },
});
