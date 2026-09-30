# Tasks: strictmode

## camel-cli

### Task 1.1: Strict directive in the truststore codec

**Files:**
- `crates/camel-cli/src/compile/trust.rs` (modified)

**Steps:**
1. Add a `strict: bool` field to `TrustStore` (parsed state, beside `entries` and `lines`).
2. Add accessor `pub(crate) fn is_strict(&self) -> bool` returning the field.
3. In `TrustStore::parse`, inside the per-line loop: after the blank/comment skip, tokenize with the existing `raw.split_whitespace()`; when the token set is exactly `["strict"]`, set `strict = true` and `continue` to the next line (idempotent across repeats — no duplicate error, unlike pins). Do NOT treat `strict` as a fingerprint: it fails `is_pinned_form` anyway, so order the directive match BEFORE the fingerprint/floor match arm so a bare `strict` line never reaches `MalformedEntry`.
4. A line tokenizing to `["strict", <anything>]` keeps falling through to the existing two-token rule: the first token `strict` fails `is_pinned_form` (no `blake3:` prefix), so the existing arm already rejects it as `MalformedEntry` — make no code change here, only keep the ordering (directive match on the exact one-token set, so a bare `strict` never reaches the fingerprint path) and prove the rejection with the test below.
5. Extend the module doc comment (lines 1–19) with the directive: one sentence on the `strict` token, its idempotence, and its verify-side-only semantics.
6. Keep the verbatim `lines` retention untouched (directive lines survive floor rewrites like comments — no code change needed, only the test proving it).

**Tests:** (add inside the existing `#[cfg(test)]` module of `trust.rs`, mirroring `HEX_A`/`HEX_B` constants and the `write_store` helper already used there)
- `strict_directive_parses_and_flags_store`: a store file containing `HEX_A` pin line + `strict` line + comment + blank line → `TrustStore::parse` Ok, `is_strict() == true`, one entry pinned (`is_pinned(&format!("blake3:{HEX_A}"))`). `command`: `systemd-run --user --scope --collect --unit=fleet-strictmode -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env CARGO_BUILD_JOBS=6 cargo test -j4 -p camel-cli --lib compile::trust::tests::strict_directive_parses_and_flags_store` (lib target; before implementation `is_strict` does not exist → compile error = RED).
- `strict_directive_idempotent_and_whitespace_tolerated`: store body `"# note\n\n strict \nstrict\nblake3:{HEX_B}\n"` (comment, blank line, whitespace-padded `strict`, bare `strict`, one pin) → parse Ok, `is_strict() == true`, exactly one pin entry, `lines` retains all five lines verbatim (directive lines survive floor rewrites like comments). `command`: `systemd-run --user --scope --collect --unit=fleet-strictmode -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env CARGO_BUILD_JOBS=6 cargo test -j4 -p camel-cli --lib compile::trust::tests::strict_directive_idempotent_and_whitespace_tolerated`.
- `strict_with_second_token_is_malformed`: store body `"strict 42\n"` → `TrustStore::parse` Err `MalformedEntry` naming path and line 1; the Display message starts with `truststore-parse` and contains `strict 42`'s line number. Also assert a two-token line `"strict blake3:<HEX_A>"` is `MalformedEntry`. `command`: `systemd-run --user --scope --collect --unit=fleet-strictmode -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env CARGO_BUILD_JOBS=6 cargo test -j4 -p camel-cli --lib compile::trust::tests::strict_with_second_token_is_malformed`.
- `strict_absent_by_default`: store body with only a `HEX_A` pin → `is_strict() == false` (back-compat default). `command`: `systemd-run --user --scope --collect --unit=fleet-strictmode -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env CARGO_BUILD_JOBS=6 cargo test -j4 -p camel-cli --lib compile::trust::tests::strict_absent_by_default`.
- Existing trust.rs unit tests stay green unchanged (regression: `systemd-run --user --scope --collect --unit=fleet-strictmode -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env CARGO_BUILD_JOBS=6 cargo test -j4 -p camel-cli --lib compile::trust` all pass).

**Acceptance:**
- `systemd-run --user --scope --collect --unit=fleet-strictmode -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env CARGO_BUILD_JOBS=6 cargo test -j4 -p camel-cli --lib compile::trust` passes with the four new tests.
- `cargo fmt --check` clean; `cargo clippy -p camel-cli -- -D warnings` clean.
- No behavior change reachable from existing call sites (`is_strict` is a new read-only accessor; parse of directive-free stores produces identical state).

- [x] 1.1

### Task 1.2: Strict rejection policy at the shared trust hook

