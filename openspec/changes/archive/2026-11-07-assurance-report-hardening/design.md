# Design: assurance-report-hardening

## Approach

Three pieces, one coherent slice:

1. **`scripts/assurance-report-body.sh`** — the body builder, extracted from the
   workflow's "Build issue body" step. Style follows
   `scripts/verify-release-assets.sh` (header comment with bd reference and
   failure story, `set -euo pipefail`, explicit usage error, env knobs).
   Interface (env vars, matching the workflow's GITHUB_ENV pattern):

   - `ARTIFACTS_ROOT` (default `.`) — where artifacts were downloaded
   - `FUZZ_RESULT` / `BASE_RESULT` / `ROT_RESULT` — needs-context job results
     (rendered verbatim; `unknown` when unset)
   - `FUZZ_SECS` (default `900` — workflow-level env exists in CI; default
     keeps offline runs stable)
   - `OUT_BODY` (required) — issue body output path
   - `OUT_ENV` (required) — env file the workflow sources; the script appends
     `OUT_BASE`, `OUT_ROT`, `FUZZF`, `INFRA_MISSING` (space-separated missing
     keys, empty when all evidence present), and `STAMP`

   `OUT_ENV` contract note (bless finding 2): values appended to a
   `$GITHUB_ENV` file become environment variables for **subsequent steps
   only** — never for the step that ran the script. Consumers (`STAMP` for the
   issue title, `INFRA_MISSING` for the infra guard) therefore live in steps
   *after* body construction; the evidence-path keys (`OUT_BASE`, `OUT_ROT`,
   `FUZZF`) are emitted for debuggability only — the publish diff consumes the
   body file directly.

   Evidence lookup never fails the script: every `find` runs with
   `2>/dev/null || true`. Missing evidence renders explicit
   `MISSING OUTCOMES (infra failure)` / `MISSING FUZZ FINDINGS (infra failure)`
   / `MISSING ROTATION RATIONALE (infra failure)` sections and populates
   `INFRA_MISSING`. Only three real-error paths exit non-zero, each with an
   actionable stderr message: missing/unwritable output paths (exit 2, usage),
   unwritable body/env target (exit 3), and malformed JSON in a *present*
   `outcomes.json` that makes `jq` fail (exit 4). No `gh`, no network — issue
   creation and the prev-issue diff stay in the workflow.

   The body contract is preserved verbatim: `# assurance-monthly:` header,
   job-results table, fuzz section, per-file mutant sections with jq counts,
   triage checklist, rotation section, candidates line
   (`.github/assurance-monthly.json` read from `ARTIFACTS_ROOT`, tolerant when
   absent), and the `<!--baseline-data-start-->` / `<!--baseline-data-end-->`
   machine-readable block with sorted `.name` lines — byte-identical form.

2. **Workflow `report` job** — minimal diff: a comment on the download step
   documenting the verified choice (bare download-all form of
   `actions/download-artifact@v8.0.2` tolerates zero artifacts — the source at
   the pinned tag lists, downloads 0, logs `Total of 0 artifact(s) downloaded`,
   exits success; verified, not guessed); a comment block explaining the
   extraction and the 41999707610 post-mortem; body construction replaced by
   one `bash scripts/assurance-report-body.sh` call with
   `OUT_ENV="$GITHUB_ENV"`; issue publication split into its own
   **"Publish issue"** step (needs `GH_TOKEN`; consumes `STAMP`, `OUT_BASE`
   from the body step's env file — subsequent-step semantics), so `gh issue
   create` now always runs (zero-artifacts body still gets published — that is
   the alarm); the existing "Fail only on infra (missing evidence)" step flips
   on `INFRA_MISSING`.
   The publish step fixes the second latent abort (bless finding 1): the
   current-survivors pipeline `sed -n ... | grep -v '<!--'` exits 1 under
   `set -euo pipefail` when the marker block holds no survivor lines (empty
   baseline set), which killed publication whenever a previous issue existed.
   The pipeline gains `|| true` with a comment — an empty survivor set is
   valid input to the diff, not an error. `gh` usage stays in the workflow.
   Order: download → body script → publish issue → infra guard, so the guard
   can never again pre-empt the alarm.

3. **`scripts/tests/assurance-report-body-test.sh`** — offline battery in the
   `verify-release-assets-test.sh` convention: temp root, fixture builders,
   `t_*` functions, runner loop with PASS/FAIL counting. Scenarios:
   all-present, baseline-missing, fuzz-missing, rotation-missing,
   zero-artifacts (exit 0, header + three results + three MISSING lines),
   marker preservation with sorted `.name` lines, malformed present
   `outcomes.json` (non-zero + message), unwritable body target (non-zero),
   usage errors. Plus **publish-path coverage** (bless finding 1/2): the
   battery extracts the workflow's "Publish issue" run block (by step name,
   from the checked-out YAML) and executes it under `bash` with a stubbed
   `gh` on PATH and `STAMP`/evidence paths set as a subsequent step would see
   them: (a) previous issue exists + zero-artifacts body → reaches
   `gh issue create` with a populated `--title`, survivor-diff section
   present, exit 0; (b) previous issue exists + all evidence present but zero
   baseline survivors → same; (c) no previous issue → straight create, no
   diff section. These prove the wiring check (title populated without
   same-step `$GITHUB_ENV` propagation) and the empty-survivor tolerance.

## Affected crates

None. No `*.rs`, no `Cargo.toml`, no `Cargo.lock`.

## Architecture boundaries

No runtime/DSL/component boundaries touched. The change is confined to CI
reporting infrastructure (`.github/workflows/assurance-monthly.yml`, `scripts/`)
and its offline test harness. The anti-obstruction contract of the assurance
workflow (findings never fail the run; only missing evidence does) is preserved
and now enforced by a testable script instead of inline YAML.
