//! Minimal WASI interface registration per ADR-0050.
//!
//! [`register_minimal_wasi`] registers `wasi:clocks` (wall + monotonic) and
//! `wasi:random` (random + insecure + insecure_seed).
//!
//! [`register_command_adapter_wasi`] additionally registers the interfaces
//! that every `wasm32-wasip2` fixture imports by virtue of the WASI
//! `command` adapter: `wasi:io/{error,poll,streams}` and
//! `wasi:cli/{environment,exit,stdin,stdout,stderr,terminal-*}`. It does NOT
//! register `wasi:filesystem/*`, `wasi:sockets/*`, or name lookup — those
//! remain the testable denial boundary. The `WasiCtx`/`WasiCliCtx` back these
//! interfaces with no resources: empty env/args, closed stdin, sink
//! stdout/stderr, no preopens, no network. This is the ADR-0050
//! command-adapter exception (the toolchain emits the full command surface
//! even when the guest does not use it; per-world denial of these imported
//! instances would break instantiation).
//!
//! Filesystem, sockets, CLI-preopens, environment-grants, and inherited
//! stdio remain absent from every linker and ctx.

use wasmtime::component::{HasData, ResourceTable};
use wasmtime_wasi::cli::{WasiCli, WasiCliView};
use wasmtime_wasi::clocks::{WasiClocks, WasiClocksView as _};
use wasmtime_wasi::p2::bindings::{cli, clocks, io, random};
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

/// Projection helper mirroring the private `HasIo` in `wasmtime-wasi`. The
/// generated `io::*::add_to_linker` accessors borrow `&mut ResourceTable` out
/// of the caller's state via `WasiView::ctx().table`.
struct HasIo;

impl HasData for HasIo {
    type Data<'a> = &'a mut ResourceTable;
}

/// Register every WASI interface that a `wasm32-wasip2` command-adapter
/// component imports, so the existing fixtures instantiate under the hardened
/// `WasiCtx`/`WasiCliCtx`. Filesystem, sockets, and name lookup are
/// deliberately NOT registered — they are the denial boundary (ADR-0050).
///
/// `WasiView` blanket-implements `WasiCliView` (via the `cli` field inside
/// `WasiCtx`), so `T::cli` is available for any `T: WasiView`.
pub(crate) fn register_command_adapter_wasi<T: wasmtime_wasi::WasiView>(
    linker: &mut wasmtime::component::Linker<T>,
) -> Result<(), wasmtime::Error> {
    register_minimal_wasi(linker)?;

    // wasi:io/* — required by the command adapter's stdio implementation.
    io::error::add_to_linker::<T, HasIo>(linker, |t| t.ctx().table)?;
    io::poll::add_to_linker::<T, HasIo>(linker, |t| t.ctx().table)?;
    io::streams::add_to_linker::<T, HasIo>(linker, |t| t.ctx().table)?;

    // wasi:cli/* — imported by every command-adapter component. Backed by the
    // hardened WasiCliCtx inside WasiCtx (empty env/args, closed stdin, sink
    // stdout/stderr).
    cli::environment::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::exit::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::stdin::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::stdout::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::stderr::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::terminal_input::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::terminal_output::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::terminal_stdin::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::terminal_stdout::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::terminal_stderr::add_to_linker::<T, WasiCli>(linker, T::cli)?;
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
        let engine = wasmtime::Engine::new(&wasmtime::Config::new()).expect("engine");
        let mut linker: wasmtime::component::Linker<crate::runtime::WasmHostState> =
            wasmtime::component::Linker::new(&engine);
        register_minimal_wasi(&mut linker).expect("clocks+random register");
    }

    /// The command-adapter surface must register successfully. This is the
    /// surface every `wasm32-wasip2` fixture imports.
    #[test]
    fn test_register_command_adapter_wasi_succeeds() {
        let engine = wasmtime::Engine::new(&wasmtime::Config::new()).expect("engine");
        let mut linker: wasmtime::component::Linker<crate::runtime::WasmHostState> =
            wasmtime::component::Linker::new(&engine);
        register_command_adapter_wasi(&mut linker).expect("command-adapter surface registers");
    }

    /// Regression guard: the WASI surface must NOT register `wasi:filesystem`
    /// or `wasi:sockets` — these are the denial boundary (ADR-0050). Even
    /// though the hardened `WasiCtx` grants no preopens or network, admitting
    /// the host implementations would let a defective guest create host file
    /// descriptors and socket objects (DoS / attack surface).
    #[test]
    fn test_no_filesystem_or_sockets_registration() {
        let src = include_str!("wasi_surface.rs");
        // Build the needles dynamically so the guard does not match its own
        // source text (which necessarily mentions these interface names).
        let fs: String = ["filesystem", "::"].concat();
        let sock: String = ["sockets", "::"].concat();
        assert!(
            !src.contains(&fs) && !src.contains(&sock),
            "wasi_surface.rs must not register wasi:filesystem or wasi:sockets"
        );
    }
}