**Files:**
- `crates/camel-cli/src/compile/runtime.rs` (modified)
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. Add variant `StrictUnsigned` to `TrustRejection` with doc comment `/// `strict-unsigned``.
2. In `trust_policy_rejection`, change the signature to take `exe: &Path` in addition to the existing `sig: &Path`. The function has SEVEN call sites: four production (the envelope-missing and envelope-present branches of both `run_verify_only` and `verify_for_boot` — all have `exe` in scope; keep each call one-line) plus three in runtime.rs's own `#[cfg(test)]` unit tests (the `schema4_boundary_is_the_first_recorded_floor` and `pin_removal_under_lock_fails_closed` groups) — update ALL seven, passing any artifact-looking path (the unit tests assert policy decisions, not the diagnostic's artifact path). Replace the `manifest.signing`-absent early-return — the `let Some(signing) = &manifest.signing else` branch that currently logs nothing and returns `None` — so that: when no signing block is present AND a truststore path is supplied, the branch calls `trust::TrustStore::parse(path)`:
   - `Err(e)` → `eprintln!("{e}")` (the `TrustError` Display already emits the `truststore-parse` step token and names the path) and return `Some(TrustRejection::Parse)` — fail closed for unsigned artifacts under an unreadable/malformed store.
   - `Ok(store)` → if `store.is_strict()`, `eprintln!("strict-unsigned: the artifact at {} carries no signing block (schema {}); the truststore at {} is strict", exe.display(), manifest.manifest_schema, path.display())` and return `Some(TrustRejection::StrictUnsigned)`. Otherwise return `None` exactly as today.
   - The `manifest.signing`-present paths (strip rule, pin, freshness) are untouched: signed manifests keep `truststore-pin`.
3. Update the `trust_policy_rejection` doc comment: the unsigned branch now reads the strict directive; well-formed non-strict stores leave the unsigned path unchanged; malformed stores fail closed for unsigned artifacts.
4. Note the parse cost: unsigned + supplied store adds exactly one store read; no truststore → still zero reads (the `truststore?` guard stays first).
5. Add integration tests to `compiled_artifact_test.rs` in the keypin truststore section (reuse `deploy_artifact`, `deploy_signed`, `write_truststore`, `manifest_fingerprint`, `manifest_marker`, `common::run_binary`, `OTHER_PIN_HEX`, `child_guard` — all existing helpers):
   - `strict_store_rejects_unsigned_at_boot_and_verify`: deploy `fixture().job` (unsigned schema-3); truststore body `"<fingerprint of signed_job>\nstrict\n"` (pin + directive); (a) `run_binary(deploy, artifact, ["--truststore", ts], [])` → code 2, combined output contains `strict-unsigned`, does NOT contain `context started`; (b) `run_binary(deploy, artifact, ["--verify", "--truststore", ts], [])` → code 2, contains `strict-unsigned`, stdout does NOT contain `key_fingerprint:`.
   - `strict_store_rejects_unsigned_through_env_dispatch_modes`: same unsigned artifact + strict store via `[("CAMEL_TRUSTSTORE", ts)]`: FIRST assert both principal surfaces under the env source — bare boot → code 2 + `strict-unsigned`, and `--verify` → code 2 + `strict-unsigned` with no `key_fingerprint:` line; THEN the three dispatch modes — `--manifest` → code 2 + `strict-unsigned`, `--help` → code 2 + `strict-unsigned`, `--version` → code 2 + `strict-unsigned`; and `["--truststore", ts, "--manifest"]` → code 2 with the argument-surface diagnostic (NOT `strict-unsigned`), proving the modifier stays rejected for exclusive modes while env policy still governs them.
   - `malformed_store_fails_closed_for_unsigned_artifact`: unsigned artifact; write a store file containing `"not-a-pin\n"`; boot with `--truststore` → code 2, contains `truststore-parse` and the line number `1`, no boot. Also an unreadable path (`--truststore /nonexistent/strict.keys`) → code 2, contains `truststore-parse` and the path.
   - `strict_store_still_boots_signed_pinned_artifact`: deploy `fixture().signed_job`; read its `manifest_marker`; truststore = `"<fingerprint> <marker>\nstrict\n"` (pin with a satisfied floor equal to the current marker, plus directive); boot → code 0; `--verify --truststore` → code 0 printing `key_fingerprint:` — strict adds the unsigned rejection and nothing else, and the freshness decision runs exactly as without the directive.
   - `strip_rule_keeps_precedence_under_strict_store`: deploy `fixture().signed_job`, delete `<artifact>.sig`; truststore = its fingerprint + `strict`; boot and `--verify` → both code 2 with `truststore-pin` (naming the fingerprint), NOT `strict-unsigned`.
   - `strict_store_rejects_v1_artifact_without_signing_block`: build the v1 artifact exactly as `v1_artifact_with_stray_envelope_fails_closed` does (`manifest::derive("app.yaml", TrailerKind::Route, ROUTE_DOC)` → `to_legacy_json` → `trailer::encode` appended to a copy of `env!("CARGO_BIN_EXE_camel")`), write NO `.sig`; boot with a strict `--truststore` → code 2, contains `strict-unsigned` — the decision keyed on the absent signing block, schema number irrelevant.
   - Existing `unsigned_artifact_ignores_truststore` test stays green unchanged (permissive default, no `truststore` token in output — a non-strict store still prints nothing for unsigned).
