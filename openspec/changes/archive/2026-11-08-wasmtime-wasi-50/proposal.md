# Proposal: wasmtime-wasi 49 → 50 component API migration

bd rc-y093x. Workspace pins move `wasmtime` and `wasmtime-wasi` from `49.0.4`
to `50` (resolves 50.0.1). `camel-component-wasm` inherits via
`workspace = true`; no crate-local version pins were added. The workspace
feature set for `wasmtime` is unchanged — every feature in the existing list
still exists in 50, none was removed.

## Why

RUSTSEC-2026-0325/0326/0327 fixes and ongoing component-API work live in the
50 line; staying on 49 blocks the Dependabot stream (PR #74, deferred by
rc-k93qw). The `scripts/check-wasmtime-floor.sh` floor (≥ 49.0.2) is
satisfied by the `50` requirement.

## API deltas hit (before → after, file:line)

1. **`AccessorTask` became a lifetime-carrying trait for async closures.**
   Upstream: wasmtime 49.0.5
   `src/runtime/component/concurrent.rs:628` declared
   `pub trait AccessorTask<T, D = HasSelf<T>>: Send + 'static` with
   `fn run(self, accessor: &Accessor<T, D>) -> impl Future<Output = Result<()>>
   + Send;` — manually implementable. wasmtime 50.0.1
   `src/runtime/component/concurrent.rs:648` declares
   `pub trait AccessorTask<'fut, T, D = HasSelf<T>>:
   AsyncFnOnce(&Accessor<T, D>) -> Result<()> + Send + 'static` with a blanket
   impl for async closures; the docs state manual user impls are no longer
   intended, and `Accessor::spawn` now takes
   `impl for<'fut> AccessorTask<'fut, T, D>` (concurrent.rs:530).
   Host change in `crates/components/camel-component-wasm/src/source_host.rs`:
   - The `SubmitExchangeDrain` struct + manual `AccessorTask` impl
     (previously lines 508–546) is deleted.
   - The spawn site (previously line 442) now passes an async closure to
     `accessor.spawn(...)` capturing the same four inputs (`stream_reader`,
     `terminal`, `chunk_tx`, `coord`), calling the same
     `drain_guest_stream::<SourceWasmError, SourceHostState>` and returning
     `Ok(())` with identical semantics (terminal errors surface through the
     chunk channel, not the task return).
   - `AccessorTask` dropped from the import list (source_host.rs:16).
   The drain still registers on the same event loop via `Accessor::spawn`;
   ordering (drain registered before the exchange is sent) and the
   spawn-failure fallback (`write_terminal`) are unchanged.

2. **No other host API delta required changes.** `WasiView`/`WasiCtxView`,
   `WasiCtxBuilder`, `WasiCliView`, the per-interface
   `add_to_linker::<T, WasiClocks/WasiRandom/WasiCli>` signatures in
   `wasi_surface.rs`, and the private `HasData` mirror (`struct HasIo`)
   compile unchanged against 50.0.1. The upstream `HasIo` type remains
   private in 50 (`wasmtime-wasi-50.0.1/src/p2/mod.rs:488`), so the local
   mirror in `wasi_surface.rs:41` stays necessary. The error-conversion and
   type changes anticipated in rc-y093x did not materialize for this crate's
   call surface; `cargo check -p camel-component-wasm --all-targets` after
   the single source fix was the only compile iteration.

3. **Registration changes: none.** `register_minimal_wasi` and
   `register_command_adapter_wasi` are byte-identical. The denial boundary
   (no `wasi:filesystem`, no `wasi:sockets`, no name lookup) and the
   hardened `WasiCtx`/`WasiCliCtx` (empty env/args, closed stdin, sink
   stdio, no preopens, no network) are preserved exactly; the hardened
   context gains no permissions.

## Guest contract: untouched

No WIT world, exported function, or capability semantics changed. Proof:

- All seven guest crates build clean against `wasm32-wasip2` (CI's WIT
  oracle, ADR-0054): `examples/wasm-source-webhook/guest`,
  `examples/wasm-bean-example/guest`,
  `examples/wasm-streaming-plugin/guest`,
  `examples/security-wasm-policy/guest`, `examples/security-wasm-policy/guest-init-check`,
  `examples/wasm-example/guest`,
  `crates/components/camel-component-wasm/tests/fixtures/conflicting-bind-guest`.
- No guest fixture was rebuilt; the committed `tests/fixtures/*.wasm` load
  and pass in the suite (bean/return-stream/producer-emission/streaming
  integration targets).
- No `.wasm` was hand-edited; no test was deleted or newly `#[ignore]`d.
  The pre-existing `#[ignore]` gates (committed baseline, run in CI with
  `-- --ignored`) all pass locally with `-- --ignored`, including
  `source_stream_integration` (7/7), which exercises the rewritten
  async-closure drain end-to-end.

## Known consequence (out of scope, for the master)

wasmtime 50 declares `rust-version = "1.97.0"` (both crates). The workspace
declares `rust-version = "1.96"` (Cargo.toml:65) and CI's `MSRV (1.96.0)`
job therefore cannot build this stack with the bump. Local MSRV check not
run: `N/A — toolchain not installed` (no rustup on this host; only stable
1.99.1). CI's MSRV job owns the verification; moving workspace MSRV to
≥ 1.97.0 is a separate decision for the master.
