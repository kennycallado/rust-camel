# Proposal: sql-connect-timeout

**Bd:** rc-sa1zo (P2 bug) · Type: trivial config correctness fix · Base: `e301bc29f`

## Problem

`crates/components/camel-sql/src/config.rs:911-915` (`enrich_db_url_with_ssl_params`) appends
`connect_timeout=10` to postgres/mysql database URLs when the operator did not set it.
`sqlx-postgres 0.9.0` does not read that URL key, so every pool creation logs

```
WARN sqlx_postgres::options::parse: ignoring unrecognized connect parameter key=connect_timeout
```

(sqlx-postgres 0.9.0 `src/options/parse.rs:107`) and the intended TCP connect timeout never
applies. The value is decorative; the in-code comment already admits it is not the pool
`acquire_timeout`.

## Evidence (sqlx 0.9.0 sources, cargo registry)

1. **No connect-timeout API exists.** `grep -rn connect_timeout sqlx-postgres-0.9.0/src/` returns
   nothing: neither `PgConnectOptions` nor `AnyConnectOptions` exposes a connect-timeout option or
   URL key. The URL-level key never existed in sqlx-postgres 0.9.
2. **What bounds a stuck TCP connect today:** `AnyPoolOptions::acquire_timeout`
   (sqlx-core 0.9.0 `src/pool/options.rs:160`, default `Duration::from_secs(30)`). The pool applies
   it as an acquire deadline (`options.rs:541-542`: `deadline = Instant::now() + acquire_timeout`)
   that covers queueing **plus** the TCP connect, not per-TCP. camel-sql never overrides it, so
   the 30 s default applies to every pool this component builds.
3. **The warning is URL-key-driven.** `parse.rs:107` warns exactly when an unrecognized key is
   present in the URL. Removing the append removes the warning deterministically.

## Chosen option: (a) drop the fabricated parameter, document the real bound

- Option (b) — "set the bound through the proper pool option" — would require wiring
  `AnyPoolOptions::acquire_timeout` from a config knob that camel-sql does not have; adding a new
  public config knob is out of scope for this mission (non-goals: no new URI parameter, no new
  config section).
- Consequence: dropping the append is a pure no-op semantically — sqlx ignored the key anyway.
  Behavior changes only in that the WARN line disappears.

## User-visible consequence

- Pool creation no longer logs the `ignoring unrecognized connect parameter` warning.
- A stuck TCP connect is bounded by the pool `acquire_timeout` (sqlx default **30 s**), which also
  covers queue wait — it is **not** a per-TCP 10 s timeout as the old code implied. The old 10 s
  value never applied, so effective behavior is unchanged.
- An operator-set `connect_timeout=…` in the URL is **left untouched** by enrichment (it already
  was). It remains meaningless to sqlx-postgres 0.9; docs say so instead of stripping it.

## Changes

1. `crates/components/camel-sql/src/config.rs`: remove the `connect_timeout` append block and its
   comment. SSL enrichment and all other query-pair rewrites untouched. Update the three unit-test
   assertions that expect `connect_timeout=10`; keep `enrich_preserves_explicit_connect_timeout`
   (operator URL must not be rewritten). Add a unit test asserting the enriched URL carries no
   `connect_timeout` key and still carries the SSL pairs.
2. `crates/components/camel-sql/README.md` + `docs/src/components/sql.md`: one paragraph stating
   what timeout actually applies (pool acquire timeout, sqlx default 30 s, covers acquire+connect,
   not per-TCP; operator URL `connect_timeout` keys are ignored by sqlx-postgres 0.9).

## Test plan

- New/updated unit tests red before the fix, green after (no Docker — pure URL transformation).
- Integration leg (testcontainer WARN capture) **not run**: the warning is emitted by sqlx's URL
  parser only when the key is present in the URL (`parse.rs:107`), and pool creation requires a
  live Postgres. The removed append plus the unit test asserting the key's absence are the proof:
  no key in URL → parser never reaches the warn arm.
- Gates: `cargo test -p camel-component-sql`, `cargo fmt --check --all`,
  `cargo clippy -p camel-component-sql --all-targets -- -D warnings`,
  `cargo xtask lint-unwrap`, `cargo xtask lint-context-citations`, `cd docs && mdbook test`.
  `lint-commits` SKIPPED per mission order. Full-workspace `cargo test` forbidden.
