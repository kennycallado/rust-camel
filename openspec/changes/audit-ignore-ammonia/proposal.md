# audit-ignore-ammonia

Drop the patched `RUSTSEC-2026-0373` (ammonia) ignore from `.cargo/audit.toml` (bd rc-9mq7v).

## Why

Trivial change — no spec breakdown needed.

## What changes

- Delete the `RUSTSEC-2026-0373` comment block (and its separator) from `.cargo/audit.toml`; the ignore is stale — `Cargo.lock` carries `ammonia 4.2.4`, inside the advisory's patched range (>= 4.2.4), and the comment's premise (`ammonia 4.1.7` pulled by `surrealdb-core 3.2.4`) no longer matches the surrealdb 4.0 chain in the lock.
- Remove `"RUSTSEC-2026-0373"` from the `ignore = [...]` array, leaving the other three IDs (`RUSTSEC-2023-0071`, `RUSTSEC-2026-0249`, `RUSTSEC-2026-0387`) and their comment blocks byte-identical (deletion-only diff).