6. Tests must respect the suite conventions: `child_guard()` where the pattern requires it (copy the guard usage of the neighbor tests), `scrub_camel_env` is inside `run_binary` already — do not re-scrub.

**Tests:** (the six above; RED/GREEN expectations stated exactly: `strict_store_rejects_unsigned_at_boot_and_verify`, `strict_store_rejects_unsigned_through_env_dispatch_modes`, `malformed_store_fails_closed_for_unsigned_artifact`, and `strict_store_rejects_v1_artifact_without_signing_block` are RED after Task 1.1 alone — the directive parses but unsigned artifacts still boot (exit 0) or verify with the generic missing-envelope diagnostic — and GREEN after the runtime change. `strict_store_still_boots_signed_pinned_artifact` and `strip_rule_keeps_precedence_under_strict_store` are regression guards: they pass once Task 1.1 parses the directive (pin/freshness/strip decisions are untouched by the runtime change); their role is proving strict adds nothing for signed artifacts. Note `--verify` on an unsigned artifact ALREADY exits 2 today with a generic missing-envelope message — the RED property is the `strict-unsigned` diagnostic replacing it, not the exit code)
- `strict_store_rejects_unsigned_at_boot_and_verify`: unsigned schema-3 fixture + pin+`strict` store → boot exit 2 `strict-unsigned` no boot; `--verify` exit 2 `strict-unsigned` no fingerprint line. `command`: `systemd-run --user --scope --collect --unit=fleet-strictmode -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env CARGO_BUILD_JOBS=6 cargo test -j4 -p camel-cli --test compiled_artifact_test strict_store_rejects_unsigned_at_boot_and_verify`.
- `strict_store_rejects_unsigned_through_env_dispatch_modes`: env-supplied strict store + `--manifest`/`--help`/`--version` → exit 2 `strict-unsigned`; flag+`--manifest` → argument-surface diagnostic instead. `command`: `systemd-run --user --scope --collect --unit=fleet-strictmode -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env CARGO_BUILD_JOBS=6 cargo test -j4 -p camel-cli --test compiled_artifact_test strict_store_rejects_unsigned_through_env_dispatch_modes`.
- `malformed_store_fails_closed_for_unsigned_artifact`: malformed line and missing file → exit 2 `truststore-parse` naming line/path. `command`: `systemd-run --user --scope --collect --unit=fleet-strictmode -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env CARGO_BUILD_JOBS=6 cargo test -j4 -p camel-cli --test compiled_artifact_test malformed_store_fails_closed_for_unsigned_artifact`.
- `strict_store_still_boots_signed_pinned_artifact`: pinned+strict → boot 0, verify 0 with fingerprint line. `command`: `systemd-run --user --scope --collect --unit=fleet-strictmode -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env CARGO_BUILD_JOBS=6 cargo test -j4 -p camel-cli --test compiled_artifact_test strict_store_still_boots_signed_pinned_artifact`.
- `strip_rule_keeps_precedence_under_strict_store`: signed artifact minus `.sig` under strict store → `truststore-pin` both sites. `command`: `systemd-run --user --scope --collect --unit=fleet-strictmode -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env CARGO_BUILD_JOBS=6 cargo test -j4 -p camel-cli --test compiled_artifact_test strip_rule_keeps_precedence_under_strict_store`.
- `strict_store_rejects_v1_artifact_without_signing_block`: hand-built v1 artifact under strict store → exit 2 `strict-unsigned`. `command`: `systemd-run --user --scope --collect --unit=fleet-strictmode -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env CARGO_BUILD_JOBS=6 cargo test -j4 -p camel-cli --test compiled_artifact_test strict_store_rejects_v1_artifact_without_signing_block`.

