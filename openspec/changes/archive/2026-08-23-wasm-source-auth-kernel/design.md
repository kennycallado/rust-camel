# Design: wasm-source-auth-kernel

## Approach

Adopt the e_opus consultation recommendation (B + C, gRPC boundary-auth model;
reject A — full reasoning in `docs/consultations/rc-f79u-wasm-source-auth.md`):

**Kernel handshake (B), Gen-B placement.** The wasm source is
boundary-authenticated like ws/grpc: the raw request exists only in the axum
handler. Mirror `GrpcKernelAuth`: new `WasmSourceKernelAuth { plan,
providers }` with `from_security_context` (returns `None` unless both are
present — fail-closed). Implement `Consumer::set_security_context` on
`WasmSourceConsumer` (today it drops the no-op default; the controller already
builds and passes the `SecurityContext` at `route_controller_trait.rs:305-321`).
The consumer captures TWO independent facts from any context it receives:
the classification plan `plan: Option<RouteSecurityPlan>` (a full clone of
`ctx.plan` — used for the bind-gate snapshot and to derive `plan_access`)
and `kernel` (plan + providers, mint-capable). The per-request decision
table in `run_http_listener`'s handler (driven by the derived
`plan_access`, never by kernel presence alone):
- `plan_access = None` (no context ever set — raw construction) →
  pass-through; the bind gate at `start()` still governs exposure.
- `plan_access = Some(Public)` → pass-through (no extraction).
- `plan_access = Some(non-Public)` + kernel present → handshake:
  `extract_token_multi` over headers/cookie/query →
  `camel_auth::kernel_authenticate` → deny renders 401 and returns BEFORE
  `request_tx.send` (channel untouched, guest never woken; the
  202-immediate-ack path for accepted requests is unchanged); success threads
  the minted principal.
- `plan_access = Some(non-Public)` + kernel missing (plan-only context —
  wiring incomplete) → 401: absent wiring never yields pass-through for
  non-Public plans (fail-closed).

**Classification delivery.** For staged routes the classification must reach
the consumer even when the route declares no security (or the DSL marker
carries no policy config): `SecurityContext.policy` becomes
`Option<Arc<dyn SecurityPolicy>>` (pre-1.0 breaking, mechanical
compatibility sweep — every existing construction path keeps `Some`), a new
`SecurityContext::from_plan(plan)` builds plan-only contexts, and the
controller additionally injects a plan-only context at both injection sites
(`route_controller_trait.rs:303` and `:710`) whenever
`managed.compiled.security_plan` exists and the sp_config path did not run.
Other transports are unaffected: `from_security_context` still requires
plan+providers, and a `Public` plan is pass-through everywhere.

**Carrier threading (the one non-obvious risk).** `install_carrier` needs
`&mut Exchange`, but the Exchange is minted later in `submit_exchange`
(`source_host.rs:272`) — and `submit_exchange` receives only the guest's
`WasmExchange`; the `HttpMeta` never reaches it directly. Threading: the
handler sets a private `principal` field on `HttpMeta`; `accept_http` moves
it into a `pending_principal` slot on `SourceHostState` guarded by a
one-outstanding-request invariant (a second `accept-http` while a request is
outstanding fails closed — no identity overwrite); `submit_exchange` takes
the slot, clears the marker, and calls `install_carrier` right after
`source_exchange_to_native`, before `exchange_tx.send`. The guest never sees
the principal (no WIT change).

**Bind governance (C).** Operator URI `?bind=` (route config, trusted) is
authoritative. The guest reveals `listener_spec.bind` only inside
`Consumer::start` (guest `configure()`), so a conflict between the two is a
consumer-startup error raised after the guest reveals its spec and BEFORE
`TcpListener::bind`, naming both addresses (ADR-0061 Rule-9 style: hard
error, never silent — no silent override in either direction). Matching
operator/guest binds (equal after normalization) bind exactly one listener.
With no operator bind, the guest bind is used but gated. Before
`TcpListener::bind`, `start()` runs `camel_auth`
`enforce_bind_exposure_gate` on the resolved bind with the compiled plan
snapshot and the operator's `BindExposureAcks` (wired from the CLI as mcp
does). Loopback passes silently; non-loopback `Public` without ack fails
`start()`.

**Transport enumeration.** Plan compilation and credential capability are
keyed by transport. Adding `wasm:` means extending the transport
discriminant (`TransportId`-style enum in `camel-api`, exhaustive by
contract) and updating the exhaustive matches it forces in `camel-core`
(plan compilation classification) and the WASM policy code
(credential capability), plus their match tests. This is
mechanical but cross-crate; it lands first (Phase 1) so plan compilation can
classify `wasm:` routes.

