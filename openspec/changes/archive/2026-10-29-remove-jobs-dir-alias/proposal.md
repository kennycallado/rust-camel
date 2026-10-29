# Proposal: remove-jobs-dir-alias

## Why

Row 8 of the pre-1.0 shedding plan (e_opus 2026-10-27, epic rc-v6wiz; bd rc-3lqgi). `JobsCamelConfig` in `crates/camel-config/src/config.rs` still accepts the legacy `[jobs].dir` alias and folds it into `dirs`. ADR-0090 (pre-1.0 compatibility policy) says delete, not hide: a removal must never take away a capability, only an ambiguous/duplicate spelling of it, and removed keys fail the `Camel.toml` load with a tombstone error naming the replacement. `dirs` is the canonical, shipped key (`dirs = ["<path>"]`); the alias is a duplicate spelling kept only for our own history. Pre-1.0 is the window where this can break without a compatibility shim.

## What Changes

- `camel-config`: delete the `dir` field from `JobsCamelConfig`; `resolved_dirs()` uses `dirs` when present (including `[]`) and the `["jobs"]` default otherwise. Hand-written `Deserialize` impl rejects the `dir` key with a tombstone naming the key and the replacement, and keeps unknown-key rejection for everything else. `#[deprecated]` is not used (ADR-0090 forbids it).
- `camel-config` tests: drop/rework the alias tests (`legacy_jobs_dir_alias_is_supported`, `explicit_job_dirs_override_legacy_dir`, `jobs_table_dir_round_trip`, the dir-aware overlay/merge/profile-warning tests); add a load-failure test asserting the message names `dir` and `dirs` with the `dirs = ["<path>"]` migration. Keep `dirs = []`, default, and `dirs`-round-trip tests.
- `camel-cli`: remove the ignored-`dir` stderr warning path in the job command and its tests; drop the alias-only CLI tests (`explicit_job_dirs_override_legacy_dir`, `listing_warns_on_profile_shadowed_legacy_dir`); add a CLI-level load-failure test.
- Spec: delta under `specs/cli-jobs/` removing the alias normative text and its two scenarios from "job discovery and no-argument listing", adding the tombstone scenario.
- Docs: CONTEXT-MAP.md `[jobs]` lines, ADR-0062 amendment note, `docs/src/configuration/schema.md` (drop the `jobs.dir` row). Migration is `dirs = ["<path>"]`.

Explicitly excluded: changing job discovery/resolution semantics, the `dirs` default, other config keys, CLI output beyond the removed warning.

## Acceptance criteria

- `[jobs].dir` in `Camel.toml` fails the load with an error mentioning both `dir` and `dirs` (e.g. `dirs = ["<path>"]`).
- `resolved_dirs()`: `dirs` present (including `[]`) wins; `["jobs"]` default otherwise; no alias branch remains.
- `cargo test -p camel-config` and the affected `camel-cli` job tests pass; `cargo fmt --check --all`; `cargo clippy -p camel-config --all-targets -- -D warnings` and camel-cli equivalents pass; the mission's xtask lint set passes; `openspec validate --all --strict` passes.

## Risk budget

Low. Config-surface breaking change, sanctioned pre-1.0 by ADR-0090. Users with `[jobs].dir` get a load-time tombstone pointing at the exact migration. No runtime/discovery behavior changes for `dirs` users.
