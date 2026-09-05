# assurance-tooling Specification

## Purpose
TBD - created by archiving change mutation-testing-tooling. Update Purpose after archive.
## Requirements
### Requirement: xtask mutants wrapper with main-checkout and disk isolation guards

The project SHALL provide a `cargo xtask mutants [--file P | --diff] [--json]`
subcommand that mirrors the shipped `xtask fuzz` wrapper discipline: it SHALL
refuse to run when the working directory resolves to the main checkout (exit
non-zero, instructive message, no build invoked), SHALL check for
`cargo-mutants` and exit non-zero with an install hint when absent, and SHALL
force `CARGO_TARGET_DIR` to the git-ignored worktree-local `target-mutants/`.
The subcommand SHALL NOT appear in the `QUALITY GATES` block, and its exit
status SHALL carry no gating semantics (exit 0 with survivors listed;
non-zero only for operational failure).

#### Scenario: Wrapper refuses the main checkout

- **GIVEN** a shell whose working directory resolves to the main checkout
- **WHEN** the user runs `cargo xtask mutants`
- **THEN** the command SHALL exit non-zero with a message naming the main
  checkout and instructing the user to run from a worktree, and SHALL NOT
  invoke any cargo-mutants or other instrumented build command (the
  `cargo xtask` alias itself may build xtask — isolation runs therefore
  invoke the prebuilt wrapper binary directly)

#### Scenario: Instrumented builds stay out of the shared target

- **GIVEN** a feature worktree with a clean `target-mutants/` and a recorded
  mtime fingerprint of the default `./target` tree
- **WHEN** the PREBUILT wrapper binary (isolation acceptance invokes
  `$WT/target/debug/xtask` directly — the `cargo xtask` alias may itself
  build xtask) completes a run
- **THEN** all mutation build artifacts and the `mutants.out/` outcomes tree
  SHALL be under `<worktree>/target-mutants/` (the wrapper passes
  `--output` because `CARGO_TARGET_DIR` does not relocate cargo-mutants'
  default repository-local `mutants.out/`), and the worktree and main
  target trees' mtime fingerprints SHALL be unchanged with no
  `mutants.out*` at the repo root

#### Scenario: Wrapper behavior is unit-tested

- **GIVEN** the xtask test suite
- **WHEN** testing the `mutants` subcommand
- **THEN** unit tests SHALL pin the main-checkout refusal path (exit code +
  message), the missing-tool hint, the `CARGO_TARGET_DIR` value, and verbatim
  `--file` flag mapping, without invoking real instrumented builds

### Requirement: Mutation scope pinned to the security module set, informational only

Mutation examination SHALL implement the two-tier scope: the baseline via
`.cargo/mutants.toml` `examine_globs` pinned to exactly
`crates/camel-api/src/ssrf.rs`, `crates/components/camel-mqtt/src/config.rs`,
and `crates/components/camel-jms/src/config.rs` (the narrow security files —
SSRF classification and the mqtt/jms `redact_broker_url` family), and
event-driven targeted runs via `--file <path>` for the broad files whose
security-critical functions are a minority of the file (camel-http
`lib.rs` `redact_url_for_diagnostics`, camel-file `lib.rs`
`validate_relative_filename`, `crates/camel-processor/src/aggregator.rs` and
`resequencer/` limit enforcement, and
`crates/components/camel-jms/src/component.rs` `redact_url`).
No mutation-score threshold SHALL be
enforced anywhere; survivor output SHALL be informational (optionally JSON for
bd automation), and the baseline kill-rate measurement SHALL be recorded in
bd, consumed by no gate. The baseline probe's completion within ~15 minutes
is a MEASURED engineering guidance figure against the recorded dev-workstation
environment (12 cores / 27 GB at plan time; each measurement records
`nproc` + `free -h` alongside it) — not an enforced timeout, and not an
ubuntu-latest claim (no CI job runs mutants in this change). Budget
tolerance: warm runs up to ~20 minutes are accepted (human decision
2026-09-05 — coverage and security signal outweigh seconds over the
guidance figure under concurrent machine load; measured 909s warm). The
design fallback cascade applies only beyond that tolerance.

#### Scenario: Baseline probe examines only the pinned narrow files

- **GIVEN** a worktree with `cargo-mutants` installed and default
  `cargo xtask mutants` (no flags)
- **WHEN** the run examines the tree
- **THEN** only the three pinned files SHALL be mutated, artifacts SHALL stay
  under `target-mutants/`, and the survivor list SHALL print informationally
  with exit status 0 regardless of survivor count

#### Scenario: Targeted run names a broad file explicitly

- **GIVEN** a developer touching `crates/components/camel-http/src/lib.rs`
- **WHEN** they run `cargo xtask mutants --file
  crates/components/camel-http/src/lib.rs`
- **THEN** the run SHALL mutate only that file (whole-file, the documented
  cargo-mutants limitation) and report survivors informationally, and the
  flag mapping SHALL be verbatim (no path rewriting)

#### Scenario: Kill rate is recorded, never enforced

- **GIVEN** the completed baseline run with its measured kill rate
- **WHEN** the result is triaged
- **THEN** the rate and actionable survivors SHALL be recorded in bd rc-eba8,
  and no CI job, gate, or threshold SHALL consume the number

