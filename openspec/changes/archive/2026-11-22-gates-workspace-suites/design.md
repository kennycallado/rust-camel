# Design: gates-workspace-suites

## Approach

The lane registry in `AGENTS.md ## COMPONENT LANES` is the single source of
truth; `scripts/gates-local.sh` parses rows out of it (splitting on the first
two `|` only) and executes each command byte-identically via
`bash -o pipefail -c` from the repo root. So the narrowest implementation is
one new declared row — no parser or dispatch changes:

```
camel-test | local | suites="${CAMEL_TEST_WORKSPACE_SUITES-dev_watch_script_test dev_watch_proto_test}"; allowed="dev_watch_script_test dev_watch_proto_test"; [[ -n "${suites//[[:space:]]/}" ]] || { echo "camel-test workspace lane: empty CAMEL_TEST_WORKSPACE_SUITES suite list" >&2; exit 2; }; set -f; for s in $suites; do case "$s" in *[\*\?\[]*) echo "camel-test workspace lane: CAMEL_TEST_WORKSPACE_SUITES must list literal suite names, got '$s'" >&2; exit 2;; esac; ok=0; for a in $allowed; do if [[ "$s" == "$a" ]]; then ok=1; break; fi; done; if [[ "$ok" != 1 ]]; then echo "camel-test workspace lane: CAMEL_TEST_WORKSPACE_SUITES entry '$s' is not an allowlisted local suite (allowed: $allowed)" >&2; exit 2; fi; done; rc=0; for s in $suites; do cargo test --workspace --test "$s" || rc=1; done; exit "$rc"
```

Properties:

- **Feature unification**: `cargo test --workspace --test <suite>` selects the
  whole workspace as the package set, so unified features
  (`camel-core/lang-rhai` via `camel-cli` / `camel-integration-test`) turn on
  — exactly the configuration where rc-lhd0y panicked. Per-suite invocation
  keeps one bad suite from masking another (`rc` aggregates).
- **Explicit suite list, allowlisted**: the default list (`dev_watch_script_test
  dev_watch_proto_test`, the suites the rc-lhd0y fix commit 8393998ad
  touched) lives in the row. `CAMEL_TEST_WORKSPACE_SUITES` overrides it, but
  only within the row's `allowed` allowlist (initially the same two reviewed
  local suites) — so the override can subset or reorder, but can never pull
  a Docker-classified suite (e.g. `redis_state_test`, whose `redis` feature
  starts a container) into a local run: the validation pass rejects it
  before any `cargo test` dispatch. Adding a suite to the local set is a
  reviewed, two-name edit of one table row.
- **Fail-closed**: empty AND whitespace-only lists → stderr message naming
  `CAMEL_TEST_WORKSPACE_SUITES` + exit 2 before any cargo test dispatch (the
  guard strips whitespace before the emptiness check, so `'   '` fails
  identically to `''`); entries containing glob metacharacters (`*`, `?`,
  `[`) are rejected by a validation pass before any cargo test dispatch, so
  `--test` always receives a literal suite name and cargo's multi-target
  glob semantics can never silently widen the run; `set -f` in the row
  disables pathname expansion so an unquoted `$suites` can never expand
  metachars to matching cwd filenames before validation; entries outside the
  in-row `allowed` allowlist (e.g. the Docker-classified `redis_state_test`)
  are rejected in the same pass — the override knob cannot cross the
  local-only boundary. The row runs in its own `bash -c`, so nothing leaks
  into the sweep's shell.
- **Declared lane**: because the row exists, a change touching `camel-test`
  never trips the NO LANE fail-safe, and `GATES_SKIP=lanes` (or a
  `camel-test` entry in `GATES_SKIP`) skips it like any lane.
- **Scope**: not full `cargo test --workspace` — Docker and native-bridge
  suites stay CI-owned; the list is three names long at most.

Suite-set evidence (not guessed): rc-lhd0y's fix commit `8393998ad` touches
exactly `crates/camel-test/tests/dev_watch_script_test.rs` (rhai registration
guard; the file imports `camel_language_rhai::RhaiLanguage` directly) and
`crates/camel-test/tests/dev_watch_proto_test.rs` (rc-850w7, generation-counter
wait, same workspace-unification failure run). `dev_watch_template_test.rs`
was not part of that fix set and exercises the template path, not rhai.

`scripts/gates-local.sh` gets a header-comment line pointing at the env var;
`scripts/gates-lanes-test.sh` gains `case_49`, which **exercises the
production declaration itself**: the sub-cases copy the REAL repo AGENTS.md
into the fixture repo (with a guard that the `suites=` row is present inside
the fenced table — if the row is deleted or moved outside the fence, the
fixture sweep dispatches no `--workspace` calls and the case goes red), and
expected argv is literal/independent of the row text. Pinned: exact ordered
argv for the default list (plus the table's compile-only `--no-run` row,
which legitimately dispatches alongside), the allowlisted subset override,
rejection of blank/wildcard/non-allowlisted (Docker-classified) overrides
(zero `--workspace` calls), failure aggregation (first suite failing still
runs the second, sweep fails), `GATES_SKIP=lanes` disabling, and a malformed
twin row derived from the extracted production text and rejected by the
parser with file+line. The stub cargo gains one guarded failure mode
(`STUB_TEST_FAIL_AT=<k>`: the k-th non-metadata invocation logs then exits
1) — inert while unset, so the existing 48 cases are unaffected.

## Affected crates

- none: `scripts/gates-local.sh`, `scripts/gates-lanes-test.sh`,
  `AGENTS.md`, and the OpenSpec change dir only. `camel-test` is the lane's
  subject, never an edit target. If implementation finds itself editing a
  crate source file, that is a scope breach → stop and park `blocked: scope`.

## Architecture boundaries

Process/tooling boundary only. The change touches no Runtime, DSL,
Component, Service, Language, or Function code; it adds observability of one
existing runtime property (feature unification) to the local gate set.
Consistent with ADR-0076's split: normative gate behavior lands in
`openspec/specs/` (this delta), the registry row in `AGENTS.md` stays the
executable source of truth.
