# Proposal: canonical-path-fuzzing

## Why

ADR-0026 makes JSON the canonical full-DSL authoring format for SDKs and
generators, yet the only fuzz target today is `dsl_yaml` (the human
convenience front-end). The canonical path — `camel_dsl::json` and the
ADR-0008 template expansion (JSON tree substitution before
deserialization) — is unfuzzed. The Phase 1 target choice was inherited
from naming, not argued against the canonical-format decision.

An expert ruling (e_opus, 2026-09-03, bd rc-fvah) also holds that the
planned mutation-testing phase (rc-eba8) is corpus-dependent: grading
mutants against a YAML-only corpus would measure the canonical path
blind. Closing the canonical gap first makes the later mutant signal
trustworthy.

## What Changes

Three new cargo-fuzz targets in the existing `camel-fuzz` crate, plus
wrapper and CI wiring:

- `dsl_json` — canonical JSON route parsing
  (`camel_dsl::json::parse_json_with_threshold_and_security`).
- `dsl_template` — JSON template section parsing
  (`parse_json_templates` + `parse_json_templated_routes`) and, when a
  document yields both, instance materialization (ADR-0008 expansion
  path).
- `dsl_parity` — differential harness enforcing the DIRECTIONAL
  agreement contract (JSON ⊂ YAML): when the JSON serde front-end
  deserializes a document to `RouteDslRoutes`, the YAML serde front-end
  must deserialize it to the same step list (pre-converter comparison,
  as the parity tests do); divergence on the overlap is a crash.
- Committed seed corpora per target. Valid `dsl_json` seeds come from
  real canonical documents (camel-dsl test fixtures, parity cases,
  schema examples); malformed/adversarial seeds are JSON-native
  fixtures — never YAML conversions.
- `cargo xtask fuzz` accepts the three new target names (everything else
  in the wrapper is already per-target).
- `fuzz-smoke.yml` runs the smoke serially over a path-filtered target
  set (front-end change selects its leg; `dsl_parity` runs when either
  front-end changes; dispatch runs all legs). The tmin drill moves to the
  `dsl_json` leg.

Excluded (tracked as bd rc-rs2v backlog): `rest.rs`, `openapi.rs`,
`mcp.rs` channels — distinct grammars and trust models, ranked later by
untrusted-input exposure. No production crate changes: `camel-dsl` and
`camel-api` code is untouched.

## Acceptance criteria

- `cargo xtask fuzz <target>` runs and isolates each of the four targets
  (worktree-local corpus/artifacts, seeds staged on first run).
- Each new target has committed seeds from its specified target-native
  sources. Valid `dsl_json` seeds trace to canonical JSON documents;
  malformed/adversarial `dsl_json` seeds are JSON-native fixtures; no
  `dsl_json` seed is a mechanical YAML-to-JSON conversion.
- `fuzz-smoke.yml` selects legs by changed paths on PRs, runs all legs on
  dispatch, and the tmin drill proves minimize-and-promote on `dsl_json`.
- A local injected-panic check (one per new target) demonstrates the
  wrapper catches crashes for all three new targets.
- CI run evidence: one dispatch with all legs green recorded in the
  change's verification notes.

## Risk budget

Acceptable: longer smoke jobs (more targets, still under the job
ceiling via path filtering and serial legs); transient CI cache misses.
Out of bounds: any change to production crates; relaxing the no-gating
contract of fuzz-smoke; raising the PR smoke per-leg budget above 60 s.

Bd: rc-fvah
