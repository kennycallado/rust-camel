## MODIFIED Requirements

### Requirement: Guarded process-environment mutation in tests

A test SHALL inject configuration directly when the API permits it. A
test OF process-environment behavior SHALL run in a dedicated child
process with an explicit environment. In-process mutation of the
process environment is a documented legacy exception: it SHALL use the
canonical RAII guard (`camel_component_api::test_support::EnvGuard`,
behind the `test-support` feature), one process-wide lock, and SHALL
restore the prior value — or absence — on drop. Ad-hoc per-crate env
locks (`camel-config` `env_lock`, `camel-bench` `ENV_LOCK`,
`camel-cli` `TRUSTSTORE_ENV_LOCK`) SHALL NOT be used by new or migrated
test code. Async or multi-threaded mutation is forbidden unless all
readers and writers are proven to use that same lock (the
`global-state` flake class). Lock poisoning SHALL NOT cascade: the
guard's lock acquisition resumes from the poisoned mutex. Reader tests
that must observe ambient-controlled configuration SHALL take the same
lock via `EnvGuard::acquire()` for the duration of the read. The
guard's module docs SHALL state the safety posture without overclaim:
all Rust-side participants are serialized by the guard's lock;
non-Rust (libc/FFI) environment readers cannot be excluded in a
multithreaded test process — that residual is the documented, accepted
limitation of the legacy exception (rc-99d5 R4 as amended: "narrows
but is not hermetic") — and the mitigation hierarchy is injection
first, out-of-process for environment-behavior subjects, guarded
in-process mutation last.

#### Scenario: Direct injection wins

- **GIVEN** a component API that accepts configuration values as
  parameters or builders
- **WHEN** a test needs those values set
- **THEN** the test injects them through that API and does not touch
  the process environment at all

#### Scenario: Environment-behavior tests run out of process

- **GIVEN** a test whose subject is how code reads the process
  environment (variable presence, precedence, escaping)
- **WHEN** the test sets those variables
- **THEN** it runs in a dedicated child process with an explicit
  environment, so no other test in the binary observes the mutation

#### Scenario: Legacy in-process mutation restores prior value

- **GIVEN** an in-process test that must mutate an environment variable
  under the documented legacy exception
- **WHEN** the mutation begins
- **THEN** the canonical RAII guard holds one lock, sets the value, and
  restores the prior value — or absence — on drop; every Rust-side
  reader or writer of that variable in the same process is synchronized
  by that lock, and non-Rust (libc/FFI) environment readers are
  explicitly documented as the residual, accepted limitation of the
  exception (rc-99d5 R4 as amended: "narrows but is not hermetic")

#### Scenario: Single canonical guard across crates

- **GIVEN** test code in camel-config, camel-dsl, camel-http,
  camel-bench, camel-cli, camel-test, or camel-core that mutates the
  process environment
- **WHEN** the mutation happens
- **THEN** it goes through `camel_component_api::test_support::EnvGuard`
  and no crate-local env-lock duplicate remains in migrated code

#### Scenario: Reader test survives hostile ambient value

- **GIVEN** the camel-config profile-loading cluster (rc-yzpb: its
  `from_file` reader used to follow the ambient `CAMEL_PROFILE` left by
  a sibling test and fail "Unknown profile: production" on
  `[default]`-only configs), a hostile ambient value of
  `CAMEL_PROFILE=production` held by the canonical guard for the whole
  test, and the original deterministic assertion of
  `test_from_file_resolves_placeholders_in_components_and_beans`
- **WHEN** the config loads through the existing explicit-profile entry
  point `from_file_with_profile(path, Some("default"))`, whose explicit
  selection takes precedence over the ambient variable
- **THEN** the original assertions pass verbatim — routes default,
  `base_url`, `plugin`, and `token` placeholder fallbacks resolve —
  decided by explicit profile selection plus guard serialization and
  not by the hostile ambient value

#### Scenario: Multi-key mutation shares one guard

- **GIVEN** a test that needs several environment variables set for one
  scenario
- **WHEN** the test mutates them
- **THEN** all mutations happen on one guard instance via the chaining
  mutators, so nested or repeated lock acquisition cannot self-deadlock

## ADDED Requirements

### Requirement: No unguarded env mutation in test scope

The `cargo xtask lint-no-env-mutation-unguarded` gate SHALL fail when
`env::set_var` or `env::remove_var` appears in a test-scope file
(`tests/` directory, `*_tests.rs` / `test_*.rs` filename, or a file
containing a `#[cfg(test)]` module) outside the canonical guard's own
source, unless that exact site carries an adjacent
`env-guard-exception: <non-empty reason>` marker. Each marker SHALL
adjudicate one site. A checked-in ledger SHALL seed the ratchet with
one entry per file: the marked-site count at migration-complete
snapshot plus its adjudication summary. The gate SHALL fail when a
file's marked count exceeds its ledger entry, and SHALL fail when it
falls below the entry until the entry is lowered in the same change —
ledger entries SHALL never increase. The gate SHALL be wired into CI
next to `lint-no-free-port` and registered in the AGENTS.md
`## QUALITY GATES` block.

#### Scenario: Raw mutation in test scope fails the gate

- **GIVEN** a test-scope file calls `std::env::set_var` directly with
  no adjacent marker
- **WHEN** `cargo xtask lint-no-env-mutation-unguarded` runs
- **THEN** the gate exits non-zero and names the file and line

#### Scenario: Marker without reason fails the gate

- **GIVEN** a raw site carries an `env-guard-exception:` marker whose
  reason is empty
- **WHEN** `cargo xtask lint-no-env-mutation-unguarded` runs
- **THEN** the gate exits non-zero and names the file and line

#### Scenario: Guarded mutation passes

- **GIVEN** a test-scope file mutates the environment only through
  `camel_component_api::test_support::EnvGuard`
- **WHEN** `cargo xtask lint-no-env-mutation-unguarded` runs
- **THEN** the gate passes with no marker and no ledger entry needed
  for the file

#### Scenario: Adjudicated exception stays within its seed

- **GIVEN** a test-scope file whose ledger entry seeds N marked sites
- **WHEN** the file gains an (N+1)th raw site, marked or unmarked
- **THEN** the gate exits non-zero: an unmarked site fails the marker
  rule, and a marked site exceeds the never-increasing ledger entry

#### Scenario: Removing residuals lowers the seed

- **GIVEN** a migrated-away site drops a file's marked count below its
  ledger entry
- **WHEN** `cargo xtask lint-no-env-mutation-unguarded` runs before the
  entry is lowered
- **THEN** the gate exits non-zero until the same change lowers the
  entry, keeping the ratchet monotone toward zero

#### Scenario: Gate is registered and wired

- **GIVEN** the repository CI workflow and AGENTS.md gate registry
- **WHEN** either is inspected after this change
- **THEN** `cargo xtask lint-no-env-mutation-unguarded` appears in both,
  adjacent to the existing `lint-no-free-port` and
  `lint-no-raw-http-test-server` gates
