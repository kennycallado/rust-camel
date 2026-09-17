# Design: kafka-feature-wiring

## Approach

Feature-graph fix at camel-cli's public surface plus a release-artifact
capability assert. Four pieces:

1. `crates/camel-cli/Cargo.toml` `[features]`:
   - `kafka = ["dep:camel-component-kafka", "camel-bundles/kafka"]` —
     definition unchanged; comment block rewritten (capability + default
     source build), dropping its `cmake-build`/`kafka-static` references
     — the only remaining doc site that mentions them (blesser-verified:
     no README/docs consumers exist).
   - `dynamic-linking = ["kafka", "camel-component-kafka/dynamic-linking"]`
     — now implies capability.
   - delete `cmake-build` and `kafka-static`.
2. `crates/components/camel-kafka` — NO changes: its `cmake-build` /
   `dynamic-linking` entries are internal rdkafka-sys forwards and stay
   (the AGENTS clippy gate builds the component `--all-targets`,
   unaffected).
3. `.github/workflows/release.yml`:
   - matrix `kafka-features: "cmake-build"` → `"kafka"` on the five
     kafka legs; musl legs unchanged.
   - new step "Assert kafka capability (release probe)", conditioned to
     legs where the built binary is executable on the runner (x86_64-gnu,
     macOS ×2, Windows; explicitly NOT the cross-built aarch64-gnu leg):
     write the fixture with `printf '%s\n'` lines (heredocs break under
     YAML block-indentation stripping — a quoted terminator must sit
     flush-left, which YAML indentation removes), run `camel lint` on
     it, and gate on a two-part normative check, both parts
     load-bearing: exit code 0 AND no `unverified-scheme` diagnostic
     in the output (matcher on the diagnostic code — word
     co-occurrence with "kafka" never matches: the header line omits
     the scheme name and ANSI escapes split the echoed token). Empirical calibration (verified by building both
     binary polarities): an unregistered kafka scheme is an
     Info-severity diagnostic that still exits 0, so exit code alone
     cannot discriminate; a registered component over an endpoint
     without `brokers=` errors into exit 1, so the fixture must carry
     `kafka:orders?brokers=localhost:9092`. Cross-built aarch64-gnu
     skips the probe: `#[cfg(feature)]` registration is compile-time
     and target-independent, and the identical `--features` string is
     exercised on x86_64-gnu.
   - update the feature-composition comments that reference cmake-build.
4. `crates/camel-cli/tests/feature_profiles.rs` additions:
   - `kafka_feature_table_implies_capability`: reads
     `crates/camel-cli/Cargo.toml` and asserts the feature table
     contains the exact line
     `dynamic-linking = ["kafka", "camel-component-kafka/dynamic-linking"]`
     and contains no `cmake-build`/`kafka-static` feature keys. The
     implication is asserted at the feature-table level because
     feature-forwarding edges never render in cargo tree (documented
     at feature_profiles.rs:183-186) — a closure-based implication
     test is vacuous.
   - `dynamic_linking_closure_resolves_kafka`:
     `cargo tree -p camel-cli --no-default-features --features
     dynamic-linking` contains camel-component-kafka (package-level
     presence) and, using `assert_absent` with
     `SLIM_FORBIDDEN_PREFIXES` minus the kafka prefix, no other
     controllable-optional crate.
   - `removed_kafka_feature_names_rejected`: `cargo tree --features
     cmake-build` (and `kafka-static`) fails with an unknown-feature
     error naming the feature.

## Affected crates

- camel-cli: feature table, feature-profile tests, CONTEXT.md build
  profiles note.
- camel-bundles: none — the gating probe tests (`src/lib.rs:437-463`)
  already cover both polarities and stay green unchanged.
- camel-kafka (component): none.
- `.github/workflows/release.yml`: matrix value, probe step, comments.

## Architecture boundaries

Components-layer wiring only. No route, DSL, core, or API code paths
change; the data/control-plane split is untouched. The probe exercises
the existing lint path (production component catalog) against the
shipped binary — no new runtime code.

Single-phase change.

## Alternatives considered

- Fix only the CI string (`--features kafka,cmake-build`): leaves the
  `kafka-static` trap and the three-names-two-behaviors surface intact;
  rejected as a symptom patch (e_opus verdict, Q3).
- Make `cmake-build` imply `kafka`: keeps a build tool naming a
  capability plus a redundant alias; rejected.
- Assert via workspace tests only: the workspace tests already pass
  today while shipped binaries are broken — the assert must run against
  the built artifact in release CI.
