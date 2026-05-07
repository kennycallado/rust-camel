wasmtime::component::bindgen!({
    path: "wit",
    world: "bean",
    exports: {
        default: async,
    },
});
