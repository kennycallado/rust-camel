wasmtime::component::bindgen!({
    path: "wit",
    world: "plugin",
    exports: {
        default: async,
    },
});
