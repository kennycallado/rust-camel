## Why

Flow-review audit (2026-09-11) found landed behaviors that never received a
spec delta, and one branch (itest-batch) that edited canonical specs
directly, bypassing the delta+archive path:

- redis-batch landed the sentinel TLS surface — per-plane CA install
  (7b750a0b), fail-closed CA on mixed sentinel schemes (a19d9cb4), and the
  live `rediss-sentinel://` round-trip (854a429c, bd rc-hbde6) — while the
  redis-tls canon still says "CA trust is standalone-only in this change".
- http-batch landed the security surface — per-redirect-hop
  `allowedUriHosts` fence (83cc7e5f, bd rc-sxe1x), the WHATWG
  special-query rejection set pin (164ae82d, bd rc-nmupb/rc-wsx2y), and
  ADR-0070 consumer test listener staging (b9deb92d, bd rc-1dgvg) — none
  of which appear in the http-url-resolution or staged-listener-binding
  canons.
- The named shared-cache memory URI convention (7cc0cb23, bd rc-gcf9n) is
  the adopted scenario-tier in-memory convention but the integration-tier
  canon still describes it as "planned" and does not state the convention.

This change repairs the canon retrospectively through ONE properly
archived change, so future changes diff against an accurate spec.

## What Changes

- redis-tls: MODIFIED "Standalone TLS connections trust the configured CA"
  (drop the stale standalone-only limitation); ADDED "Sentinel CA trust
  installs per plane and fails closed on mixed schemes"; ADDED "Sentinel
  TLS live coverage through rediss-sentinel://".
- http-url-resolution: MODIFIED "Outbound query composition" (pin the full
  WHATWG special-query rejection set and the `%27` carve-out); MODIFIED
  "CamelHttpUri host fence" (fence applies to every redirect hop).
- integration-tier: MODIFIED "SQLite shared-cache mandate" (codify the
  named shared-cache memory URI convention, pool size as author choice,
  lint steering); MODIFIED "Scenario datasource teardown" (convention
  note moves from planned to landed).
- staged-listener-binding: MODIFIED "Test port acquisition from staged
  listeners" (extend to component-internal test listeners — camel-http
  consumer tests per ADR-0070, readiness poll instead of sleep).

## Impact

- Specs: `openspec/specs/{redis-tls,http-url-resolution,integration-tier,
  staged-listener-binding}/spec.md`, updated via the archive sync at
  close of this change.
- No code, no API surface, no docs outside `openspec/` — specs-only
  retrospective. Every delta cites the landing commit; scenarios match
  behavior verified on main @ 591e1b85.
