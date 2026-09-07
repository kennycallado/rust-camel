# Design: scenario-harness-ergonomics

## Approach

Ten pilot follow-ups, four delivery phases ordered by risk and fence constraints.

**Phase 1 — lanes and taxonomy.** `ClientLane::launch` (adapters/http.rs:523, insert at :548) replaces the single `LaneEntry` per key with a bounded per-key FIFO (`VecDeque<LaneEntry>`, capacity 64). Receives pop oldest-first, preserving wire order; generation guards stay per entry; `failed_send_does_not_poison_later_receive` semantics are preserved by failing only the overflowed launch. FIFO overflow maps to `TransportError` → apparatus `ActionTransport`, never a silent overwrite. `enqueue_arrival` (http.rs:827-861) records a per-lane dropped counter when `try_send` fails; `await_arrival` converts a receive timeout on an overflowed lane into a new apparatus-class `ScenarioFailure::ArrivalLaneOverflow` (plus an `is_apparatus` arm in camel-cli commands/test/scenario.rs:57-63). Inline routes and provisioning-without-bound-authority move to load-time `DocError` gates, copying the reserved-provisioning precedent (document.rs:513-523); the boot_scenario inline rejection stays as defense-in-depth.

**Phase 2 — boot and env.** camel-cli commands/test.rs:392-396 resolves the scenario boot root through the lean walk `find_camel_toml_root` (commands/test/runner.rs:132-137); no root → a named exit-2 error. `boot_scenario` gains the document directory as a second anchor: sealed config and `routeFilesFromRoot` follow the root, relative `routeFiles` stay document-anchored (today the two coincide; the split only widens for nested documents). camel-dsl discovery interpolation (env_interpolation.rs) becomes a parse-tree walk: parse the raw YAML, apply the existing per-string interpolation (same grammar, same `$${…}`/`$$` escapes, over string keys and leaves) recursively, re-serialize, then hand off to `parse_yaml`; a parse/serialize failure falls back to today's raw splice. The pre-pass uses the same serde_yaml settings as `parse_yaml`, so the fallback fires only for text the real parse would reject. Comments and structure are never interpolated; block-scalar content interpolates as a string value, matching raw-splice behavior. Only placeholder-bearing scalars are rewritten; an interpolated leaf that resolves to numeric-looking text keeps string typing (the camel-config leaf-interpolation precedent — a documented, deliberate narrowing of raw-splice re-parse typing). camel-config already interpolates leaves this way — parity, not novelty. `sendDeadline` (optional top-level humantime, default 30 s) replaces the fixed constant's use site (runner.rs:61).

