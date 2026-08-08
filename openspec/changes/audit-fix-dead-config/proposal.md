# Proposal: audit-fix-dead-config

## Why

The v1.0 audit (FC-DEAD-CONFIG) found config fields that are parsed from URI/TOML, unit-tested, and documented — but silently ignored at runtime. This is a fidelity and least-surprise defect: an operator who sets `proxy_url` or `cookieHandling=InMemory` reasonably expects the runtime to honor it. Pre-freeze is the last window to remove public fields without a breaking change.

## What Changes

**Remove** 6 dead config surfaces across 3 crates (5 fields + 1 enum):
- `camel-xj`: `transformDirection` (redundant with `direction`) and `resourceUri` (additional resource loading is unimplemented; primary stylesheet loading exists via `direction`)
- `camel-http`: `CookieHandling::InMemory` enum + field (blocked on reqwest `cookie_store` feature, TODO HTTP-013) — remove runtime state, reject param in URI parser
- `camel-direct`: `block` (TODO DIR-001, direct is synchronous) and `exchange_pattern` (always InOut)

**Keep-and-reject** 1 field:
- `camel-http`: `proxy_url` field retained for serde compatibility (HttpConfig lacks `deny_unknown_fields`); `validate()` rejects any `Some` value with SSRF-specific error. Remove deprecated `with_proxy_url` setter.

**Wire** 1 field:
- `camel-ws`: `send_timeout` — wrap the WebSocket send path in `tokio::time::timeout` (feature is legitimate and trivially implementable)

**Excluded**: post-1.0 re-addition of cookie jar or proxy support (those require design work for SSRF-compatible integration).

## Affected crates

- `camel-xj` (rc-1v0s)
- `camel-ws` (rc-yaep)
- `camel-http` (rc-7hgc, rc-ngnt)
- `camel-direct` (fold from rc-p0ta — no standalone bd)

## Acceptance criteria

- No config field is parsed and silently ignored
- Removed fields leave no orphan parse, test, or doc references
- `send_timeout` is enforced in the WS send path with a test that proves it
- All existing tests pass (adapted for removed fields)
- `lint-context-citations` passes (no stale CONTEXT.md references)

## Risk budget

Removing pub fields is a breaking change — acceptable only pre-freeze. The fields have zero runtime consumers (validator-confirmed). Risk is limited to operators who set these params and silently got no effect; for them the change makes the failure explicit (unknown-param error or field-not-found) rather than silently wrong.

Bd: rc-p0ta, rc-1v0s, rc-yaep, rc-7hgc, rc-ngnt
