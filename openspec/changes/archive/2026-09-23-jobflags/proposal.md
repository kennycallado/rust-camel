# Proposal: jobflags

## Why

`camel job` accepts job arguments only through `--arg NAME=VALUE`. Users
expect normal app-style flags: `camel job doc.job.yaml --name world`
(see bd rc-eug6b, owner request 2026-09-22). Research is complete and
owner-approved (`.opencode/fleet/inbox/jobflags-research.md`): a clap
4.6.6 two-phase in-process re-parse, no new dependency.

## What Changes

- New front-end: one `--<name> VALUE` flag per argument declared in the
  document's top-level `args:` block. Flag name = declared name
  VERBATIM (underscore spelling, long-only, no dash aliases, no
  inference). Both `--name VALUE` and `--name=value` accepted.
- Bool arguments: bare `--flag` = true, absent = document default,
  `--no-flag` = false. `--flag=false` is a hard error naming the
  accepted spellings. `--flag` and `--no-flag` together = conflict
  error.
- Lowering converges on the existing `Vec<(String, String)>` pair
  channel BEFORE coercion/defaults/required/interpolation — downstream
  pipeline (resolve, coerce, interpolate, report) stays untouched.
  `--name world` and `--arg name=world` are structurally identical
  downstream.
- Conflicts: same key via both forms in one invocation = error naming
  the key (never cross-form last-wins). Same flag twice in one form =
  last-wins (house rule parity).
- Undeclared `--foo` = clap-style unknown-argument error with "did you
  mean" hint, exit 2.
- Dynamic flag on a document WITHOUT `args:` = hard error (exit 2)
  pointing at `args:` / `--arg`; no legacy header fallback, no
  deprecation note for dynamic flags.
- Reserved-name guard: declaring `help`, `config`, `report`, or `arg`
  fails document load (exit 2) on both execution and `--help` paths.
- `--arg KEY=VALUE` keeps working forever (deprecated path, retained).
- `camel job <name> --help`: the `Arguments:` table gains one note
  line documenting the flag spellings.
- Excluded: shell completions for dynamic flags (v1: none), dash-alias
  spelling, short flags, `infer_long_args`.

## Acceptance criteria

- `camel job doc.job.yaml --name world` behaves identically to
  `--arg name=world` (interpolation, coercion, report, exit code);
  bare-name invocation `camel job myjob --name world` works too.
- Every bool spelling pinned by tests, including the rejected
  `--flag=false` form naming the correct spellings.
- Reserved collision, no-`args:`-block error, cross-form conflict,
  undeclared-flag hint: all pinned, all exit 2.
- Back-compat: existing invocations without dynamic flags behave
  identically; `--arg` after the document path keeps working.
- Full gates green (fmt, clippy -p camel-cli -D warnings, cargo test
  -p camel-cli --lib + affected integration tests, workspace build).

## Risk budget

Highest risk is the clap `trailing_var_arg` ordering sharp edge
(static flags after the document path). Resolved by an empirical
clap 4.6.6 probe, facts frozen into the design: static flags after
the path parse statically; from the first unknown token the raw tail
begins, and `--arg`/`--help`/`--report` in the tail are recovered by
the tail re-parse while a tail `--config` is honored by an exact-token
pre-scan before config load (last argv occurrence wins). No
runtime/pipeline crates touched; blast radius is `camel-cli`'s job
module plus its tests. Byte-exact clap diagnostics are pinned with a
clap-version note so bumps surface drift loudly.
