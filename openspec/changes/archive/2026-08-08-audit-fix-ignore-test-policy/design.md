# Design: audit-fix-ignore-test-policy

## Approach

Four coordinated deliverables, each grounded in the project's existing
architecture:

### 1. ADR-0054 — the policy decision

Follows the ADR-0012/0049 shape: a policy ADR paired with an xtask lint.
Defines a closed vocabulary of TWO `#[ignore]` reason prefixes:

| Prefix | Category | CI gate |
|--------|----------|---------|
| `requires pre-built <artifact>` | buildable artifact (WASM guests/fixtures) | MUST run in CI |
| `slow test: <description>` | slow but self-contained | excluded from PR gate |

External-service tests (Kafka, Redis, OpenSearch, K8s, etc.) are NOT a
valid `#[ignore]` category. The project's established pattern is:
integration tests live in `camel-test` behind
`--features integration-tests`, self-provision dependencies via
testcontainers, and run in CI's `full-tests-linux` job. This is the same
pattern used for JMS/XML/CXF bridges and containers.

Rule: a test may only be `#[ignore]` for a prerequisite CI cannot
cheaply satisfy. Buildable artifacts are cheaply satisfiable
(`wasm32-wasip2` is in `flake.nix`), so such tests MUST run in a
dedicated CI job.

### 2. `cargo xtask lint-ignore` — the enforcement

Source-scanning lint added to `scripts/xtask/src/main.rs` as
`Commands::LintIgnore`.

**Scanner scope:** scans all `.rs` files under `crates/` and
`examples/`, excluding `target/`, `.worktrees/`, `scripts/`, and
`bridges/`. Test files under `tests/` ARE scanned (unlike
`lint_log_levels` which skips them).

**Checks:**
- Rejects bare `#[ignore]` (no `= "..."`)
- Extracts reason string and checks against the two-prefix vocabulary
- `requires live` is treated as a **migration error** — the lint emits
  `ignore:migration-error:` with a message pointing contributors to
  `camel-test` + `--features integration-tests` + testcontainers
- Near-prefix typos are rejected — exact prefix boundary required
- For `requires pre-built` prefix: bidirectional allowlist coupling
- **No escape hatch.**

**Allowlist coupling (bidirectional):**
- `scripts/xtask/allowlist-ignore.txt` lists test files containing
  `requires pre-built` tests.
- Forward check: every `requires pre-built` test must be in allowlist.
- Reverse check: every entry must be a direct-child `.rs` file under
  `crates/components/camel-component-wasm/tests/`, exist on disk, and
  contain at least one `requires pre-built` test.
- Mixed-reason check: all `#[ignore]` in an allowlisted file must use
  `requires pre-built`.

### 3. CI `wasm-integration` job — WASM coverage

New job in `.github/workflows/ci.yml`:

1. Checkout + rust-toolchain with `targets: wasm32-wasip2`
2. Build all WASM guest crates (6 guest dirs under `examples/`)
3. Tests resolve guest `.wasm` from the Cargo target directory — no copy
4. Derive test targets from `allowlist-ignore.txt` and run
   `cargo test -p camel-component-wasm --test <target> -- --ignored`

### 4. CI integration tests — external-service coverage

The existing `camel-test` testcontainers integration tests are wired
into the `full-tests-linux` CI job:
- `kafka_test` (4 tests, KRaft Kafka via testcontainers)
- `redis_test` (9 tests, Redis via testcontainers)
- `opensearch_test` (14 tests, OpenSearch via testcontainers)
- `kubernetes_test` (4 tests, K3s via testcontainers)

These tests already existed but were not wired into CI. This change
connects them, turning a coverage gap into a coverage gain.

### 5. Duplicate test cleanup

7 inline `#[ignore]` tests in component crates that duplicated existing
camel-test coverage have been DELETED:
- kafka `src/lib.rs`: 2 tests (superseded by `kafka_test.rs`)
- redis `src/lib.rs`: 3 tests (superseded by `redis_test.rs`)
- opensearch `tests/live_opensearch.rs`: 1 test (superseded by
  `opensearch_test.rs`, file deleted)
- k8s `readiness_gate.rs`: 1 test (superseded by `kubernetes_test.rs`)

### 6. Remaining `#[ignore]` normalization

- 8 Ollama tests: `requires local Ollama...` → `slow test: requires
  local Ollama with <model>`. Genuinely special — 4B model pull is
  GB-scale and not cheaply provisionable per-PR.
- 1 camel-file test: bare `#[ignore] // Slow test` → `slow test: file
  polling`.
- 13 WASM tests: already used `requires pre-built` prefix, verified
  compliant.

## Affected crates

- `scripts/xtask`: `LintIgnore` command + `lint_ignore()` function
- `crates/components/camel-component-wasm/tests/`: reason normalization
- `crates/components/camel-component-llm/tests/ollama_live.rs`: Ollama
  reason normalization
- `crates/components/camel-file/src/lib.rs`: reason normalization
- `crates/components/camel-kafka/src/lib.rs`: deleted duplicate tests
- `crates/components/camel-redis/src/lib.rs`: deleted duplicate tests
- `crates/components/camel-opensearch/tests/live_opensearch.rs`: deleted
- `crates/platforms/camel-platform-kubernetes/src/readiness_gate.rs`:
  deleted duplicate test
- `docs/adr/`: ADR-0054
- `AGENTS.md`: `lint-ignore` in QUALITY GATES
- `.github/workflows/ci.yml`: `lint-ignore` in quality job, new
  `wasm-integration` job, integration tests in `full-tests-linux`

## Architecture boundaries

This change is entirely meta-tooling, CI plumbing, and test cleanup. It
does not touch Runtime, DSL, Components (runtime logic), Services,
Languages, or Functions. Test bodies are NOT modified — only `#[ignore]`
reason strings change, and duplicate tests are deleted (not rewritten).

## Alternatives considered

- **Three-prefix vocabulary with `requires live`:** rejected — blesses
  an anti-pattern the project already solved with testcontainers.
- **Do nothing:** rejected — leaves the WASM CI gap open.
- **Docker-compose for live services:** rejected — testcontainers
  already provides superior self-provisioning.
- **Delete `#[ignore]` from buildable tests:** rejected — breaks local
  dev without wasm target.
- **Escape hatch:** rejected — a closed vocabulary with an escape hatch
  is not closed.
