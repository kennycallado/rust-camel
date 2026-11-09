# Proposal: audit-advisories

Turn `cargo audit` green on main (`cbe2ba0df`). Dependency-only mission (bd rc-kdmdq):
no `crates/**/src/**` changes, no `bridges/**`, no behavior change.

## The two advisories

| Crate | Advisory | Fix available | Action |
|---|---|---|---|
| `rmcp` 3.8.0 (via camel-component-mcp) | RUSTSEC-2026-0462 — unbounded response body buffering in Streamable HTTP clients, 6.5 | `>= 3.9.0` exists | `cargo update -p rmcp --precise 3.9.0` (moves rmcp + rmcp-macros only). Verified: `cargo build -p camel-component-mcp` and `cargo test -p camel-component-mcp` pass with zero source edits. |
| `libsqlite3-sys` 0.30.1 (via sqlx 0.9.0 → sqlx-sqlite) | RUSTSEC-2026-0459 — C SQLite CVE-2026-119375, uninitialized WAL page cache, 6.8 | none: sqlx 0.9.0 (newest) pins `libsqlite3-sys ^0.30`; fix needs `>= 0.39.0` | accept-and-document via one `.cargo/audit.toml` ignore entry + bd follow-up |

## SQLite decision and evidence

Route: **accept-and-document** (not a silence — the exposure analysis below is the
reasoning the comment in `.cargo/audit.toml` carries).

1. **Bundled is TRUE.** The advisory's first precondition holds. Evidence: sqlx 0.9.0's
   own manifest defines `sqlite = ["sqlite-bundled", ...]` and
   `sqlite-bundled = ["_sqlite", "sqlx-sqlite/bundled", ...]`, and the resolved feature
   tree (`cargo tree -e features -i libsqlite3-sys`) shows the chain
   `libsqlite3-sys "bundled" ← sqlx-sqlite "bundled" ← sqlx "sqlite-bundled" ← sqlx "sqlite"`
   reached from `camel-integration-test`/`camel-cli` defaults and `camel-component-sql`.
   sqlx 0.9 made bundling the default for the plain `sqlite` feature; only
   `sqlite-unbundled` (buildtime bindgen + system library) avoids it.
2. **Reachability.** The advisory bites when a process opens a database file together
   with a sibling `-wal` whose page size differs — i.e. a less-trusted local party
   plants a crafted db/-wal pair where the victim will open it. In rust-camel, SQLite
   paths come from operator-configured route URIs (camel-component-sql), not from
   untrusted request input. The writer of the data directory and the opener are the
   same trust domain in every supported deployment; the crafted-pair attack requires a
   local attacker who can already write into the operator's data directory, which is a
   pre-existing compromise dominating this disclosure's impact. Not "non-applicable" —
   the exposure is judged not reachable within the component's trust model.
3. **No upgrade path today.** `cargo search sqlx` → 0.9.0 newest, pinning
   `libsqlite3-sys ^0.30` (fix needs >= 0.39.0). Confirmed in Cargo.lock:
   `libsqlite3-sys 0.30.1` has exactly one parent chain, sqlx-sqlite 0.9.0.
4. **Removal conditions** (stated plainly in the audit.toml comment): removed when sqlx
   ships a release compatible with `libsqlite3-sys >= 0.39.0`, or when the workspace
   moves camel-component-sql to `sqlite-unbundled` against a system SQLite >= 3.54.0.
   Tracked in bd follow-up (discovered-from: rc-kdmdq).

## Non-goals

- No `--ignore` flags in CI, no `continue-on-error`; `.cargo/audit.toml` is the only lever.
- No other dependency moved: the lockfile diff touches rmcp and rmcp-macros versions/checksums only.
- No source edits: if a later re-lock forces one, the mission parks instead of adapting call sites.
