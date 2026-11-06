# Proposal: deps-majors-minijinja-ed25519

## Why

bd rc-k93qw tracks four Dependabot majors. Mission 515 takes the two that were
assumed self-contained: minijinja 2.24 → 3.0.5 and ed25519-dalek 2.2 → 3.0.
Investigation at mission start shows **the API migrations already landed on
main** (mission 490, recorded in rc-k93qw notes): the workspace declares
`minijinja = "3"` (lock 3.0.5) and `ed25519-dalek = { version = "3", features
= ["hazmat", "digest"] }` (lock 3.0.0), zero `from_serialize` call sites
remain, and the ADR-0083 cross-version signing fixture
(`ed25519ph_envelope_dalek_2_2.hex`) plus strict-undefined semantics tests
already exist. What does **not** exist is a closed verification record for
this mission's acceptance criteria, and one ed25519 duplicate remains in the
lock that needs evidence and a written deferral.

## What Changes

1. **minijinja 3.0.5 — verify and close the coverage audit.** Enumerate every
   migrated call-site shape in `camel-language-minijinja/src/engine.rs` and
   map each to a named test in `camel-template` / `camel-language-minijinja`,
   persisted as a call-site/branch → test matrix in this change dir. Add
   tests for any uncovered shape — including a **new exact-diagnostic pin**:
   for canonical broken-template inputs (strict-undefined path, reader misuse,
   stream-body refusal), assert the full `err.to_string()` text with
   `assert_eq!`. Textual identity to the 2.24-era diagnostics was established
   by mission 490's landed migration (commit `54ad37c97`, suites green at its
   acceptance run); no 2.24 diagnostic artifact exists in-repo, so this
   mission freezes the exact text as the durable oracle for future bumps
   rather than re-claiming an unverifiable pre-upgrade comparison. The
   `language-value-boundary` spec and `missing_value_semantics.rs` shape
   assertions remain unchanged.
2. **ed25519-dalek 3.0 — verify the byte-identical signing contract.** Run
   `signing_compat_test.rs` (frozen 2.2.0-produced envelope must verify and
   re-sign byte-identically under 3.x). If it fails, STOP and park with the
   byte diff. Record the vector evidence in the park.
3. **Document the remaining transitive ed25519 duplicate.** `cargo tree -d`
   still shows `ed25519-dalek 2.2.0` via `jsonwebtoken 10.4.0 ← surrealdb-core
   3.2.4` (pinned `jsonwebtoken ^10.3.0`; unification to 11.1.0 already used by
   `camel-auth` was attempted and rejected by the resolver). This is
   ecosystem-blocked — same class as the k8s-openapi deferral. Evidence goes to
   the park and rc-k93qw stays open.

**Not in this mission:** `wasmtime-wasi 49 → 50` (breaking component API —
separate mission, stays in bd). `k8s-openapi 0.28 → 0.29` is **already
resolved on main** (single lock version 0.29.0, `kube 5.0.0`) — mission 490
landed it; the order's premise is stale and this is recorded, not re-opened.

Affected crates: `camel-template`, `camel-language-minijinja`, `camel-cli`
(signing test only). Expected diff: new/extended test files plus this change's
openspec artifacts only — **no production code, no manifest or `Cargo.lock`
edits, no change to the frozen signing fixture** (locked verification).

## Acceptance criteria

- Template/language suites green; every engine call-site shape is mapped in
  the persisted coverage matrix with a named test; no `from_serialize`
  anywhere.
- Exact-diagnostic pins added: full `err.to_string()` equality for canonical
  broken-template inputs (strict-undefined, reader misuse, stream refusal).
- `signing_compat_test` green; frozen 2.2.0 envelope byte-identical under 3.x.
- `cargo tree -d`: no duplicate minijinja; the one ed25519 dup documented as
  ecosystem-blocked with resolver evidence.
- Park written with per-dependency outcome, vector evidence, deferred items
  (wasmtime-wasi 50; k8s-openapi already resolved), gate table, buzzer line.

## Risk budget

Low: no API migration in flight; risk is concentrated in false "green"
claims — mitigated by running real suites, not assertions. Out of bounds:
touching surrealdb/kube/jwt manifests to force unification; changing the
ADR-0083 envelope format; any wasmtime migration.

Ref: bd rc-k93qw (stays open), ADR-0083, specs `language-value-boundary`,
`template-component`.
