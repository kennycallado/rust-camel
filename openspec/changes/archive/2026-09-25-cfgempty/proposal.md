# Proposal: cfgempty — reject an explicitly empty `--config` value

## Why

`camel job --config=` (empty value; same for `camel run --config=`) is a
silent near-miss: the user typed the flag, the value is empty, and the CLI
pretends nothing happened. `load_config_or_default` hits the
`try_exists() == Ok(false)` branch for `""`, returns the in-memory default
config, and `try_canonical_project_root("")` canonicalizes `.` — so the
jobs roots anchor to the process CWD. A typo (`--config=` from a broken
shell assignment, a `--config="$CFG"` with unset `CFG`) looks exactly like
a successful default boot. bd rc-4l2h8.

## What Changes

- `load_config_or_default` (camel-cli `commands/run.rs`) gains an
  explicit-empty guard: an empty `config_path` returns
  `CamelError::Config` with a message naming `--config`. This is the
  single consumption choke point shared by `camel run` and `camel job`
  (both the phase-1 flag and the tail `--config` override, plus an empty
  `CAMEL_CONFIG_FILE` env value on both subcommands).
- `camel compile --config=` never reaches resolution: clap's
  `PathBufValueParser` rejects the empty value at the parse boundary
  (exit 2, flag named). Its command entry nonetheless gets the same
  empty-value guard as defense-in-depth for programmatic `CompileArgs`
  construction, so the error names `--config` instead of an opaque
  empty-source diagnostic on that path.
- No behavior change for: omitted flag (default `Camel.toml` discovery
  unchanged), explicit valid path (loads that file), explicit non-empty
  missing path (unchanged silent-default rule of rc-… — the documented
  absent-file fallback).

## Capabilities

No spec delta: no existing requirement in `openspec/specs/` governs the
`camel run`/`camel job` `--config` flag's value-validation or fallback
boundary (checked: `config-loader-semantics` covers camel-dsl canonical
helpers and the compile include-chain walk; `cli-jobs`/`cli-startup`
cover boot/signal/listing behavior, not flag value validation). Per
mission order scope item 5: no requirement exists → no delta.

## Impact

- `crates/camel-cli` only (`commands/run.rs`, `commands/compile.rs`,
  tests).
- Affected specs: none (no delta).
- Docs: none (error UX, no documented surface changes).
