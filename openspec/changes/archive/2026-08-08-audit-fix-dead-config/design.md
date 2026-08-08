# Design: audit-fix-dead-config

## Approach

Per-field disposition: each dead config field receives **remove** (field + parse + test + doc surface deleted, param explicitly rejected in URI parser), **keep-and-reject** (field retained for serde compatibility but validate() rejects any Some value), or **wire** (field connected to runtime logic + enforcing test added).

## Disposition matrix

| # | Field | Crate | Disposition | Rationale |
|---|-------|-------|-------------|-----------|
| 1 | `transform_direction` | camel-xj | **Remove + reject param** | Redundant with `direction` URI param (which is consumed). Apache Camel alias that adds no value. URI parser explicitly rejects `transformDirection` as unknown. |
| 2 | `resource_uri` | camel-xj | **Remove + reject param** | Additional resource loading unimplemented (primary stylesheet loading exists via `direction`). URI parser explicitly rejects `resourceUri` as unknown. |
| 3 | `send_timeout` | camel-ws | **Wire (client mode only)** | Legitimate operational concern. Wrap client-mode WebSocket send (`ws_stream.send`) in `tokio::time::timeout`. Server-send mode (`try_send_with_backpressure`) is an internal mpsc channel send and is NOT in scope — it is a backpressure mechanism, not a network send. |
| 4 | `CookieHandling` enum + `cookie_handling` field | camel-http | **Remove runtime state + reject param** | Blocked on reqwest `cookie_store` feature (TODO HTTP-013). DNS pinning rebuilds client per request — cookie jar persistence across rebuilds is non-trivial. Remove enum and runtime state; keep `cookieHandling` as a reserved param that URI parser explicitly rejects. |
| 5 | `proxy_url` + `with_proxy_url` | camel-http | **Keep-and-reject** | `#[deprecated]`, incompatible with SSRF DNS pinning. HttpConfig lacks `deny_unknown_fields` — removing the field would silently discard TOML config. Retain field for serde compatibility; `validate()` rejects any `Some` value with an SSRF-specific config error. Remove deprecated `with_proxy_url` setter. |
| 6 | `block` | camel-direct | **Remove + reject param** | TODO(DIR-001). Direct is synchronous by design. URI parser explicitly rejects `block` as unknown. |
| 7 | `exchange_pattern` | camel-direct | **Remove + reject param** | Always InOut. URI parser explicitly rejects `exchange_pattern` as unknown. |

## Affected crates

- **camel-xj** (`config.rs`, `README.md`): delete `transform_direction` and `resource_uri` fields. URI parser rejects `transformDirection` and `resourceUri` as unknown params. Delete unit tests. Update README table.
- **camel-ws** (`config.rs`, `lib.rs`): wire `send_timeout` into the client-mode WebSocket send path (`ws_stream.send`) via `tokio::time::timeout`. Server-send mode unchanged. Add a test using paused Tokio time that verifies the timeout fires.
- **camel-http** (`config.rs`, `lib.rs`, `ssrf.rs`): remove `CookieHandling` enum + `cookie_handling` field + `build_client` cookie param + `ssrf.rs` cookie param. URI parser explicitly rejects `cookieHandling`. For `proxy_url`: retain field, remove `with_proxy_url` setter, add `validate()` rejection with SSRF-specific error.
- **camel-direct** (`lib.rs`): remove `block` and `exchange_pattern` fields. URI parser rejects both as unknown params. Update tests.

## Architecture boundaries

All changes are within component crates (leaf-level). No contract-crate, DSL, or core changes. The removal of public fields is a pre-freeze breaking change — after v1.0 these would require deprecation cycles.

## Alternatives considered

- **Wire all fields**: Rejected — cookie jar and proxy require SSRF-compatible designs that don't exist yet. Additional XSLT resource loading is a feature, not a bugfix.
- **Remove all fields including proxy_url**: Rejected — HttpConfig lacks `deny_unknown_fields`; removing `proxy_url` would silently discard TOML config. Keep-and-reject makes the failure explicit.
- **Silently ignore removed params**: Rejected — a param that parses without error and silently does nothing is the exact defect being fixed. All removed params must be explicitly rejected by the URI parser.
