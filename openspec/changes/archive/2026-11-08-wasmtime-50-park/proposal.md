# Proposal: park the wasmtime 50 bump for 0.66.0

## Why

`d8fd19c9a` moved `wasmtime`/`wasmtime-wasi` from the locked 49.0.5 to 50.0.1.
`wasmtime` 50.0.1 declares `rust-version = "1.97.0"` (registry
`wasmtime-50.0.1/Cargo.toml:14`), while the workspace floor is
`rust-version = "1.96"` (`Cargo.toml:65`) and the CI `msrv-check` job pins
`1.96.0` and runs `cargo check --workspace --locked`
(`.github/workflows/ci.yml:68-100`). The job is green on `origin/main` with the
49.0.5 lock (run 42470660723), so the bump alone turns it red.

ADR-0095, decision 1: "A dependency bump that raises the true floor above the
declared floor must either park or land together with a floor revision."
Decision 2: a floor change is a public-contract change and needs owner
ratification. The owner chose to park the bump for this release rather than
raise the floor to 1.97 mid-release (2026-11-08).

## What this change does

- `Cargo.toml`: `wasmtime`/`wasmtime-wasi` back to the `49.0.4` requirement main
  already carried (satisfied by the locked 49.0.5).
- `Cargo.lock`: family restored to **49.0.5**, the version the pre-bump lock
  carried and v0.65.0 shipped (`cargo update -p wasmtime --precise 49.0.5`, then
  `-p wasmtime-wasi --precise 49.0.5`); one version of each in the lock. This is
  an exact restoration, not a patch downgrade.
- `crates/components/camel-component-wasm/src/source_host.rs`: restored to the
  pre-migration form (`AccessorTask` manual impl, appropriate to 49).
- `CHANGELOG.md`: the `wasmtime-wasi 49 → 50` bullet is removed; 0.66.0 does not
  ship the bump.

## Verification

- `cargo metadata --locked`: the maximum `rust_version` over the whole graph is
  `1.96.0` — no package exceeds the declared floor. (Before: `wasmtime 50.0.1`
  demanded `1.97.0`.)
- One `wasmtime` and one `wasmtime-wasi` entry in `Cargo.lock`.
- `cargo check -p camel-component-wasm --locked` green.

## Non-goals

- No floor change, no ADR revision, no CI pin change.
- The archived `2026-11-08-wasmtime-wasi-50` change stays as history; the bump can
  be re-proposed together with a ratified 1.97 floor.
