# Design: flavor-markers

## Context

- Distribution verdict (e_opus 2026-09-17, docs/audits): three flavors
  (slim/regular/full); version-string flavor MANDATORY for triage.
- Today: `default = ["full"]`; `full` = historical default + lang + lsp,
  kafka NOT inside; `slim-http = []` is the no-default baseline marker.
- release.yml composes `--features` via sed comma-surgery over
  `KAFKA_FEATURES`/`ALLOC_FEATURES` matrix keys (rc-vnm8 class of bug).
- Flavor CONTENT (mqtt in slim, bridges in regular, kafka in full) is
  rc-5t5fo.5 territory, blocked on rc-9720m/rc-wcs3v. This change is
  the MECHANISM: markers + version + single-feature CI legs.

## Goals / Non-Goals

- Goals: marker features as single source of truth; parseable
  `--version` flavor suffix; kill the sed; zero closure drift.
- Non-Goals: content curation (.5), docker/binstall (.6/.8/.9),
  marker mutual exclusion (impossible in cargo; documented priority),
  manifest `RUNTIME_VERSION` change (schema stability — semver only).

## Decision 1: markers are aliases, `default` flips to `flavor-regular`

```toml
flavor-slim    = ["slim-http"]
flavor-regular = ["full"]
flavor-full    = ["full", "kafka"]
default        = ["flavor-regular"]   # was ["full"]
```

- Closure-identical by construction (pure forwarding); golden fixture
  (package-set based) must pass without regeneration.
- TODAY's honest mapping: default content == regular content (all
  components except kafka, plus lang+lsp); full == regular + kafka.
  When .5 curates content, only the marker bodies change — the
  selection surface (what CI and users type) is already frozen.
- Raw compositions (`--no-default-features --features slim-http`)
  remain legal; they report `custom`.

## Decision 2: compile-time flavor detection with documented priority

```rust
pub const FLAVOR: &str = if cfg!(feature = "flavor-full") {
    "full"
} else if cfg!(feature = "flavor-regular") {
    "regular"
} else if cfg!(feature = "flavor-slim") {
    "slim"
} else {
    "custom"
};
```

- Priority full > regular > slim: selecting multiple markers reports
  the superset flavor; documented in Cargo.toml comment (no exclusive
  features in cargo).
- `--version` wiring: clap `version` attr on the derive becomes a
  computed value: `concat!`-style const or a `fn version_line() ->
  String` (`{CARGO_PKG_VERSION} ({FLAVOR})`). Placement:
  `crates/camel-cli/src/main.rs` (single clap entry). Exact mechanism
  delegated to tasks; the observable contract is the printed line.
- `camel job`/`camel run` startup banners, if any mention version,
  stay untouched (only `--version` output changes).

## Decision 3: release.yml single-feature legs

Matrix key `kafka-features` → `flavor` (`"flavor-full"` on x86_64-gnu,
aarch64-gnu, both macOS, windows-msvc; musl legs carry
`flavor-regular` + keep `alloc-features: "jemalloc"`). Build step:

```
FEATURES="${FLAVOR}${ALLOC_SUFFIX:+,$ALLOC_SUFFIX}"  # no sed, no comma surgery
```

- Leg closure equivalence (proof obligation, tested):
  gnu/mac/win today = default + kafka ≡ flavor-full; musl today =
  default + jemalloc ≡ flavor-regular,jemalloc.
- `kafka-probe: true`, `install-librdkafka`, `use-cross`,
  `install-musl-tools`, jemalloc assert: unchanged keys/gates.
- New probe (cheap, native legs): `--version` output contains the
  expected flavor suffix per leg — catches marker transcription drift
  at release time.

## Decision 4: tests

- `feature_profiles.rs`: exact-set additions for the three marker
  lines; closure-equivalence tests (flavor-full tree == full+kafka
  tree; flavor-regular tree == default tree) reusing `tree_lines`.
- Version: unit test asserting the clap version string equals
  `concat(env!("CARGO_PKG_VERSION"), " (", FLAVOR, ")")` —
  self-consistent in every build polarity (no cross-compilation
  needed); the four-suffix matrix is exercised by the release legs'
  --version probe + a polarity build in the task's verification.
- Golden fixture: NOT regenerated (asserted unchanged).

## Risks / Trade-offs

- Default-flip (`["full"]` → `["flavor-regular"]`) is textual churn in
  Cargo.toml; anything parsing that exact line (none known; greps in
  task) would break. Golden test is the net.
- `custom` for raw builds is honest but new; docs (CONTEXT.md
  Feature-profiles paragraph) must state it.
- slimblockers (rc-9720m, parallel agent) will add bridge features to
  the same `[features]` section — different lines, no design
  interaction; both changes keep `full`'s body stable until .5.

## Migration Plan

None for users: `--features full`/`kafka`/`slim-http` keep working
identically; the sed removal is internal to CI. `.5` later flips leg
bodies and docker consumes the suffix.

## Open Questions

- None blocking. (Naming settled: `flavor-*` prefix per ticket
  rc-5t5fo.3; suffix format `(flavor)` per e_opus verdict.)

## Decision 5: compiled-artifact `--version` scope (added at spec-bless)