**Phase 3 — vocabulary and docs.** `expectReply` on a `direct:` send parses into `camel_matchers::Expectation` (same verbs as `validate`); `FakeAdapter` records sends and produces no synchronous reply, so `expectReply` is valid on `direct:` sends only — on any other target it is a load error. `DirectStimulus::send` (adapters.rs:777-825) returns the parked reply instead of discarding it; the runner evaluates the expectation against the reply body through a feature-free bytes→Value helper (the partner-body extractors stay `http`-gated). Mismatch → verdict-class `ValidationMismatch`. `expectReply` on an `http` partner send → load error. The burst-send recipe lands in the camel-integration-test README plus a pointer in docs/src/testing/index.md, citing `partner_verification_test.rs::immediate_count_assert_e2e` (the verified canonical shape) and `http_partner_scripting_test.rs` (the bd issue's reference).

**Phase 4 — inbound bound-address (rc-5yon, last).** Harness-side staging per ADR-0070: a doc-level `inbound:` declaration (top-level section, no collision with `partners:` or `provisioning:`) carrying a `bindVar` binds 127.0.0.1:0, stages the listener (camel-component-http `stage_listener`, keyed by the actual bound address), and exposes `http://<bound>` as a bindVar; route URIs interpolate the var so consumers consume the staged entry — no port-0 URI grammar. Amendment (task 4.1 review): provisioning lives INSIDE the itest boot path — `boot_scenario` provisions when `doc.inbound` is set and extends a cloned `LayeredEnv`'s harness-provisioned layer with the full-URL variable before discovery, so route-file interpolation resolves with zero CLI orchestration; the bound address reaches the run outcome's `inbound_bound` through the itest run flow. The fenced `commands/test/**` subtree receives ZERO inbound edits (the Phase 1 `is_apparatus` arm remains the sole edit). Route files interpolate the var through the dsl grammar (`${env:INBOUND}/in`-style URIs with a full-URL variable value, matching the partner bindVar precedent). The ServerRegistry is verified reachable pre-boot (`global()` OnceLock + pub `stage_listener`); if the itest-internal plumbing proves impossible, the pre-approved escalation is the human-coordinated fence exception — never a silent fence breach. Fixed-port documents stay valid.

## Affected crates

- camel-integration-test: lane FIFO, overflow taxonomy, load-time gates, `sendDeadline`, `expectReply`, inbound staging, README recipe.
- camel-cli: scenario root resolution, `is_apparatus` arm, exit mapping.
- camel-dsl: tree-walk interpolation (env_interpolation.rs, discovery.rs).
- camel-matchers: none (consumed only).
- camel-component-http / camel-test: Phase 4 only, and only if the itest-internal plumbing fails (human-coordinated exception).

## Architecture boundaries

Data/control plane untouched — this is test-harness surface. ADR-0069 §7 drives every class correction; ADR-0072 rules matcher reuse (verbs from camel-matchers; grammar and observation stay scenario-tier); `expectReply` is covered by ADR-0069 §2's existing vocabulary split — no ADR edit in scope; ADR-0070 governs staged listener reuse; ADR-0069 §4 layered env precedence is untouched (the interpolation fix is leaf-level). ADR-0069 §6 forbids virtual time — `sendDeadline` is real-time only. Fence record: the sole edit under `crates/camel-cli/src/commands/test/**` is the `is_apparatus` match arm for `ArrivalLaneOverflow` (rc-7mli exit-2 mapping), pre-approved with the wave; that hunk is coordinated with pyramid step 2. Root resolution lands in `commands/test.rs`, outside the fenced subtree. Post-gate record (quality-gate loopback, commit 3bc7b749): a second, strictly mechanical edit under the fenced subtree — boxing `ScenarioDocument` in `ParsedDocument` (document.rs:750) and `LoadedDoc` (test.rs:270) — was forced by `clippy -D warnings` (`large_enum_variant`) after the new fields grew the struct; it mirrors the enum's own existing `Unit(Box<TestDocument>)` precedent, changes no logic, and is reported to the human at the merge gate.

## Phases

### Phase 1: Lane FIFO, overflow taxonomy, load-time gates
- **Goal:** silent and misclassed failures become correct-class errors.
- **Dependencies:** none.
- **Externally-visible types/interfaces:** `ScenarioFailure::ArrivalLaneOverflow`; new load-time `DocError` variants; ClientLane FIFO semantics.
- **Deliverable:** commits with unit + e2e tests per new scenario.
- **Exit-criteria:** new taxonomy scenarios green; `http_client_lane_test.rs` green; the burst-send e2e (3 sends, 0 receives) still passes and now parks all three responses.

### Phase 2: Boot root, comment-safe interpolation, sendDeadline
- **Goal:** nested trees boot; comments never fail loads; sends bound per document.
- **Dependencies:** Phase 1 (shared runner touchpoints).
- **Externally-visible types/interfaces:** `sendDeadline` document field; ancestor root resolution.
- **Deliverable:** commits with dsl/boot tests.
- **Exit-criteria:** nested-document e2e boots; a comment-placeholder fixture loads with the variable unset; dsl discovery tests green (`camel run` parity); a hung send fails at the document deadline.

### Phase 3: expectReply vocabulary and burst-send recipe
- **Goal:** direct replies assertable; concurrency recipe canonical.
- **Dependencies:** Phase 1 FIFO (reply parking order).
- **Externally-visible types/interfaces:** `expectReply` grammar on sends.
- **Deliverable:** commits with tests; README recipe + guide pointer.
- **Exit-criteria:** new requirement scenarios green; itest suites pass with and without `--features http`.

### Phase 4: Inbound bound-address (rc-5yon)
- **Goal:** inbound documents without pinned ports.
- **Dependencies:** Phases 1–3; human-coordinated rebase against pyramid step 2.
- **Externally-visible types/interfaces:** inbound declaration + bindVar.
- **Deliverable:** commits; `http_inbound_test.rs` migrated off `CONSUMER_PORT`.
- **Exit-criteria:** inbound e2e runs on an ephemeral port; no fixed ports in itest inbound tests; `PORT_GUARD` removed or justified.

## Alternatives considered

- Comment-stripping before raw interpolation: rejected — distinguishing block-scalar content from comments requires parsing anyway.
- Per-action send deadline: deferred — the document-level bound is the cheap fail-fast fix; per-action grammar can grow later without migration.
- Unbounded lane FIFO: rejected — unbounded queues hide apparatus defects (the rc-7mli class).
- Port-0 route URI grammar: rejected — interpolated bindVars over staged listeners reuse the ADR-0070 machinery with no new URI semantics.
