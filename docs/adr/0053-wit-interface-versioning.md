# ADR-0053: WIT Interface Versioning

**Date:** 2026-08-07
**Status:** Accepted; implementation pending (`rc-aaxe`)
**Amends:** none
**Related:** ADR-0014, ADR-0031, ADR-0049, ADR-0050
**Origin:** `camel-wit` quality audit, `WIT-006` / DP-2

## Context

`camel-wit` defines the component-model ABI between rust-camel and WASM
guests. Its four WIT files currently declare an unversioned
`package camel:plugin;`. The same unresolved `WIT-006` note appears in those
files and in `src/lib.rs`. `rg -n 'TODO\(WIT-006\)' crates/camel-wit` verifies
five sites.

Compiled guests depend on package, interface, world, type, and function
identities. A Rust crate release can change without changing that ABI. A WIT
shape change can also break an existing guest even when the Rust API remains
compatible. The Rust crate version and ADR-0049's `#[non_exhaustive]` policy
therefore cannot express WIT compatibility.

Adding a version after stable guests exist changes package identity and forces
a migration without a prior compatibility contract. WIT versioning is thus a
v1.0 freeze decision, not post-v1.0 documentation work.

## Decision

The `camel:plugin` WIT package uses an independent package-level SemVer.

1. One version covers every interface and world in the package. We do not
   version `plugin`, `bean`, `authorization-policy`, or `source` separately.
2. The rust-camel v1.0 release establishes `camel:plugin@1.0.0`. Pre-v1
   unversioned packages have no compatibility guarantee.
3. The WIT version does not follow the Rust workspace version. A Rust release
   that does not change the WIT contract keeps the existing WIT version.
4. A change to an existing function, record, variant, enum, resource, import,
   or export is breaking unless the supported component toolchain proves it
   compatible in both host and guest directions. Breaking changes increment
   the WIT major version.
5. A proven compatible contract addition increments the minor version.
   Documentation-only corrections increment the patch version only when a WIT
   package release needs a distinct identity.
6. `@since` annotations record the minor version that introduced an element
   when the supported toolchain can validate them. They supplement the package
   version and do not replace it.
7. The host links only package majors that it explicitly supports. It must not
   silently reinterpret a guest from another major. Supporting two majors
   requires separate bindings and an explicit migration period.
8. Canonical WIT files, generated host bindings, shipped guest examples, and
   compatibility tests change in one code change. `rc-aaxe` tracks the initial
   `1.0.0` application. `rc-osj0` tracks removal of the host's duplicate WIT
   source.

## Consequences

- Package identity detects incompatible guest and host contracts during
  linking instead of allowing ambiguous runtime behavior.
- WIT evolution can remain stable across unrelated Rust crate releases.
- The package-wide version keeps shared `types` and `host` interfaces coherent
  across all worlds.
- A post-v1 breaking ABI change requires a new package major and host bindings.
  This cost is deliberate because silently replacing the ABI would break
  compiled guests.
- The initial implementation changes package identities in canonical files,
  host bindings, copied files, and examples. It must land before the v1.0
  freeze.

## Options considered

### Defer versioning until after v1.0

Rejected. Adding the first package version after stable guests exist is itself
a package-identity break. Deferral would freeze ambiguity into the v1 contract.

### Follow every Rust crate version

Rejected. Most Rust releases do not change the WIT ABI. Lockstep versions would
signal false incompatibility and couple guest tooling to unrelated Rust work.

### Version each world independently

Rejected. The worlds share package-level `types` and `host` interfaces.
Independent versions would either duplicate those interfaces or create a
compatibility matrix without a present use case.

### Use one independent package version

Accepted. It matches the actual compatibility boundary and permits all worlds
to evolve as one contract while remaining independent from Rust releases.

## Why this is not an amendment

ADR-0014 governs runtime limits and configuration. ADR-0031 defines source
world lifecycle. ADR-0049 governs Rust enum evolution. ADR-0050 governs sandbox
capabilities. None defines ABI identity or compatibility across WIT releases.
This decision is orthogonal and applies to all WASM worlds, so it needs its own
ADR.

## Self-grill record

**Questions generated:**

1. [glossary] Does “WIT package version” overlap Rust crate SemVer or the WASM
   sandbox capability posture?
2. [sharpen] Is the compatibility unit one package, one interface, or one
   world?
3. [scenario] What happens if `wasm-exchange` gains a field after v1.0 while
   the package remains `camel:plugin@1.x`?
4. [cross-ref] Can an existing ADR own this decision, or can versioning wait
   until after the v1.0 release?

**Answers:**

1. [glossary] It is separate from both. The workspace crate version is
   `0.26.0` (`Cargo.toml:54`), while WIT files are unversioned
   (`crates/camel-wit/wit/camel-plugin.wit:1`). ADR-0050 controls granted host
   capabilities, not package compatibility.
2. [sharpen] The package is the compatibility unit. `plugin`, `bean`, and
   `source` share `camel:plugin` interfaces and types
   (`camel-plugin.wit:8-94`, `camel-bean.wit:12-18`,
   `camel-source.wit:9-82`). Per-world versions would split shared types.
3. [scenario] Existing guests compiled against the old record shape can fail
   to link or lower/lift values correctly. Under this decision, that shape
   change defaults to a major bump unless compatibility tooling proves both
   directions safe. It cannot pass as an undocumented additive Rust change.
4. [cross-ref] No existing ADR governs WIT evolution. ADR-0049 explicitly
   covers Rust contract enums, and ADR-0050 covers the host capability surface.
   Deferral fails because adding `@1.0.0` later changes the identity consumed by
   the four Wasmtime `bindgen!` sites in `camel-component-wasm/src/`.

**Outcome:** confirm as new ADR. The decision is hard to reverse after guests
compile, surprising without the Rust/WIT version distinction, and resolves a
real trade-off between lockstep, per-world, and package-wide versioning.
**Self-grill mode:** self-grill-proposals skill