**Acceptance:**
- All six new integration tests pass under the systemd-run wrapper; battery `systemd-run --user --scope --collect --unit=fleet-strictmode -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env CARGO_BUILD_JOBS=6 cargo test -j4 -p camel-cli --test compiled_artifact_test` passes with baseline 73 + 6 = 79 tests (0 failures; count printed by the run).
- `pgrep -c compiled_artifa` returns 0 before and after the battery (no leaked artifact processes).
- `cargo fmt --check` clean; `cargo clippy -p camel-cli --all-targets -- -D warnings` clean.

- [x] 1.2

### Task 1.3: Document the strict opt-in, amend ADR-0083, and keep context docs honest

**Files:**
- `docs/src/cli/compile.md` (modified)
- `docs/adr/0083-artifact-signing-envelope.md` (modified)
- `CONTEXT-MAP.md` (modified)
- `crates/camel-cli/CONTEXT.md` (modified)

**Steps:**
1. In `docs/src/cli/compile.md`, "Pinning a producer" section: extend the truststore-format sentence that begins "The file holds one pin per line" with the directive — a line whose whitespace-separated content is exactly `strict` sets the strict directive (idempotent, whitespace-tolerated, not a pin). Append one bullet to the "Under a supplied truststore" list: with the strict directive, an artifact with no signing block (any unsigned manifest) exits 2 with `strict-unsigned` at boot, `--verify`, and the env-governed `--manifest`/`--help`/`--version` dispatch; a store that cannot be parsed fails closed for unsigned artifacts too; the directive changes nothing about pin or freshness decisions for signed artifacts.
2. In `docs/adr/0083-artifact-signing-envelope.md`, append a dated sub-note (2026-09-30) under the existing Amendment section, three to six sentences: the strict directive closes the accepted unsigned boundary as an opt-in; the syntax; the fail-closed parse rule for unsigned artifacts; default permissive unchanged.
3. `CONTEXT-MAP.md`: (a) find the artifact-signing/truststore index entry that records "amended 2026-09-26 (keypin)" (~line 115) and extend it with "amended 2026-09-30 (strictmode): opt-in strict unsigned rejection"; (b) check the truststore glossary clause (~line 220) and, if it summarizes keypin behavior, extend it with the strict directive in the same style (one sentence).
4. `crates/camel-cli/CONTEXT.md`: runtime.rs holds the zone's size ceiling and records that the next growth of its verification block extracts `compile/verify.rs` first (deferral bd rc-5u0jx item (c)). Task 1.2 grows the verification block by ~20 lines. Do NOT extract `verify.rs` in this change (out of scope, P3); instead update CONTEXT.md honestly: the new runtime.rs line count, the stale "compile::trust owns the truststore pinning policy for signed artifacts" sentence (now: "for signed artifacts, plus the strict directive decision for unsigned ones"), and one sentence noting the deferral still holds with the strictmode increment recorded.
5. Verify docs cross-references still resolve: the compile.md "See also" list and any ADR reference numbers stay untouched.

**Tests:** (docs — verification is mechanical)
- `strict_optin_documented_in_compile_docs`: the "Pinning a producer" section of `docs/src/cli/compile.md` contains `strict-unsigned` and mentions the `strict` directive token. `command`: `grep -c "strict-unsigned" docs/src/cli/compile.md` → ≥ 1.
- `adr_amendment_note_present`: `docs/adr/0083-artifact-signing-envelope.md` gains a `2026-09-30` sub-note containing `strict`. `command`: `grep -c "2026-09-30" docs/adr/0083-artifact-signing-envelope.md` → ≥ 1.
- `context_map_mentions_strict_directive`: `CONTEXT-MAP.md` carries the amended entry and the directive mention. `command`: `grep -in "strict" CONTEXT-MAP.md` → a truststore-context hit (index entry or glossary clause).
- `camel_cli_context_reflects_strictmode`: `crates/camel-cli/CONTEXT.md` no longer claims trust policy is signed-artifacts-only. `command`: `grep -c "strict" crates/camel-cli/CONTEXT.md` → ≥ 1, and the recorded runtime.rs line count matches `wc -l crates/camel-cli/src/compile/runtime.rs`.

**Acceptance:**
- All four grep checks pass; the CONTEXT.md line-count claim equals the actual `wc -l` output.
- `cargo xtask lint-context-citations` exits 0 (CONTEXT-MAP.md and CONTEXT.md edits stay citation-clean).
- Docs prose is English (canonical language policy); no emoji.

- [x] 1.3
