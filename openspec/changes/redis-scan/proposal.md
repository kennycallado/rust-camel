# Proposal: redis-scan

## Why

`camel-component-redis` can only enumerate keys with `KEYS`, which is O(N)
over the whole keyspace and blocks a single-threaded Redis server; production
keyspace walks through the component are effectively unavailable. Redis's
cursor-based `SCAN` family (`SCAN`, `SSCAN`, `HSCAN`, `ZSCAN`) exists nowhere
in the crate (verified: 0 occurrences). Iteration is not just a command: the
contract must define cursor round-trips, per-page batch shape, option
translation (`MATCH`/`COUNT`/`TYPE`), and the termination signal, or every
route invents its own loop protocol. bd rc-r3m6x (p2, e_opus-ranked phase 3,
2026-11-13).

## What Changes

- Four new component commands: `SCAN`, `SSCAN`, `HSCAN`, `ZSCAN`
  (`RedisCommand` variants, `FromStr`/`as_str`, routing, retry-safe read
  classification).
- Cursor contract: `CamelRedis.Cursor` request header in, next cursor out on
  the same header (`0` = complete), `CamelRedis.ScanComplete=true` on the
  final page only. Absent/null/empty/`0` cursor = fresh start; cursors are
  opaque caller state (no server-side cursor store); malformed cursors are
  named pre-send errors.
- Options: `MATCH` via the existing `CamelRedis.Pattern`, `COUNT` via the
  existing `CamelRedis.Count` (default 100, the repository pager's page size),
  `TYPE` via `CamelRedis.Type` (SCAN-only; named rejection on the collection
  variants).
- Batch shape: ONE exchange per page; body reuses the crate's existing
  element encodings (keys array / members array / hash map / scored-member
  pairs). The family is NOT a `command=multi` batch entry (allowlist keeps
  rejecting it; cursor round-trips do not compose inside one pipeline).
- Shared scan-page plumbing in `camel-component-redis` (`encode_scan_page`,
  `parse_scan_reply`), and `camel-redis-repo`'s `scan_unlink_pattern_where`
  refactored onto it — one pager primitive, two consumers, no second fork,
  no dependency inversion.
- Docs: the camel-redis page gains the SCAN family section with one runnable
  cursor-loop example; `KEYS` gets a prefer-SCAN note (docs only, no
  behaviour change).
- Excluded: KEYS behaviour change (forbidden by the order), RESP3-specific
  reply modes beyond the driver's pair decoding, a pending/backlog gauge for
  stream recovery (rc-e6324 policy), ADR (spec delta only).

Ride-along rc-6qoxs (recovery-pass log split) already landed upstream in
7a4b8cb3d, contained in this branch's base — no work here, recorded in the
park report.

## Acceptance criteria

- Spec delta `specs/redis-commands/spec.md` validates; cursor contract,
  options, batch shape, iteration guarantees each pinned by scenarios.
- Unit tests: cursor resolution (absent/0/empty/opaque/malformed), wire-frame
  builders, page-reply parsing, body shapes; repository pager tests stay
  green after the refactor.
- Live Redis-lane tests: multi-page iteration with `MATCH`+`COUNT`, `HSCAN`
  map body, `ZSCAN` pair body, termination signal, repeat-across-pages.
- `bash scripts/gates-local.sh` exit 0; docker lanes run locally before
  parking.

## Risk budget

- Acceptable: refactoring the repository pager onto the shared encoder/parser
  (mechanical; existing repo tests pin behavior).
- Not acceptable: changing `KEYS` semantics, inverting a crate dependency,
  new external dependencies, a server-side cursor store.
