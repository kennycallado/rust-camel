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

## Decision 2 — Flavor bodies (hand-curated contract)

Budget-based curation was REJECTED by e_opus (§2): size is the wrong axis;
security surface and fail-closed startup coupling are not percentages. The
contract is prefix sets in `feature_profiles.rs`, reviewed like API surface.

- **slim** (musl-only, pure Rust): base CLI closure (core, direct, seda, log,
  file, timer — unconditional after rc-9720m) + `mqtt` + http-static path
  (`slim-http` body: the http server components with pure-Rust hyper). No
  security features — their omission must fail closed at startup with a
  tested rejection (an insecure option on a slim build errors out, never
  degrades silently).
- **regular** (7 targets, pure Rust): `full` minus `exec` (verdict: exec is a
  support-and-CVE magnet, fail-closed capability model is a full-flavor
  concern) minus `lang-js`/`lang-rhai`/`lang-xpath` (regular keeps
  `lang-jsonpath` + `lang-minijinja` only) minus `kafka` (already absent from
  `full`; kafka remains full-only via `flavor-full`).
- **full**: `full` + `kafka` — unchanged from rc-5t5fo.3.

Feature syntax in `crates/camel-cli/Cargo.toml`:
```toml
flavor-slim    = ["mqtt", "http-static", "slim-http"]   # final body; see alias note
flavor-regular = ["otel", "grpc", "wasm", "http-static", "llm", "surrealdb",
                  "mqtt", "mcp", "integration-http", "integration-sql", "security",
                  "redis-tls", "lsp", "lang-jsonpath", "lang-minijinja", "jms", "sql",
                  "redis", "opensearch", "ws", "cxf", "xj", "xslt"]
flavor-full    = ["flavor-regular", "exec", "lang-js", "lang-rhai", "lang-xpath", "kafka"]
```
(Normalization: full as regular+delta avoids duplicating 20 entries and makes
the regular contract the single list to review. `surrealdb` stays in regular —
its nixpkgs LLVM block is a nix problem, not a flavor problem.)

Alias chain: `slim-http` → `slim-benchmarks` (one-release aliases, rc-n6iop
tracks expiry at 0.50). If this change lands for ≥0.50, DELETE both aliases
and fold the real slim body into `flavor-slim` directly. When de-aliased, the
empty `slim-benchmarks` placeholder is replaced by the pure-Rust http-static
server closure (the hyper-based `http-static` path — no `security`, no
`exec`, no `lang-*` beyond none; the concrete component set is the
`http-static` feature body in camel-cli, already exercised by the slim
closure tests). rc-n6iop remains the tracking ticket for the alias expiry
itself.

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
- **`surrealdb` in regular**: keeps a heavy dep in the recommended flavor;
  accepted (pure Rust from cargo's view; nixpkgs block is upstream-LLVM).
- **Default closure change**: `cargo build` (no features) now yields the
  regular contract — golden fixture regen covers it.

## Phases

Single-phase change; task ordering inside tasks.md handles the lockstep
(features → tests → matrix → docker → release guard → docs). No phase gates
beyond the standard flow.
