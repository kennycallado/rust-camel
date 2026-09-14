# Jobs discovery

The `[jobs]` table in `Camel.toml` configures where the `camel job` command looks for job documents. It is an operator-tool surface, separate from route discovery by design: `[routes]` entries are globs for the always-on data plane; `[jobs]` entries are discovery roots for a bounded directory walk.

```toml
[jobs]
dirs = ["jobs", "src/jobs"]
```

## Keys

- `dirs` (list of strings): ordered discovery roots. Each entry is a directory path resolved against the `Camel.toml` root. The walk is bounded (depth and file caps). The default is `["jobs"]`. An explicit empty list, `dirs = []`, means "no discovery roots": `camel job` finds no documents.
- `dir` (string, legacy): compatibility alias from before the list form. It folds into a one-element `dirs` list. When both keys are present, `dirs` wins and `dir` adds no duplicate root. Prefer `dirs` in new configurations.

Entries are literal directory paths, not globs. Do not use `src/**/*.yaml` patterns here; use the directory that contains the job documents.

## Profiles

`[jobs]` follows the standard profile rules: a `[<profile>.jobs]` section merges on top of `[default.jobs]` (or top-level `[jobs]`), and table merges are additive.

Use the same key at both levels. If one profile sets `dirs` and another sets `dir`, the merged table carries both keys and `dirs` takes precedence — the profile-level `dir` is ignored without a warning:

```toml
[default.jobs]
dirs = ["jobs"]

[production.jobs]
dirs = ["jobs/production"]   # same key: replaces the default list
```

## Discovery behavior

`camel job` walks the resolved roots in order, collecting job documents up to the bounded depth and file count. Discovery order follows the `dirs` list order. Job names derive from the document, per the `camel job` listing contract.

Authority: ADR-0062 (amended 2026-09-13), the `cli-jobs` canonical spec, and the camel-job orientation ruling.
