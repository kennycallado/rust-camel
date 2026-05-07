wasmtime::component::bindgen!({
    path: "wit/camel-plugin.wit",
    world: "plugin",
    exports: {
        default: async,
    },
});
