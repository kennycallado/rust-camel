# Design: jobpath

## Approach

One function owns resolution today: `resolve_job_path` in
`crates/camel-cli/src/commands/job/mod.rs` (single caller: `run_job`).
Its `explicit` classification (separator or `.yaml`/`.yml`/`.json`
suffix → as-is; bare name → append-probe) is replaced by an ordered
ladder, per the owner-approved binding design:

1. **Absolute argument** → returned as-is, no probing (back-compat:
   probing an absolute path against roots is meaningless; the old code
   also treated it as explicit).
2. **Explicit-class CWD existence wins** → an explicit-class argument
   (path separator, or `.yaml`/`.yml`/`.json` suffix — the existing
   classification) that satisfies `raw.exists()` is used as-is. This
   preserves every existing pinned behavior for real explicit paths
   (`explicit_path_wins_over_jobs_resolution`,
   `explicit_job_path_bypasses_roots_and_bare_miss_is_named`). The
   behavior delta vs today: a non-existent explicit path now falls
   through to probing instead of dying at `canonicalize` — the fix's
   point. Bare names are NOT explicit-class, so they never consult
   the CWD: their resolution is byte-identical to today.
3. **Root probe on CWD miss** → one probe per root, joined verbatim
   from the argument as spelled:
   - suffix-bearing arg (`.yaml`/`.yml`/`.json`, case-insensitive) →
     probe `<root>/<arg>` verbatim, so the listing's displayed
     spelling (`daily/ingest.job.yaml`) is invocable verbatim;
   - separator-bearing stem path (no recognized suffix) → probe
     `<root>/<arg>.job.yaml` (bd acceptance: `camel job daily/ingest`
     runs).
   All matches are collected before selection: zero → exit-2 miss
   naming every probed path (existing style); one → resolve; many →
   exit-2 collision naming every matching path (existing style,
   extended to relative paths).

   Bare names keep today's probe exactly: `<root>/<name>.job.yaml`
   append, root level, `.job.yaml` only — `ingest` never resolves
   `daily/ingest.job.yaml`, `.job.yml` stays display-only for bare
   lookups (pinned scenario `listed job.yml is not bare-name
   resolvable` unchanged).

**Display.** `walk_level` builds nested rows as
`<relative-path>: <stem>`; the `: <stem>` suffix is dropped so the row
is the invocable spelling. Root-level rows keep the bare stem. A new
shared helper (`job_display_name(resolved, roots)`: lexical
`strip_prefix` against the configured root path → the relative path
when it has >1 component, else `job_stem` of the file name) drives the
`camel job <name> --help` header for the same spelling — mirroring how
`job_stem` is already shared so listing and help "cannot drift".
`render_job_help`'s signature is unchanged; its `stem` parameter now
receives the display name; `help_tests.rs` gains a pinning test passing
a relative path.

**Doc comments** on `resolve_job_path`, `ListedJob`, `JobArgs.document`,
and the module header are updated to the new ladder (they currently
document root-level-only probing).

## Affected crates

- `camel-cli`: `src/commands/job/mod.rs` (resolution ladder, nested
  display, shared display-name helper, doc comments), `src/commands/
  job/help.rs` (header doc comment only), `src/commands/job/
  help_tests.rs` (display-name pin), integration tests
  `tests/job_one_shot_test.rs` + `tests/job_signal_test.rs` (updated
  display pins + new resolution tests).

## Architecture boundaries

CLI boundary only (`camel job` command surface). No Runtime, DSL,
component, or config-loader change: `jobs_roots`, the bounded walker,
description probing, and the exit-code taxonomy are untouched. The
delta spec stays inside `cli-jobs` (listing + named resolution +
job-scoped help requirements).

## Decisions

- Bare names never consult the CWD (e_gpt spec-bless round 1): the
  CWD-existence branch is restricted to explicit-class arguments
  (separator or recognized suffix — the existing classification), so
  bare-name resolution is byte-identical to today and cannot be
  shadowed by a CWD file or directory that happens to share the name.
- Suffix-bearing args probe verbatim rather than appending: appending
  would make the displayed spelling (`daily/ingest.job.yaml`) resolve
  to the nonsense probe `<root>/daily/ingest.job.yaml.job.yaml`, and
  the miss diagnostic would name that nonsense path. Verbatim probing
  keeps "listing display == invocable spelling" exact.
- Probing performs no normalization and no confinement: probes are
  plain `Path::join`s of the argument as spelled, so `.`/`..`
  components, trailing separators, and any other spelling probe
  exactly as joined and appear verbatim in miss diagnostics. The
  command already loads unrestricted explicit paths (any relative or
  absolute path), so probing adds no new reach; a confinement filter
  would be a silent behavior change and a diagnostic liar.
- Display-name matching is lexical, on the resolved spelling BEFORE
  canonicalization: the walker lists `root.join(relative)` and the
  prober resolves `root.join(probe)`, so both strip the same root
  path strings and cannot drift within one spelling. Symlink
  aliasing is deliberately NOT identity-resolved — a document reached
  through a different spelling (for example an absolute real path
  behind a symlinked file) may render its file stem; that is
  deterministic per spelling, and canonicalize happens only after the
  display name is fixed.
- CWD check uses `exists()` (not `is_file()`), matching "if the path
  exists relative to CWD" literally; a directory argument still fails
  loud at the read step (exit 2).

## Phases

Single-phase: one resolver + display slice, one spec area, small
bounded diff. No `## Phase N` headings in tasks.md.
