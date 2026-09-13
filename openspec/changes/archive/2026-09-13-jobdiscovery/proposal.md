# Proposal: jobdiscovery

## Why

`camel job` currently treats one directory as its complete job set. This blocks projects that organize jobs by domain and makes discovery depend on the process working directory. Issue `rc-k0rvt` requires a declarative, Camel.toml-rooted discovery set while preserving the existing single-directory configuration and cheap description listing.

## What Changes

- Add `[jobs].dirs` with default `["jobs"]`; fold legacy `[jobs].dir` into one entry.
- Recursively scan each declared directory with depth and file-count bounds.
- Reuse `camel_dsl::discovery::is_job_document` and existing description probing.
- Resolve named jobs in declared order and report cross-directory stem collisions.
- Keep absent directories harmless and keep listing outside route discovery, interpolation, and security compilation.
- Explicitly exclude argument schemas, `--arg` validation, interpolation, help rendering, typed arguments, and CLI globs.

## Acceptance criteria

- Lists `.job.yaml` and `.job.yml` documents across ordered `dirs`, rooted at Camel.toml.
- Legacy `dir` works; `.test.yaml` files are silently skipped.
- Bounds produce one warning, continue with exit 0, and never abort malformed siblings.
- Named-job ambiguity names all matching files and exits 2; first hit otherwise follows declared order.
- No route-discovery pipeline runs during listing.

## Risk budget

Accept small internal refactoring in `camel-cli` and `camel-config`, plus focused CLI/config tests. Amend ADR-0062 and `CONTEXT-MAP.md` to keep architectural authorities current. Do not change job document schema, route discovery, argument behavior, or public runtime APIs. Preserve existing single-directory behavior and output semantics except for additional discovered entries and required ambiguity/truncation diagnostics.
