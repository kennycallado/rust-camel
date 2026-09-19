# Design: three-flavor-matrix

## Context

Operative ruling = bd rc-5t5fo.5 (written after, and partially superseding,
the e_opus verdict of 2026-09-17). Where the two diverge, this section records
the call and its rationale. Prior landed work this builds on: flavor markers +
`--version` suffix (rc-5t5fo.3, squash c1356314), optional bridges for slim
(rc-9720m), minijinja slim exclusion (rc-wcs3v), reusable release matrix +
dev harness (rc-5t5fo.4, 010e84af), publisher homing (ADR-0082, b56f9681).

## Decision 1 — Immediate re-alias vs transition release

The verdict (§4) recommended keeping `camel-<target>` = full for one
transition release and re-aliasing at 1.0. The bd ruling overrides: **immediate
re-alias at the first flavored release (0.50), one-time semantic break,
release-notes callout** (xtask changelog already gates announcements).
Rationale: pre-1.0, single announcement covering artifact rename + Docker
`latest` + crates.io `default` (already flipped), and the transition path
would ship FOUR names for one release (legacy-full, slim, regular, full) at
14+ legs. binstall metadata (rc-5t5fo.8) lands AFTER this change pointing at
final semantics — no stale-name window exists because binstall's default
guesses never matched (crate `camel-cli` vs assets `camel-*`).

## Decision 2 — Flavor bodies (chained, principled exclusions)

FINAL MODEL (owner rulings 2026-09-19, after three expert adjudications, one
completeness sweep, and the license axis — see
`docs/audits/2026-09-19-flavor-facts-dossier.md` and siblings). Bodies are
CHAINED: each flavor includes the one below (`slim ⊆ regular ⊆ full` is
structural, not tested-in). Moving a feature = editing exactly ONE list.

Principles — regular excludes ONLY what violates a named principle:
kafka (C dep, musl-impossible), surrealdb (BUSL-1.1, only non-OSI dep in
the graph), exec (arbitrary host-binary execution, ADR-0037 doctrine),
containers (Docker-daemon socket client: camel-function + camel-component-
container gated as ONE feature). Everything else — including wasm (sandboxed
canonical ABI per ADR-0050), lang-js/boa, lsp, rhai — is IN regular by owner
ruling. Slim adds the edge pack on top of base.

```toml
flavor-slim = ["mqtt", "mqtt-tls", "http-static", "sql",
               "lang-jsonpath", "lang-rhai"]
flavor-regular = ["flavor-slim", "otel", "grpc", "wasm", "llm", "mcp",
                  "security", "redis", "redis-tls", "jms", "cxf", "xj",
                  "xslt", "opensearch", "ws", "lang-xpath", "lang-js",
                  "lang-minijinja", "lsp", "kubernetes",
                  "integration-http", "integration-sql"]
flavor-full = ["flavor-regular", "exec", "kafka", "surrealdb", "containers"]
```

New gates this change creates (Tier-2 optionalization):
- `containers = ["dep:camel-function", "camel-bundles/containers"]` —
  camel-cli dep goes optional; camel-bundles gains feature gating the
  ContainerBundle registration (camel-bundles/src/lib.rs:344) and the two
  `camel_function::FunctionRuntimeService::with_default_container_provider`
  call sites in camel-cli (`run.rs:253`, `job/mod.rs:1022`) get
  `#[cfg(feature = "containers")]` gates (function⇒container per ADR-0005,
  one feature). Fail-closed: without the feature no function runtime is
  constructed and `function:`/`container:` paths error explicitly at those
  call sites.
- `kubernetes = ["camel-config/kubernetes"]` — remove the hardcoded
  `"kubernetes"` from camel-cli's camel-config dep features (Cargo.toml
  dependency-declaration line); camel-config already has the feature.
  The hardcoded `otel` STAYS (base telemetry wiring, documented as
  base-cost honesty in the dossier).

The full minus-4 historical framing is REPLACED by the lists above. The
`full` feature (historical closure list) remains as-is for compatibility;
flavors no longer reference it.

Iteration doctrine (owner requirement — must be cheap to adjust):
- Flavors are PRESETS, not walls: users compose beyond them
  (`--features flavor-regular,cxf`).
- Additions flow DOWN freely in minor releases (full→regular→slim is
  non-breaking); removals only at majors. Documented in the distribution
  doc with a "how to move a feature between flavors" recipe (edit one
  list, update contract sets if a principle is crossed, regen fixture —
  one command; CI legs never move because legs are target×flavor).
- `full_covers_universe` test (Task 2): every camel-cli feature except
  the non-flavor axes (jemalloc, dynamic-linking, itest-e2e, and the
  marker features themselves) SHALL be reachable from flavor-full — a
  new feature placed nowhere goes red in CI. This mechanizes "nothing
  gets left out of camel-cli".

## Decision 3 — Matrix topology (14 tag legs)

| leg | target | runner | flavor |
|---|---|---|---|
| 1–2 | x86_64/aarch64-unknown-linux-musl | ubuntu | slim |
| 3–9 | all 7 current targets | as today | regular |
| 10–13 | gnu ×2 (**aarch64 on `ubuntu-24.04-arm` native**), macOS ×2 | native ARM | full |
| 14 | x86_64-pc-windows-msvc | windows | full |

