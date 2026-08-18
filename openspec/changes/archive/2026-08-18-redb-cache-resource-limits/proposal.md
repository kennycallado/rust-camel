# Proposal: redb-cache-resource-limits

## Why

Production incident (preprod k8s, bd `rc-2iz3`): camel-cache with the redb backend reached RSS 632Mi after 13h of pod life under a 768Mi memory limit, with no real user traffic. Root cause: `RedbCacheRepository` opens redb through `redb::Database::create` without `Builder::set_cache_size`, so redb 4.1.0 applies its default page cache of 1GiB. The page cache grows with touched pages, independent of entry TTLs or sweep activity. In any container with a memory limit below 1GiB, OOMKill becomes inevitable once the database file grows. Two more redb open sites (idempotent repository, lifecycle journal) share this defect class; they are follow-ups, not scope here.

Drift found during triage: ADR-0056 Decision 6 states the redb sweep runs "at a configurable interval (default `60s`)", but the wiring hardcodes 1h and `CacheRepoConfig` exposes no such knob. This change resolves the doc/code contradiction.

## What Changes

Two delivery phases, blessed as one plan.

**Phase 1 — incident fix (memory budget under operator control):**

- `CacheRepoConfig` (camel-config): new **required** `cache_size` field when `backend = "redb"` — byte-size string such as `"384MB"` or `"512MiB"` — and optional `sweep_interval` (humantime string such as `"30m"`, default `"1h"`, must be > 0). A config declaring the redb backend without `cache_size` fails validation with an actionable error: the 1GiB redb default is exactly what caused the incident, so it must not be reachable silently. Strict parsing: malformed `cache_size`/`sweep_interval`/`stale_retention` values fail config load with field-naming errors — a typo silently restoring a default would recreate the incident.
- `RedbCacheRepository::new` (camel-core): takes a required `cache_size: usize` and opens redb via `Builder::set_cache_size`. Programmatic callers must state their budget, same as config users.
- Startup WARN when the effective cache size exceeds the container memory limit detected from cgroup v2 (`/sys/fs/cgroup/memory.max`, v1 fallback). Warning only, never fatal.
- ADR-0056: Decision 6 amended with a dated rationale — the documented 60s default never shipped; configurability is now real, the default stays at the shipped 1h.

**Phase 2 — dead-config hygiene on the same section:**

- Cross-backend field rejection per the dead-config-policy spec: `path`, `stale_retention`, `max_entries`, `cache_size`, `sweep_interval` set with `backend = "memory"` fail validation; `max_capacity` with `backend = "redb"` fails too.
- `stale_retention` serde default changes from `Some("168h")` to `None` (omitted must not materialize as a set field — otherwise every memory config would trip the new rejection); the 7d fallback applies in wiring only, after validation.
- Delta requirements added under the `dead-config-policy` spec domain.

Explicitly excluded: `cache_size` exposure for the idempotent repository and lifecycle journal (bd follow-ups), sweep cost reduction via an expiry secondary index (storage schema migration; bd follow-up).

Affected crates: `camel-core`, `camel-config`, plus `docs/` and in-repo example code that constructs `RedbCacheRepository`. bd: `rc-2iz3`.

## Acceptance criteria

- An operator setting `cache_size = "384MB"` and `sweep_interval = "30m"` in `[default.cache_repo]` gets those values in the redb `set_cache_size` call and the sweep task; propagation is observable through a test seam.
- A redb-backend config without `cache_size` fails validation naming the field and suggesting values.
- Malformed `cache_size`, `sweep_interval`, or `stale_retention` fail validation; zero `sweep_interval` fails (`tokio::time::interval` panics on zero).
- The startup WARN fires when the effective cache size exceeds a detected cgroup limit; malformed or missing cgroup files stay silent; tests use temp files and captured tracing, no real cgroup.
- Cross-backend fields are rejected with dead-config-policy delta scenarios covering each mismatch.
- ADR-0056 text matches the implementation, with a dated amendment note.

## Risk budget

Moderate, and honest about it: this is a **breaking config change** for redb-backend users — configs must add `cache_size` (no in-repo TOML example uses `cache_repo`, so the blast radius inside the repo is example code only). New validation turns previously-ignored fields and malformed durations into hard errors. In exchange, the OOMKill class becomes unreachable: no code path reaches redb's 1GiB default anymore. Storage format, trait surface, and DSL are untouched. Decimal (`MB` = 10^6) vs binary (`MiB` = 2^20) suffix semantics are pinned by explicit tests; overflow values are rejected at parse time.
