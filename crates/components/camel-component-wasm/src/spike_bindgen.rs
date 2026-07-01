//! Spike evidence: bindgen output verification for WIT with stream<u8> + async func.
//! Compiles but never called.

use wasmtime::component::bindgen;

bindgen!({
    path: "wit-spike/camel-spike.wit",
    world: "spike-world",
    imports: { default: async | store },
    exports: { default: async | store },
});

// === DISCOVERY 6: stream<u8> → StreamReader<u8> ===
// === DISCOVERY 7: future<result<_, E>> → FutureReader<Result<(), E>> ===
// === DISCOVERY 8: `stream` is WIT keyword, escape with %stream ===
// === DISCOVERY 9: separate wit dir needed (package collision) ===

const _: fn() = || {
    fn check_types(
        _body: camel::spike::types::SpikeBody,
        _handle: camel::spike::types::StreamBodyHandle,
        _exchange: camel::spike::types::SpikeExchange,
    ) {}

    fn check_stream_field_types(
        _stream: wasmtime::component::StreamReader<u8>,
        _terminal: wasmtime::component::FutureReader<Result<(), camel::spike::types::WasmError>>,
    ) {}
};
