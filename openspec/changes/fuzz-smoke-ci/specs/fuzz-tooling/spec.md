# fuzz-tooling Delta Spec (modification)

## MODIFIED Requirements

### Requirement: Crash minimization and promotion

When a fuzz run finds a crash, the xtask wrapper SHALL minimize the
input with `cargo +nightly fuzz tmin <target> <artifact>`, forwarding
the fuzz run's `-artifact_prefix` so the minimized output lands in the
same `target-fuzz/artifacts/<target>/` directory the wrapper scans,
report the minimized input path, and instruct the operator to add a
committed Rust regression test. Raw crash artifacts SHALL NOT be
committed, and minimization SHALL NOT write to cargo-fuzz's default
`fuzz/artifacts/` location.

#### Scenario: injected panic is caught and minimized

- **GIVEN** a harness build that contains an intentional panic on a known
  input shape
- **WHEN** the fuzzer finds the crash
- **THEN** the wrapper runs `tmin`, prints the minimized artifact path under
  `target-fuzz/artifacts/<target>/`, prints the regression-test promotion
  instruction, exits non-zero so the crash is not silently swallowed, and
  no crash file is added to git

#### Scenario: tmin writes only to the scanned artifact directory

- **GIVEN** a crash artifact under `target-fuzz/artifacts/<target>/`
- **WHEN** the wrapper runs `tmin` on it
- **THEN** the minimized file appears under `target-fuzz/artifacts/<target>/` and no `fuzz/artifacts/` directory is created
