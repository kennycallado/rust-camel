# Proposal: flavor-markers

## Why

The three-flavor distribution split (bd rc-5t5fo, e_opus verdict
2026-09-17) needs each released binary to identify its flavor. Today
there is no machine-readable notion of "which profile is this binary":
release CI composes feature sets through a fragile sed pipeline
(`release.yml` Build binary step: merging `KAFKA_FEATURES` and
`ALLOC_FEATURES` with comma surgery — the class of bug that already
shipped a silently-dropped feature once, rc-vnm8), and `--version`
prints a bare semver with no profile information, so support triage
cannot tell the three future binaries apart.

## What Changes

1. Three marker features in `crates/camel-cli/Cargo.toml`, one per
   flavor, as the single source of truth for profile selection:
   - `flavor-slim = ["slim-http"]`
   - `flavor-regular = ["full"]`
   - `flavor-full = ["full", "kafka"]`
   - `default = ["flavor-regular"]` (closure-identical to today's
     `default = ["full"]`; the golden fixture must not move).
   Marker priority for overlapping selection (cargo cannot express
   mutual exclusion): full > regular > slim; builds with no marker
   enabled report `custom`.
2. `--version` reports the flavor in a parseable suffix:
   `camel 0.48.0 (regular)`. The flavor is computed at compile time
   from the marker cfgs; unmarked builds report `(custom)`. The
   compiled-artifact manifest's `RUNTIME_VERSION` stays semver-only
   (schema stability); the suffix is CLI-presentation only.
3. `release.yml` stops sed-composing features: each leg's matrix key
   becomes a single flavor marker plus, where needed, the allocator
   feature (`flavor-full` on the 5 kafka legs, `flavor-regular,jemalloc`
   on the 2 musl legs). Closures are byte-identical to today's legs;
   `kafka-probe`, `install-librdkafka`, and `use-cross` gates are
   untouched.

### Non-goals

- Flavor CONTENT curation (mqtt-in-slim, bridges-in-regular, kafka
  gating inside `full`) belongs to the flavor-matrix change
  (rc-5t5fo.5) and depends on rc-9720m/rc-wcs3v landing.
- Docker tags, binstall metadata, latest-channel flip (rc-5t5fo.6/.8/.9).
- Mutual exclusion enforcement between markers (documented priority
  instead; cargo has no exclusive-feature mechanism).

## Acceptance Criteria

- `cargo tree -p camel-cli --locked` output (package set) is identical
  before/after this change; `default_closure_matches_golden` passes
  without fixture regeneration.
- `camel --version` on a default build prints `... (regular)`;
  `--no-default-features --features flavor-slim` build prints
  `... (slim)`; `--features flavor-full` build prints `... (full)`;
  `--no-default-features --features slim-http` (raw, unmarked) prints
  `... (custom)`.
- release.yml contains no feature sed composition; each leg's build
  command carries exactly one flavor marker (+ allocator where
  applicable).
- Existing kafka probe, jemalloc assert, and gating tests unchanged
  and green.

## Risk Budget

Low. No closure changes (pure renames of how the same feature sets are
expressed); the only runtime-visible delta is the version suffix. Risk
concentrated in release.yml leg transcription, guarded by closure
equivalence tests and the existing probes. Breaking surface: none
(`full`/`slim-http`/`kafka` names unchanged).

Bd: rc-5t5fo.3
