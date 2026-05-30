wasmtime::component::bindgen!({
    path: "wit",
    world: "authorization-policy",
    exports: {
        default: async,
    },
});
