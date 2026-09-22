# Proposal: jobpath

## Why

`camel job` listing recurses (depth 8) and advertises nested jobs as
`relative/path: stem`, but those names cannot be used: separator-bearing
arguments are treated as CWD-relative explicit paths and never probed
against `[jobs].dirs` roots, while bare names probe root level only.
The listing teaches spellings the resolver rejects (bd rc-r63b1,
owner-verified UX inconsistency).

## What Changes

- `resolve_job_path` gains an ordered resolution ladder for document
  arguments. The existing explicit classification (path separator, or
  `.yaml`/`.yml`/`.json` suffix) is kept and drives the ladder:
  1. absolute arguments are used as-is (no probing, unchanged);
  2. an explicit-class argument that exists relative to the CWD is
     used as-is (back-compat: explicit paths win);
  3. on a CWD miss, every configured root is probed with one probe
     per root, joined verbatim (no normalization, no confinement):
     suffix-bearing arguments probe `<root>/<argument>`, separator-
     bearing stem paths probe `<root>/<argument>.job.yaml`.
  Bare names keep today's behavior byte-for-byte: root-level
  `<root>/<name>.job.yaml` append probing only, never consulting the
  CWD.
- Cross-root collision on a relative path keeps the explicit
  all-matches error; a miss names every probed file (existing error
  styles extended).
- Nested listing display becomes the configured-root-relative path
  verbatim (the `: stem` descriptive suffix is dropped) — exactly the
  spelling that now resolves. Root-level display (bare stem) is
  unchanged.
- `camel job <name> --help` header shows the same invocable display
  name (root-relative path for nested documents, file stem otherwise),
  via one shared display-name construction so listing and help cannot
  drift.
- Affected crate: `camel-cli` (`commands/job/{mod.rs,help.rs}`,
  `help_tests.rs`, integration tests `job_one_shot_test.rs`,
  `job_signal_test.rs`). Delta spec: `cli-jobs`.

Excluded: bare-name descent into subdirectories (`ingest` still never
resolves `daily/ingest.job.yaml`), alternate-suffix probing for bare
names (`.job.yml` stays display-only), and any change to `[jobs].dirs`
configuration semantics.

## Acceptance criteria

- `camel job daily/ingest` and `camel job daily/ingest.job.yaml` both
  resolve `<root>/daily/ingest.job.yaml` and run (exit 0).
- An existing CWD-relative path argument still wins over root probing.
- Cross-root collision on a relative path exits 2 naming every match.
- A relative-path miss exits 2 naming every probed file.
- Listing display for nested jobs equals the invocable spelling;
  tests pin both display and invocation. Root-level rows unchanged.
- Bare-name resolution is unchanged in behavior and diagnostics (bare
  names never consult the CWD and never descend below a root).

## Risk budget

CLI-surface-only change inside `camel job`; no runtime, DSL, or
component impact. Acceptable risk: diagnostics for nonexistent
suffixed arguments change from the canonicalize error to the miss
error naming probes (both exit 2). Out of bounds: touching `[jobs]`
config parsing, route discovery, or the exit-code taxonomy.
