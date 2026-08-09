# ADR-0050: WASM sandbox capability posture

**Date:** 2026-08-06
**Status:** Accepted; implementation pending
**Decision:** Option B, selective WASI registration per world
**References:** ADR-0011, ADR-0014, ADR-0031, ADR-0032, ADR-0033
**Origin:** audit of `camel-component-wasm`, findings `F-camel-component-wasm-I1` and `F-camel-component-wasm-I2`

## Context

The WASM host exposes two capability surfaces. The first contains the Camel functions from `wit/camel-plugin.wit`. The second contains the WASI 0.2 interfaces that Wasmtime registers.

`WasmCapabilities` controls the first surface. The `camel_call` and `camel_poll` calls use an allowed-scheme list. An empty list denies all schemes. Policy worlds use `WasmCapabilities::denied()`. Processor and bean worlds enable the host store explicitly.

The second surface does not follow that posture. The current code calls `wasmtime_wasi::p2::add_to_linker_async` in all four worlds. The WASI context grants no preopens, environment variables, socket ports, or name resolution. However, the linker advertises the full WASI surface. In addition, the processor, bean, and policy worlds inherit stderr. The source world does not. This difference does not reflect a security policy.

The trust model accepts plugins installed by the operator. The sandbox limits guest defects. However, a capability the host does not need must not appear in the linker. A Wasmtime upgrade or a change to `WasiCtxBuilder` must not extend capabilities by accident.

## Decision

We adopt **Option B: selective WASI registration per world**.

The host applies these rules:

1. Each world has an explicit list of WASI interfaces.
2. All four worlds may register `wasi:clocks` and `wasi:random` when their components import them.
3. No world registers filesystem, sockets, CLI, environment, or stdio by default.
4. The source world keeps its `http-listener` interface. This interface grants no general socket access.
5. Worlds that have Camel functions use `camel_call` for logging. The host does not call `inherit_stderr()`.
6. A new WASI interface requires a per-world grant, a negative test for the other worlds, and documentation in the crate context.

The Camel-function posture follows the same principle. The `camel_call` and `camel_poll` schemes use an allowlist. Policy worlds receive no call or store operations. Processor and bean grants stay explicit in `WasmCapabilities`.

This decision describes the target state. The current code still registers full WASI and keeps the unequal stderr inheritance. The audit findings cover that migration in the code stream.

## Consequences

### Positive

- The linker and the context express the same capability policy.
- Filesystem, sockets, and environment variables do not depend on Wasmtime defaults to stay denied.
- Each future extension leaves a reviewable per-world grant.
- Policy worlds keep a smaller surface than processor and bean worlds.

### Negative

- Selective registration couples the host to submodule APIs of `wasmtime-wasi`.
- Wasmtime updates may require changes across several registrars.
- Guests that use `eprintln!` stop working until they migrate to the Camel logging channel. Source guests will have no stderr output.

### Neutral

- The memory, instance, table, and epoch limits from ADR-0014 do not change.
- The `http-listener` interface of the source world stays under ADR-0031.
- Operator configuration stays trusted. Exchange data stays untrusted per ADR-0032.

## Options considered

### Option A: full WASI with denial in the context

Rejected. It has lower immediate cost, but the linker advertises capabilities the host does not intend to grant. Security depends on defaults and on no future change extending the context.

### Option B: selective registration per world

Chosen. It keeps compatibility with clocks and random, and removes interfaces guests do not need. The Wasmtime integration cost is acceptable for a verifiable surface.

### Option C: remove WASI

Rejected. It is the minimum surface, but it breaks guests compiled with common clocks or random imports. Option B captures most of the benefit without that broad incompatibility.

## Relation to other decisions

ADR-0014 unifies configuration and resource limits for the WASM runtime. It does not define which interfaces a guest may import. This ADR decides a different class: the sandbox capability surface. It therefore does not amend ADR-0014.

ADR-0031 defines the source-world lifecycle and its `http-listener` resource. ADR-0032 defines the trust direction of Exchange data. ADR-0033 requires safe defaults and specific grants. This decision applies those rules to the WASI linker.

## Self-grill record

1. **Glossary:** "WASM sandbox capability posture" does not replace Component, Endpoint, or SecurityPolicy. It names the union of two surfaces: Camel functions and WASI. `CONTEXT-MAP.md` records the cross-cutting term.
2. **Precision:** The decision does not claim all Camel functions are denied by default. `from_scheme_list()` enables the store for processor and bean. The empty list denies only call schemes.
3. **Scenario:** A guest that imports filesystem will fail to instantiate. That failure is intentional. A guest that imports only clocks and random keeps compatibility.
4. **Code:** `runtime.rs`, `wasm_plugin_context.rs`, and `source_host.rs` still call `add_to_linker_async`. `runtime.rs` still uses `inherit_stderr()`. The ADR therefore declares a target state. It does not describe the current code as already conformant.

**Outcome:** approve Option B as a workspace-wide decision. The decision is costly to reverse, surprising without context, and resolves a real trade-off.
**Mode:** `self-grill-proposals`.
