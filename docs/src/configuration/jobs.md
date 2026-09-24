# Jobs discovery

The `[jobs]` table in `Camel.toml` configures where the `camel job` command looks for job documents. It is an operator-tool surface, separate from route discovery by design: `[routes]` entries are globs for the always-on data plane; `[jobs]` entries are discovery roots for a bounded directory walk.

In a config with profiles, place the table under the profile sections:

```toml
[default.jobs]
dirs = ["jobs", "src/jobs"]

[production.jobs]
dirs = ["jobs/production"]
```

A top-level `[jobs]` table works only in a flat config, one without `[default]` or profile sections. When `[default]` exists, profile application replaces the whole root with the merged `[default]` tree, and a top-level `[jobs]` is silently dropped.

## Keys

- `dirs` (list of strings): ordered discovery roots. Each entry is a directory path resolved against the `Camel.toml` root. The walk is bounded (depth and file caps). The default is `["jobs"]`. An explicit empty list, `dirs = []`, means "no discovery roots": `camel job` finds no documents.
- `dir` (string, legacy): compatibility alias from before the list form. It folds into a one-element `dirs` list. When both keys are present, `dirs` wins and `dir` adds no duplicate root. Prefer `dirs` in new configurations.

Entries are literal directory paths, not globs. Do not use `src/**/*.yaml` patterns here; use the directory that contains the job documents.

## Profiles

`[jobs]` follows the standard profile rules: a `[<profile>.jobs]` section merges on top of `[default.jobs]`, and table merges are additive.

Use the same key at both levels. If one profile sets `dirs` and another sets `dir`, the merged table carries both keys and `dirs` takes precedence — the profile-level `dir` is ignored without a warning:

```toml
[default.jobs]
dirs = ["jobs"]

[production.jobs]
dirs = ["jobs/production"]   # same key: replaces the default list
```

## Declared arguments

A job document can declare its interface with a top-level `args:` map. Each entry names one argument and admits four keys: `type` (one of `string`, `int`, `bool`, or `enum[a,b,c]`), `required` (boolean), `default`, and `description` (strings).

The declaration is part of the document, not of `Camel.toml`, so it travels with the job. `camel job` resolves the supplied arguments against it — each argument arrives through its dynamic `--<name>` flag — applies defaults, coerces typed values, and interpolates the results through `${arg:NAME}` tokens in the document's `to`, `body`, `headers`, and `timeout`. A required argument without a default must arrive on the command line, or the run fails before boot.

See [camel job](../cli/job.md) for the full grammar, coercion rules, and the `--help` interface renderer. Authority: the `cli-jobs` canonical spec and [`crates/camel-cli/CONTEXT.md`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-cli/CONTEXT.md).

## Discovery behavior

`camel job` walks the resolved roots in order, collecting job documents up to the bounded depth and file count. Discovery order follows the `dirs` list order. Job names derive from the document, per the `camel job` listing contract.

Authority: ADR-0062 (amended 2026-09-13), the `cli-jobs` canonical spec, and the camel-job orientation ruling.
