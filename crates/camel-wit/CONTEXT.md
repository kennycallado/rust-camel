# WIT Contracts

`camel-wit` owns rust-camel's cross-language WebAssembly Interface Types
(WIT) contracts. It publishes the canonical WIT sources as embedded strings
and includes the raw `.wit` files in its crate package. It does not own guest
execution, resource limits, or sandbox capabilities. Those behaviors belong to
`camel-component-wasm` under ADR-0014, ADR-0031, and ADR-0050.

## Contract surface

All interfaces and worlds belong to one WIT package, `camel:plugin`. ADR-0053
defines its versioning policy.

| Export | Contract | Current external use |
|---|---|---|
| `PLUGIN_WIT` | `types` and `host` interfaces; `plugin` and `authorization-policy` worlds | Consumed by `camel-cli` templates. The WASM host keeps a drift-enforced copy (bindgen-forced; see model below). |
| `BEAN_WIT` | `bean` world over the plugin package interfaces | Consumed by `camel-cli` templates. The WASM host keeps a drift-enforced copy (bindgen-forced; see model below). |
| `SOURCE_WIT` | `source-host` interface and `source` world from ADR-0031 | No Rust consumer outside this crate. The WASM host keeps a drift-enforced copy (bindgen-forced; see model below). |
| `FULL_WIT` | Manually merged reference document for all worlds | No consumer outside this crate. `WIT-001` tracks its duplication. |

`camel-wit` is the contract source of truth and the canonical delivery vehicle
for the WIT contract. The contract has two consumer populations with different
constraints.

**Guest authors** (Population A: Python, Go, JavaScript, and other guest-side
wit-bindgen users) consume `camel-wit` directly. `cargo add camel-wit` exposes
the embedded strings (`PLUGIN_WIT`, `BEAN_WIT`, `SOURCE_WIT`, `FULL_WIT`), and
the raw `.wit` files ship in the published crate package for languages that read
them from disk.

**The WASM host** (`camel-component-wasm`, Population B) keeps a physical copy
of the three per-world `.wit` files under its own `wit/` directory. This copy is
not duplication debt. `wasmtime::component::bindgen!` requires a literal `path`
resolved against `CARGO_MANIFEST_DIR`, and the published host crate must be
self-contained, so an in-package `.wit` copy is structurally mandatory. The
non-skipping cross-crate test `test_host_wit_matches_canonical` in `src/lib.rs`
validates that copy against canonical on every CI run, converting a
compiler-forced build input into a verified projection of the source of truth.

`rc-osj0` originally tracked replacement of the host copies with consumption of
this crate. That goal is structurally unachievable under the wasmtime
literal-path constraint and crates.io self-containment. The issue has been
reframed to guest-delivery discoverability (see bd follow-up: camel-wit README
section for multi-language guest authors).

## Dependency posture

`camel-wit` is a zero-dependency leaf. WIT source publication does not need
runtime types or a WIT toolchain dependency. The `camel-api` dependency,
`WitHost`, the MIME constants such as `APPLICATION_JSON`, and `wit_dir()` have
been removed. This crate now has no `Cargo.toml` dependencies and no runtime
or convenience code outside contract publication.

## Interface evolution

The package is versioned at `camel:plugin@1.0.0` per ADR-0053. One SemVer
covers all interfaces and worlds and is independent from the Rust crate
version. Package identity, compatibility classification, and host support are
the contract evolution mechanism.

## Language

**WIT contract**:
The package-level interfaces, types, and worlds that define the ABI between a
WASM guest and the rust-camel host.
_Avoid_: Rust API, plugin implementation, host bindings

**WIT host**:
`camel-component-wasm`, which links generated bindings and executes guests.
_Avoid_: plugin runtime

## Self-grill record for DP-1

**Questions generated:**

1. [glossary] Does “WIT contract” conflict with an existing contract or WASM
   term in `CONTEXT-MAP.md`?
2. [sharpen] Does “canonical source” describe current consumption or target
   ownership?
3. [scenario] What happens when a CLI-generated guest and the host use
   different copies of `camel-plugin.wit`?
4. [cross-ref] Does `camel-wit` own runtime resource limits because it exports
   `WitHost`?

**Answers:**

1. [glossary] No. `CONTEXT-MAP.md:136` defines the WASM sandbox capability
   posture, while this context defines the cross-language ABI contract. The
   terms identify different boundaries.
2. [sharpen] It is target ownership. `camel-cli` consumes `PLUGIN_WIT` and
   `BEAN_WIT`, but `camel-component-wasm` reads its local `wit/` copies
   (`camel-wit` audit I2; bd `rc-osj0`). This file states both facts.
3. [scenario] The guest compiles against one function or type shape and the
   host links another. Component instantiation then fails instead of producing
   an Exchange. The file-diff test in `src/lib.rs::test_host_wit_matches_canonical`
   does not prevent this when it skips or does not run.
4. [cross-ref] No. Wasmtime resource and lifecycle limits live in
   `camel-component-wasm` (`crates/components/camel-component-wasm/CONTEXT.md:50`).
   `WitHost` has no external callers and alone causes the `camel-api`
   dependency (`camel-wit` audit I1; bd `rc-m9nn`).

**Outcome:** confirm. `camel-wit` needs a crate context because it defines a
public cross-language contract. The context separates contract ownership from
current host duplication and dead runtime code.
**Self-grill mode:** self-grill-proposals skill
