wasmtime::component::bindgen!({
    path: "../../../wit/camel-plugin.wit",
    world: "plugin",
    async: true,
});
