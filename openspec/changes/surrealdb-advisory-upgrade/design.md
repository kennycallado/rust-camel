# Design: surrealdb-advisory-upgrade

## Approach

Single-phase dependency-security change: move the workspace `surrealdb`
requirement from `"3"` to `"4"`, regenerate `Cargo.lock`, verify nothing
breaks, record evidence, close the audit-gate debt (bd rc-yhpyz).

**Version decision (probe-verified at mission start, 2026-11-06).**
crates.io offers `3.3.5` (clears 0453 only) and `4.0.0` released 2026-11-03
(clears both 0453 via `affinitypool` 0.8 and 0454 via the new `reblessive`).
The order prefers a single bump that clears both. A throwaway probe applied
the bump in this worktree and produced decisive facts:

- `sed` the workspace requirement to `"4"`, `cargo update -p surrealdb` →
  lock resolves `surrealdb 4.0.0` / `surrealdb-core 4.0.0` /
  `affinitypool 0.8.0`; all four existing feature keys
  (`protocol-ws`, `protocol-http`, `rustls`, `kv-mem`) still exist in 4.0.0
  (cargo would hard-error otherwise).
- `cargo check -p camel-component-surrealdb` → **0 errors** (component's
  default target).
- `cargo check -p camel-test -p surrealdb-example -p camel-bundles
  --all-targets` → **0 errors**. The component's used surface —
  `engine::any::{connect, Any}`, `Surreal`, `opt::auth::Root`,
  `types::{Action, Value, Object}`, `Error::{internal, query, connection}`,
  `types::QueryError::{TransactionConflict, TimedOut}` — compiles under the
  4.0.0 SDK for everything this repo touches. This proves compilation only:
  the feature-gated `camel-test` SurrealDB fixtures
  (`integration-tests` feature, Docker-backed, server fixture pinned
  `v3.1.4`) do not compile under default features and remain **unverified by
  the probe** — they are covered by the dedicated runtime gate below.
- `cargo audit` on the probed lock → **0 vulnerabilities**; the 7
  pre-allowed warnings are unchanged. No `audit.toml` edit, no follow-up bd
  needed.

The probe was then reverted (`Cargo.toml`/`Cargo.lock` restored); the real
bump is STAGE 3 work. Consequence for tasks: **the migration is a
lockfile-class change, not a code migration**; the only sanctioned code edits
are ones a failing verification forces, each recorded in evidence.md.

**Verification strategy.** After the bump: workspace-wide `cargo check
--all-targets`; then the full gate battery, run foreground inside the
contained systemd scope (`systemd-run --user --scope --collect`, `MemoryMax=12G`,
`TMPDIR=/home/shared/tmp`, `CARGO_BUILD_JOBS=6`), never `cargo test
--workspace`:

1. `cargo build --workspace`
2. `cargo test -p camel-component-surrealdb --all-targets`
3. `cargo test -p camel-test --all-targets`
4. `cargo test -p camel-test --features integration-tests --test
   surrealdb_test` — the Docker-backed runtime gate; real executed tests
   required, not a zero-test success (Docker is available)
5. `cargo test --workspace --lib`
6. `cargo fmt --check --all`
7. The AGENTS.md clippy invocation (workspace set + per-crate extras)
8. The full `cargo xtask lint-*` battery
9. `cargo audit` — the point of the mission
10. `cd docs && mdbook test` if `docs/**` changed

and the Docker SurrealDB suite (item 4) because compile-clean cannot prove
runtime compatibility of a 3-day-old SDK major (query result shapes,
LIVE-select event payloads, and auth handshakes are the realistic drift
points). If a test fails on a behavior change, the change is either (a) an
in-scope adapter fix in the component, recorded with the reason, or (b) an
ecosystem behavior change that requires a documented-behavior update — then
the crate `CONTEXT.md` and the `docs/src/components/surrealdb.md` prose move
in the same change, and `doc-guide-test` runs.

**Evidence discipline.** `evidence.md` in this change dir records: audit
before (2 vulnerabilities) / after (0), `cargo tree -d --target all` inverted
trees for `surrealdb` / `jsonwebtoken` / `ed25519-dalek`, exact post-bump
versions, the lock diff summary (the precise transitive delta the bump
forces), and per-test change rationale. Base commit recorded in the park.

## Affected crates

- Root `Cargo.toml`: the one-line requirement change (`"3"` → `"4"`).
- `Cargo.lock`: surrealdb subtree resolution (transitives move with it —
  `ammonia`, `object_store`, `diskann-*`, `html5ever`, `string_cache`,
  `crossbeam-*` patch steps; enumerated in evidence).
- `camel-component-surrealdb` / `camel-test` / `camel-bundles` /
  `surrealdb-example`: compile-verified clean; edits only if a gate forces.
- `CHANGELOG.md`: one `### Changed` entry (security/dependency class).

## Architecture boundaries

No boundary moves. The component keeps its public surface (endpoint config,
producer/consumer contracts, error taxonomy) — the SDK boundary it consumes
(`engine::any` + typed `types`) is unchanged, so Runtime/DSL/Components/
Services seams are untouched. Data/control-plane split unaffected. The
change lives at the manifest + lock level plus verification evidence.

## Alternatives considered

- **3.3.5 step-bump + scoped `audit.toml` ignore for 0454** (the order's
  fallback): rejected — 4.0.0 compiles clean and audits clean, so the
  fallback's residual advisory and ignore entry would be pure debt.
- **Stay on 3.2.4 + ignores for both advisories**: rejected — dishonest
  gate, banned by the order ("never ignore 0453").
- **Bump to 3.3.5 now, 4.0.0 later in a dedicated mission**: rejected —
  doubles the audit-debt window for no risk reduction; the probe shows the
  4.x migration cost for this repo is at the lockfile level.

## Phases

Single-phase: bump → verify → document. No milestone grouping; tasks.md has
no `## Phase N` headings.
