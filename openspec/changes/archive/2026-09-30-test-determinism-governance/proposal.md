# Proposal: test-determinism-governance

## Why

ADR-0069 §13 made the seven-class flake taxonomy and the test-design
rules R1–R7 normative, but that doctrine lives only in the ADR. The
flaky-tests epic (bd rc-99d5) needs one citable governance home in the
spec canon for the R7 quarantine rules, and bd rc-jwp3 orders governance
split into focused changes instead of one big one. Today a reviewer who
asks "what does this repo require of a deterministic test?" must read an
ADR appendix; the `test-determinism` capability carries mechanics-level
requirements (lint resolution, nextest ceilings, virtual time) but not
the design rules themselves.

## What Changes

One ADDED-requirements delta on the `test-determinism` capability,
codifying five rule areas plus the quarantine policy as spec
requirements, faithful to ADR-0069 §13.1–13.2 as amended by the
counter-review, with runner-pollution hygiene additionally anchored on
the sealed fleet containment policy (bd rc-tyq9e):

- `no-sleep-as-sync`: sleep never substitutes for synchronization; a
  `wait_until` barrier on observable state replaces it.
- `no-free-port`: bind-inspect-close-rebind is forbidden; staged
  listeners per ADR-0070 (capability `staged-listener-binding`).
- `no-raw-http-test-server`: outbound HTTP client tests use real
  loopback servers with correct connection semantics — pooled reuse is
  served by a complete request loop; one-response-and-close only in
  single-transaction scenarios; raw TCP only for protocol-fault tests.
  (The `http-test-harness` capability covers consumer readiness and
  registry mutation serialization; the loopback-server rule itself is
  new here.)
- `no-env-mutation-unguarded`: direct injection first, child process
  for env-behavior tests, in-process mutation only through the
  crate-wide EnvGuard pattern.
- runner-pollution hygiene: no leaked processes, listeners, or
  firewall residue; bounded teardown that reaps what a test spawned.
- R7 quarantine policy: registry entries with exact test ID, bd issue,
  owner, and ISO expiry; gating lint rejects bad entries; 14-day
  maximum lifetime; `#[ignore]` is not quarantine.

Explicitly excluded: scanner and lint implementation (bd rc-3lx2 and
the part-3 mechanics change — nextest rollout measurement, quarantine
registry build, gating lint build), any ADR text change (ADR-0069
already amended), R5 (capability `ignore-test-policy` stands unchanged),
and R6 per-test ceilings (already codified here as "Bounded per-test
execution in the Rust library-test job").

## Acceptance criteria

- `openspec validate test-determinism-governance --type change` passes
  with no delta-structure errors.
- Six ADDED requirements on `test-determinism`, each with at least one
  GIVEN/WHEN/THEN scenario, in English.
- Every cross-reference (ADR-0069 §13, ADR-0070, capabilities
  `staged-listener-binding`, `http-test-harness`, `lint-test-sleep`,
  `unbounded-wait-bounding`, `ignore-test-policy`, bd rc-w1u9,
  rc-1dgvg, rc-s7dyw) matches an existing artifact.
- No runtime code changes; no crate builds; spec-only.

## Risk budget

Low risk: spec-only, no code. The one real hazard is canon duplication
with the mechanics capabilities — mitigated by scope discipline: the
RULE statements and their scenarios live here; lint resolution details,
readiness helpers, and listener mechanics stay in their own
capabilities, cross-referenced by name.

Bd: rc-jwp3 (epic rc-99d5).
