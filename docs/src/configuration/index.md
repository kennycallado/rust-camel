# Configuration

`Camel.toml` is the operator surface for rust-camel. `CamelConfig` deserializes the file into a profile-aware tree. Fields live under `[default]` and `[<profile>]` sections, deep-merged with includes and `CAMEL_*` overrides.

Top-level sections: `[default.routes]` (discovery globs), `[components.*]` (per-component defaults, untyped TOML), `[supervision]` (retry and backoff), `[observability]` (tracing and metrics), `[idempotent_repo]` (persistent idempotent backend).

Set `CAMEL_PROFILE` to select a profile. The `[default]` section always applies. The named profile merges on top. Use `include = ["path/to/file.toml"]` to pull shared sections from other files.

- [Environment variable interpolation](env-interpolation.md): substitute `${env:VAR}` tokens in route files before parse
- [Hot reload](hot-reload.md): swap pipelines at runtime without downtime

**Reference**: [Config crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-config/CONTEXT.md)
