# Proposal: three-flavor-matrix

## Why

The release pipeline builds one closure (full-ish, kafka-less on musl) across 7
targets. The epic verdict (e_opus, `docs/audits/2026-09-17-distribution-flavor-verdict.md`)
endorses splitting distribution into three curated flavors — slim (IoT/edge,
pure Rust), regular (most-used, pure Rust, no kafka), full (everything incl.
kafka) — isolating the single C dependency (rdkafka) into full. The flavor
markers, version-suffix reporting, and closure-test machinery already landed
(rc-5t5fo.3, rc-9720m, rc-wcs3v). This change is the switch: it gives the
markers real bodies, grows the tag matrix to 12 entries (11 uploading), renames artifacts, and
re-aliases `camel-<target>` to regular.

The same change folds in the approved dev-harness slimming (e_opus trigger
adjudication 2026-09-18, user ruling): the per-push dev harness keeps only a
3-leg representative subset, cutting its runner cost ~60% while preserving the
drift tripwire (house doctrine rc-wk29: expensive legs belong to the release
path).

## What Changes

- **Flavor bodies** in `crates/camel-cli/Cargo.toml` (CHAINED — each flavor
  includes the one below; moving a feature = editing one list):
  - `flavor-slim` = edge pack on base: mqtt + mqtt-tls + http-static + sql
    (sqlite) + lang-jsonpath + lang-rhai. Musl-only in CI.
  - `flavor-regular` = slim + everything pure-Rust/OSI EXCEPT four
    principled exclusions: otel, grpc, wasm, llm, mcp, security, redis(+tls),
    jms, cxf, xj, xslt, opensearch, ws, lang-xpath, lang-js (boa),
    lang-minijinja, lsp, kubernetes, integration-http, integration-sql.
  - `flavor-full` = regular + exec, kafka, surrealdb, containers.
  - Principled exclusions from regular (each named): kafka (C dep, musl),
    surrealdb (BUSL-1.1, sole non-OSI dep), exec (arbitrary host-binary
    execution, ADR-0037), containers (Docker-daemon client — camel-function
    + camel-component-container gated as one feature).
  - New gates: `containers` (Tier-2: two optionalized deps + cfg-gated
    registration + fail-closed steps), `kubernetes` (de-hardcode the
    camel-config feature forward); `full_covers_universe` test mechanizes
    "nothing gets left out" (every feature must be reachable from
    flavor-full).
  - Remove the one-release `slim-http` alias chain (rc-n6iop).
- **Matrix** in `release-matrix.yml`: 12 entries = slim (musl ×2), regular
  (4 Linux targets + a macOS compile-guard that builds but never uploads),
  full (gnu ×2 on native `ubuntu-24.04-arm` for aarch64, macOS ×2, Windows).
  Desktop platforms ship full-only (post-rc.2 owner ruling; 11 uploaded
  assets). Per-leg `--features flavor-*` marker (single selection surface —
  no composed `sed` feature plumbing).
- **Artifact names**: `camel-slim-<target>`, `camel-<target>` (= regular,
  one-time semantic break, release-notes callout), `camel-full-<target>`.
- **Docker** job: production←regular musl bins, alpine←slim musl bins, gnu←full
  gnu bins; update `*-artifact` keys in lockstep (the seam that silently
  breaks); publish explicit `:slim`/`:regular`/`:full` tags alongside legacy
  suffixes; `latest` re-aliases to regular in the same release (one
  announcement with the artifact rename).
- **Closure contract** in `feature_profiles.rs`: add `REGULAR_REQUIRED`,
  `REGULAR_FORBIDDEN`, `FULL_REQUIRED` (kafka) prefix sets alongside
  `SLIM_FORBIDDEN_PREFIXES`; slim security omission must fail closed with a
  tested rejection. Capability asserts run against shipped artifacts in the
  release path, not only in tests.
- **Dev harness slimming**: new `dev-profile` input to the reusable matrix —
  dev runs 3 representative legs (e.g. gnu-full, musl-regular, darwin-regular)
  + closure-check; tag runs remain 12 entries.
- **Prerelease safety**: crates.io publish job excludes `-rc.*` tags (rc tag
  smoke plan: one throwaway `vX.Y.Z-rc.1` exercises publish/OIDC/docker-login
  seams without publishing).

## Affected Crates / Files

- `crates/camel-cli/Cargo.toml` (flavor bodies, alias drop)
- `crates/camel-cli/tests/feature_profiles.rs` (regular/full contract sets,
  golden fixture regen)
- `.github/workflows/release-matrix.yml` (12-entry matrix, dev-profile, docker
  keys, release-job asset pattern, capability asserts)
- `.github/workflows/release.yml` (publish job `-rc.*` guard)
- `.github/workflows/release-dev.yml` (pass dev-profile)
- `docs/src/operations/*` (distribution/flavor documentation, download matrix)

## bd

- rc-5t5fo.5 (this change), epic rc-5t5fo; unblocks rc-5t5fo.7 (post-switch
  verification) and gates rc-5t5fo.8 (binstall, re-ordered to land after this).