The `camel` binary has TWO `--version` surfaces. The interactive clap
path (`main.rs`) gains the flavor suffix. The compiled-artifact path
(`compile/runtime.rs`, reached by `self_detect_artifact` BEFORE clap)
prints `camel <RUNTIME_VERSION>` (bare semver) and is DELIBERATELY left
unchanged this change. Rationale: the artifact `--version` sits next to
`--manifest`, whose `runtime_version` JSON field is a machine-read
schema value that must stay semver-only; the two are read from the same
`manifest::RUNTIME_VERSION` const. Diverging the artifact `--version`
without also touching `RUNTIME_VERSION` would split a currently-shared
constant and invite the manifest field to drift. Triage of released
*artifacts* (the `.6`/`.8`/`.9` consumers) reads `--manifest`, not
`--version`; the flavor-in-artifact question is genuinely a later cut.
The spec now scopes the suffix Requirement to the interactive CLI and
adds a scenario pinning the artifact path to bare semver, so a future
task cannot silently regress either surface.

## Self-grill record

**Questions generated:**
1. [glossary] Does "flavor" / "marker feature" collide with any
   existing feature-table term (`full`, `slim-http`, `capability
   feature`, `RUNTIME_VERSION`) in CONTEXT.md / the Cargo.toml glossary?
2. [sharpen] "`camel --version` reports the flavor" — is "camel
   --version" one surface or several? Which binary entry point owns it?
3. [scenario] Construct a build/leg where the "closure-identical"
   claim is false — specifically the aarch64-gnu `use-cross` leg and a
   simultaneously-enabled `flavor-slim`+`flavor-full` build.
4. [cross-ref] Does the code today already print a bare `--version`
   somewhere the spec's universal wording would falsely bind, and does
   the golden test actually observe the `default` rename?

**Answers (with citations):**
1. [glossary] No collision. `full`/`slim-http`/`kafka`/`dynamic-linking`
   are the existing feature vocabulary (Cargo.toml `[features]`;
   `crates/camel-cli/CONTEXT.md` "Feature profiles"). `flavor-slim`
   /`flavor-regular`/`flavor-full` are a new prefixed namespace that
   forwards to those; no name reuse. "flavor" is already the verdict's
   term (docs/audits distribution verdict) and the ticket's. `custom`
   for unmarked builds is new but unambiguous — no feature is named
   `custom`.
2. [sharpen] TWO surfaces, and the original spec wording ("`camel
   --version` SHALL...") conflated them. Surface A: clap `version` attr
   (`main.rs:15`) — the interactive CLI. Surface B: the compiled-artifact
   runtime (`compile/runtime.rs:415-417`), reached by
   `self_detect_artifact` BEFORE clap (`runtime.rs:35-39`), printing
   `camel {RUNTIME_VERSION}` bare. Sharpened: the suffix belongs to
   Surface A only; Surface B stays bare (Decision 5). Spec + scenarios
   updated.
3. [scenario] (a) aarch64-gnu is `use-cross: true`
   (`release.yml:30-31`); its binary is not host-executable, so the
   per-leg `--version` probe cannot run there — the spec scenario
   already scopes the probe to "any release leg whose binary is
   executable on its runner" (spec.md "version flavor probed per leg"),
   which correctly excludes both cross legs (aarch64-gnu, aarch64-musl).
   Closure equivalence still holds: aarch64-gnu today builds
   `kafka` (`release.yml:33`) ≡ `flavor-full` = `["full","kafka"]`, same
   package set. (b) `--features flavor-slim,flavor-full` is a legal but
   nonsensical combination; the priority const (`FLAVOR` in design
   Decision 2, `full > regular > slim`) resolves it to `full`
   deterministically. No consumer needs mutual exclusion at build time
   (release legs each pass exactly one marker; binstall/docker are
   deferred to `.6`/`.8`). Sound.
4. [cross-ref] (a) Yes — `compile/runtime.rs:416` prints bare
   `--version`; the original spec's universal wording falsely bound it.
   Fixed by scoping (Decision 5 + new scenario). (b) The golden test
   (`tests/feature_profiles.rs::default_closure_matches_golden`) compares
   `tree_lines(&[])` — a sorted, deduplicated, normalized set of
   `cargo tree -e features,no-dev` lines — against a fixture. Cargo tree
   NEVER renders feature-forwarding edges (asserted in-code:
   `kafka_feature_table_implies_capability` comment, "feature-forwarding
   edges never render in cargo tree"). `default = ["flavor-regular"]`
   adds only a forwarding edge to an already-present `full` closure, so
   the package/feature-node set is byte-identical. The claim holds.
   Nothing else observes the rename: the only exact-string assertion in
   the test targets the `dynamic-linking` line
   (`DYNAMIC_LINKING_LINE`, `feature_profiles.rs:364`), not `default`
   or `full`. Coordination: slimblockers (rc-9720m) adds bridge
   features to `[features]` on different lines; no shared test asserts
   over the whole section body, only the two-line kafka surface — no
   collision.

**Outcome:** refine (sharpened the `--version` surface into A/B; added
Decision 5 and two spec scenarios; scoped the Requirement wording).
**Self-grill mode:** self-grill-proposals skill