## Affected crates

- `camel-component-wasm`: `source_consumer.rs` (kernel field,
  `set_security_context`, bind resolution + gate), `source_host.rs`
  (`WasmSourceKernelAuth`, handshake in handler, `HttpMeta.principal`,
  `install_carrier` in `submit_exchange`), tests for handshake/denial/bind.
- `camel-api`: transport discriminant gains the `wasm:` variant
  (exhaustive-by-contract enum).
- `camel-core`: exhaustive-match arms for the new variant (plan compilation
  classification).
- `camel-cli`: wire `BindExposureAcks` to the wasm source consumer (as mcp).
- `camel-auth`: unchanged; reuse `kernel_authenticate`,
  `install_carrier`, `extract_token_multi`, `enforce_bind_exposure_gate`,
  `BindExposureAcks` as-is.
- Docs: ADR-0061 amendment, crate `CONTEXT.md`, `CONTEXT-MAP.md`,
  `examples/wasm-source-webhook`.

## Architecture boundaries

Components (wasm) may not depend on camel-core or other components
(`lint-component-deps`, ADR-0055); `camel-auth` is already a dependency, so
the kernel path adds no edge. ADR-0031's guest-owned `run(listener)` loop is
preserved: the host authenticates at the TCP edge and forwards only
authenticated requests into the existing `request_tx` channel the guest
drains. The WIT surface is unchanged.

## Phases

### Phase 1: Security wiring + transport enumeration + bind governance
- **Goal:** the consumer captures its `SecurityContext` (plan + providers),
  `wasm:` classifies in plan compilation, and the resolved listener bind
  runs the per-bind exposure gate.
- **Dependencies:** existing `bind_gate.rs`, `BindExposureAcks` CLI wiring
  (mcp precedent).
- **Externally-visible types/interfaces:** `camel-api` transport enum gains
  the `wasm:` variant; no new config keys (reuses
  `[binds."<addr>"].allow_public_exposure`).
- **Deliverable:** `set_security_context` + `WasmSourceKernelAuth` capture;
  transport enum extension with exhaustive matches; gate enforcement in
  `start()` + CLI ack wiring + tests.
- **Exit-criteria:** consumer holds the compiled plan; non-loopback `Public`
  bind without ack fails `start()` with a bind-naming error; loopback
  passes; operator/guest bind conflict fails at consumer startup before the
  socket is bound; `parse_guest_config`-forwarded `bind=` feeds the same
  gate.

### Phase 2: Kernel handshake + carrier
- **Goal:** host-edge authentication and typed carrier for wasm-source routes.
- **Dependencies:** Phase 1 (plan capture and gate run with compiled plans).
- **Externally-visible types/interfaces:** none new (internal
  `WasmSourceKernelAuth` handshake step).
- **Deliverable:** handshake in `run_http_listener`, principal threading to
  `submit_exchange`, denial tests per credential source, substitution test.
- **Exit-criteria:** missing/invalid credential on `Authenticated` route →
  401, guest never sees the request, body never polled; valid credential via
  each permitted source → pipeline Exchange with readable carrier;
  provider-A token fails a provider-B route.

### Phase 3: Docs + example convergence
- **Goal:** posture docs match the new reality; example is legal under the
  gate.
- **Dependencies:** Phases 1-2.
- **Externally-visible types/interfaces:** ADR-0061 amendment text.
- **Deliverable:** ADR-0061 amendment, `CONTEXT.md` inbound-request posture,
  `CONTEXT-MAP.md` updates, webhook example fixed (loopback bind or declared
  `security_policy` + ack example).
- **Exit-criteria:** `nix shell nixpkgs#mdbook -c mdbook build docs` exits 0;
  two-source rule satisfied; example starts under the new gate.

## Alternatives considered

- **A — route the listener through camel-http's ServerRegistry + kernel:**
  rejected. Adds a component→component dependency (ADR-0055 cycle risk),
  inverts ADR-0031's guest-owned listener loop, re-plumbs two components for
  no security benefit the shared kernel does not already provide.
- **C alone (loopback-default + ack, no handshake):** rejected as incomplete.
  It governs exposure but leaves `Authenticated` unimplementable — the
  semantic gap stays open. Subsumed into B as Phase 1.
- **New ADR:** rejected. ADR-0061 already scopes kernel convergence across
  transports; the wasm source is an amendment (5th transport), not a new
  decision.
