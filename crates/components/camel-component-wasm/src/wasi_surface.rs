//! Minimal WASI interface registration per ADR-0050.
//!
//! Only `wasi:clocks` (wall + monotonic) and `wasi:random` (random + insecure
//! + insecure_seed) are registered.
//!
//! Filesystem, sockets, CLI, environment, and stdio are absent from the linker.

use wasmtime_wasi::clocks::{WasiClocks, WasiClocksView as _};
use wasmtime_wasi::p2::bindings::{clocks, random};
use wasmtime_wasi::random::{WasiRandom, WasiRandomView as _};

pub(crate) fn register_minimal_wasi<T: wasmtime_wasi::WasiView>(
    linker: &mut wasmtime::component::Linker<T>,
) -> Result<(), wasmtime::Error> {
    clocks::wall_clock::add_to_linker::<T, WasiClocks>(linker, T::clocks)?;
    clocks::monotonic_clock::add_to_linker::<T, WasiClocks>(linker, T::clocks)?;
    random::random::add_to_linker::<T, WasiRandom>(linker, T::random)?;
    random::insecure::add_to_linker::<T, WasiRandom>(linker, T::random)?;
    random::insecure_seed::add_to_linker::<T, WasiRandom>(linker, T::random)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression guard: no call site in the crate may use the full-surface
    /// `add_to_linker_async`. Only clocks+random are registered per ADR-0050.
    #[test]
    fn test_no_full_wasi_registration_in_crate() {
        let src = include_str!("runtime.rs");
        assert!(
            !src.contains("add_to_linker_async"),
            "runtime.rs must not use add_to_linker_async"
        );
        let src = include_str!("wasm_plugin_context.rs");
        assert!(
            !src.contains("add_to_linker_async"),
            "wasm_plugin_context.rs must not use add_to_linker_async"
        );
        let src = include_str!("source_host.rs");
        assert!(
            !src.contains("add_to_linker_async"),
            "source_host.rs must not use add_to_linker_async"
        );
    }

    /// Regression guard: no call site in the crate may inherit host stderr.
    #[test]
    fn test_no_inherit_stderr_in_crate() {
        let src = include_str!("runtime.rs");
        assert!(
            !src.contains("inherit_stderr"),
            "runtime.rs must not call inherit_stderr"
        );
        let src = include_str!("host_functions.rs");
        assert!(
            !src.contains("inherit_stderr"),
            "host_functions.rs must not call inherit_stderr"
        );
        let src = include_str!("stream_bridge.rs");
        assert!(
            !src.contains("inherit_stderr"),
            "stream_bridge.rs must not call inherit_stderr"
        );
    }

    /// The helper must register successfully into a fresh linker.
    #[test]
    fn test_register_minimal_wasi_succeeds() {
        let engine = wasmtime::Engine::new(&wasmtime::Config::new())
            .expect("engine");
        let mut linker: wasmtime::component::Linker<crate::runtime::WasmHostState> =
            wasmtime::component::Linker::new(&engine);
        register_minimal_wasi(&mut linker).expect("clocks+random register");
    }
}
