# audit-ignore-prune

Drop the stale `RUSTSEC-2026-0235` (rkyv) ignore from `.cargo/audit.toml` (bd rc-fiw58).

## Why

Trivial change — no spec breakdown needed.

## What changes

- Delete the `RUSTSEC-2026-0235` comment block (and its separator) from `.cargo/audit.toml`; the ignore is a no-op — `Cargo.lock` carries `rkyv 0.8.18` (fixed in >= 0.8.17), so the advisory cannot trigger. The bd issue's "rkyv absent from lock" premise is stale; the removal is correct for a different reason.
- Remove `"RUSTSEC-2026-0235"` from the `ignore = [...]` array, leaving the other four IDs and their comments byte-identical (deletion-only diff).
