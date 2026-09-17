# ADR-0077: Fuzzing and Mutation Testing Adoption

- Status: Accepted (decided 2026-08-31, canonized 2026-09-17)
- Companion: security audit 2026-08-31, recommendation R1 (no fuzz targets
  existed)
- Source: architect decision memo of 2026-08-31 (unversioned, `docs/audits`)
- Normative specs: `openspec/specs/fuzz-tooling/spec.md`,
  `openspec/specs/fuzz-smoke/spec.md`
- Implementation: `fuzz/` (targets `dsl_yaml`, `dsl_json`, `dsl_parity`,
  `dsl_template`), `cargo xtask fuzz` / `cargo xtask mutants`,
  `.cargo/mutants.toml`, `.github/workflows/fuzz-smoke.yml`

## Context

The 2026-08-31 security audit found zero fuzz targets. The parsers of
untrusted input are the proven attack surface of this codebase.

The repo has two disk constraints. The workspace member globs would sweep a
nested fuzz crate into the shared build and lockfile. The default `./target`
in the main checkout must stay cold. A naive cargo-fuzz or cargo-mutants
setup adds 20-40 GB target trees and blocks merges.

The architect decided the adoption on 2026-08-31 in a decision memo. This
ADR canonizes that decision. The implementation already landed. The specs
named above are the normative authority for how the tools run.

## Decision

1. **Adopt cargo-fuzz now.** Rank targets by exposure to untrusted input.
   The order is: DSL YAML parse, DSL JSON parse, `${env:}` interpolation,
   Simple expression, SSRF IP classification, per-component URI parsers.
   The first two ranks landed, with two extra parity targets (`dsl_parity`,
   `dsl_template`). The fuzz-tooling spec owns the schedule for the
   remaining ranks.
2. **Adopt cargo-mutants scoped, informational, second.** Scope is
   module-level over the security-critical modules: `camel-api/src/ssrf.rs`,
   the `redact_*` family, the file path validators, the aggregator and
   resequencer limit enforcement, and the claim-check depth cap. Whole-crate
   runs are forbidden. No mutation score is enforced anywhere. The output is
   a surviving-mutant list that a human reads.
3. **Sequence: fuzz first, mutants second.** Fuzzing grows the regression
   corpus. Mutation testing then grades that corpus. The reverse order grades
   a suite that fuzzing is about to improve.

## Consequences

- Neither tool is a merge gate. The PR fuzz smoke job is `continue-on-error`.
- Findings become bd issues. Minimize each crash and commit it as a
  regression test in the owning crate. Raw corpora and crash blobs never
  enter the repo.
- Every fuzz or mutants build sets `CARGO_TARGET_DIR` to a dedicated
  worktree-local directory. Both wrappers refuse to run in the main checkout.
  The `fuzz/` crate is `workspace.exclude`d so the member globs cannot sweep
  it in.
- Do not fuzz the JS, WASM, or exec sandboxes for OOM. The audit records Boa
  heap amplification (finding F3-2) as an accepted residual with no API to
  bound it. A fuzzer would only rediscover that known limit.