Single selection surface: each leg passes `--features flavor-<x>` only. No
composed feature `sed` (verdict (c): the string-plumbing class of bugs goes
away). Windows/macOS runners unchanged. The gnu-aarch64-full leg moves from
QEMU cross to the native ARM runner (faster, no emulation cliff); gnu-aarch64
REGULAR stays where it is today.

## Decision 4 — Artifact + Docker renames (the lockstep seam)

Workflow artifacts, GH release assets, and docker `*-artifact` download keys
rename together: `camel-slim-<target>`, `camel-<target>` (regular),
`camel-full-<target>`. The release job's `files: dist/camel-*` already
prefix-matches; docker matrix keys must be updated in the same diff
(`amd64-artifact: camel-x86_64-unknown-linux-musl` → `camel-slim-x86_64-…` for
alpine, etc.) — e_opus final gate: this is the seam that silently breaks.

Docker variant mapping (verdict §5):
- production (scratch, musl) ← regular musl bins; tags: `latest`, `:regular`,
  legacy no-suffix.
- alpine (musl) ← slim musl bins; tags: `-alpine` (legacy) + `:slim`.
- gnu (distroless) ← full gnu bins; tags: `-gnu` (legacy) + `:full`
  (semantic alias — users reach for "full" when they want kafka, not a libc).
- musl images never link rdkafka — invariant asserted by closure tests.
- Dev smoke asserts flip: production/alpine → `(regular)`/`(slim)`, gnu →
  `(full)`.

## Decision 5 — Dev harness slimming (folded in, user-approved)

e_opus trigger adjudication 2026-09-18 ranked "slim dev-profile" first: keep
the per-push tripwire, cut ~60% runner cost. New `dev-profile: bool` input on
the reusable matrix (default `false` = full 14 legs). When true, the build
matrix reduces to 3 representative legs — gnu-full (x86_64-unknown-linux-gnu),
musl-regular (x86_64-musl), darwin-regular (x86_64-apple-darwin) — plus the
closure-check job unchanged. Windows/aarch64/macOS-full coverage moves to the
tag path only. Docker dev mode keeps ONE variant (gnu) per the existing smoke.
`release-dev.yml` passes `dev-profile: true`; tag wrapper unchanged.
Cancel-in-progress stays dev-only (rc-myx4r guard untouched: permissions trio
ceiling remains).

## Decision 6 — Prerelease tag safety (conductor addendum)

The smoke plan (one throwaway `vX.Y.Z-rc.1` tag to exercise
publish/OIDC/docker-login seams before the first real flavored release) would
fire the crates.io publish job. Guard: the publish job in `release.yml` gets
`if: !contains(github.ref, '-rc.')` — prerelease tags run the full matrix,
attach assets, push docker images, but skip crates.io. (Alternatively accept
prerelease publishing; ruling: skip — 0.50.0-rc.1 on crates.io is noise.)

## Decision 7 — Closure contract enforcement

`feature_profiles.rs` gains, alongside `SLIM_FORBIDDEN_PREFIXES`:
- `REGULAR_REQUIRED` — schemes that must resolve in regular (http, file,
  timer, log, direct, seda, mqtt, redis+tls, sql, jms, otel, jsonpath,
  minijinja, security guard active).
- `REGULAR_FORBIDDEN` — `camel-component-kafka`, `exec`, `lang-js`,
  `lang-rhai`, `lang-xpath`.
- `FULL_REQUIRED` — kafka (the historical defect: full shipped kafka-less).

The golden closure fixture regenerates IN THIS CHANGE (default =
flavor-regular; its closure changes when regular is re-curated) — visible in
review, never a surprise CI failure. Independently of the fixture: the
`default_closure_matches_golden` package-presence loop
(`feature_profiles.rs:260-271`) hard-asserts `camel-language-js/rhai/xpath`
in the default closure — it SHALL drop those three from its required-prefix
set (they leave the regular default tree); only `camel-language-jsonpath`
and `camel-language-minijinja` remain asserted-present.

`REGULAR_FORBIDDEN` SHALL reference the same prefix constants as
`SLIM_FORBIDDEN_PREFIXES` for the shared entries (kafka, exec, lang-js,
lang-rhai, lang-xpath — it is an exact subset), not a forked duplicate list. Release path gains per-flavor capability
asserts against the downloaded artifact (extend the existing
jemalloc-symbol-assert pattern; camel-bundles already has the gating assert at
`camel-bundles/src/lib.rs:475`).

## Risks

- **Artifact-name lockstep** (docker keys, release `files:` pattern, smoke
  probes, any external downloader): single-diff discipline + holistic review.
- **ARM runner availability**: `ubuntu-24.04-arm` is a GitHub-managed runner
  pool; if flaky, fall back to QEMU cross for that leg only (perf, not
  correctness).
- **Tier-2 gates are new code** (containers: two optionalized deps + cfg
  registration + fail-closed path; kubernetes: de-hardcode): the function/
  container fail-closed None-path is verified in Task 1 step 4 with a
  STOP-and-report escape hatch if the explicit rejection is missing.
- **boa/wasm in regular** (owner ruling): the default install carries a JS
  engine + wasmtime — accepted product cost for capability presence; the
  crate-count axis understates boa's compile weight (documented in the
  dossier's honesty section).
- **Default closure change**: `cargo build` (no features) now yields the
  regular contract — golden fixture regen covers it.

## Phases

Single-phase change; task ordering inside tasks.md handles the lockstep
(features → tests → matrix → docker → release guard → docs). No phase gates
beyond the standard flow.
