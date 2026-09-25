# Design: cfgempty

## Context

`--config` has three consumption shapes in camel-cli:

1. `camel run` (main.rs `Run.config: String`, clap
   `default_value = "Camel.toml"`, `env = "CAMEL_CONFIG_FILE"`) →
   `run()` → `load_config_or_default(&config_path)` +
   `canonical_project_root(Path::new(&config_path))`.
2. `camel job` (job/mod.rs `JobArgs.config: String`, same clap
   default/env) with a tail `--config` override
   (`tail_config_override`) — argv last-wins — → the same
   `load_config_or_default` + `jobs_roots(&config_path, …)` +
   `canonical_project_root`.
3. `camel compile` (compile.rs `config: Option<PathBuf>`, no default) →
   `sources::resolve` with `Selection.config_path`.

Empty-value behavior today (reproduced on this worktree's build):

- run/job: `Path::new("").try_exists() == Ok(false)` →
  `in_memory_default_config()` (silent default); root resolution
  canonicalizes `.` (CWD). `camel job --config=` in an empty directory
  exits 0 listing nothing — indistinguishable from a sane default run.
- compile: `Some("")` cannot arrive via the CLI — clap's
  `PathBufValueParser` rejects empty values at parse (exit 2, flag
  named). Programmatically, `Some("")` would reach
  `resolve_named("")` → `canonicalize()` fails →
  `SourceError::MissingSource("")` — loud but the diagnostic is an
  empty string that does not name the flag.

## Goals / Non-Goals

- Goals: an explicitly-passed empty `--config` value fails early with a
  clear error naming the flag, on all three subcommands (flag, tail
  override, and env spelling included); omitted flag and valid paths are
  byte-identical to today.
- Non-Goals: changing the absent-file silent-default rule (missing
  non-empty path still falls back — that is documented, intended
  behavior); validating that non-empty values point at files that exist;
  touching config-loader semantics in camel-dsl/camel-config.

## Decisions

### D1 — Guard lives in `load_config_or_default` (run + job)

Mirror the existing `CamelError::Config` error path already used for
"failed to check config path" / "failed to load" — first statement of the
function:

```rust
if config_path.is_empty() {
    return Err(camel_api::CamelError::Config(
        "--config requires a non-empty path; pass a Camel.toml path or omit the flag"
            .to_string(),
    ));
}
```

Rationale: one choke point covers `camel run`, `camel job` (phase-1
flag, tail override, `CAMEL_CONFIG_FILE=""`), and every existing test
helper caller. It is the config-resolution boundary the mission order
names. Error surfaces follow each command's existing failure path
(`camel run` → `CamelError` out of `run()`; `camel job` →
`camel-cli job failed: …`, exit 2).

Rejected: a clap `value_parser` — run/job `--config` deliberately accept
non-existent paths (absent-file fallback is intended), and a parser-level
guard would also mutate help/env semantics; the function-level guard is
the narrowest change at the boundary the order names.

### D2 — Same-class guard at compile's selection boundary

`camel compile` stays loud; add the identical empty-value rejection
where the selection is consumed (compile.rs, before
`sources::resolve`), printing the same message via the existing
`eprintln!` + `EXIT_REJECTION` (exit 2) path. The guard is
CLI-unreachable (clap's `PathBufValueParser` already rejects empty
values, naming the flag) — it exists for programmatic `CompileArgs`
callers, replacing the confusing `MissingSource("")` diagnostic with
one that names `--config`. Pinned in-process by exit code (stderr has
no capture seam).

### D3 — Env spelling covered for free

`CAMEL_CONFIG_FILE=""` flows into the same `config: String` for run/job
(the env var's value becomes the flag value), so D1 rejects it with the
same message. No separate env guard.

## Risks / Trade-offs

- Scripts that today "work" by passing `--config=` get a hard error.
  That is the intended fail-early change; the message tells them to omit
  the flag.
- `load_config_or_default` unit tests that pass empty strings would now
  error — checked: existing tests pass only non-empty paths.

## Migration Plan

None (error UX fix; no config format or API change).

## Open Questions

None.
