# URL Redaction Surface Map (2026-09)

Mission `redactleaks` (bd rc-zf06t, rc-y6101, rc-eh49). Pre-flight ruling:
e_opus, 2026-09-16. All prose follows STE discipline.

## Scope

Every `redact*` URL helper and call site inside the mission zone lease:
`camel-http` (`lib.rs`, `ssrf.rs`), `camel-config/src/config.rs`, `camel-jms`.
Adjacent helpers outside the lease appear in a separate section. They are
context only. No code in this audit changes them.

## Zone helpers

Line references are the pre-Phase-B/C snapshot; helper locations shifted in
the fix commits.

| Helper | Crate / file | Redacts | Mask style | Callers | Tests |
|---|---|---|---|---|---|
| `redact_url_for_diagnostics` | camel-http `lib.rs:2894`, `pub(crate)`, url-crate based | userinfo (parsed arm), whole query string | `***@` userinfo; `?[redacted]` query; `[redacted]` sentinel on unparseable authority credentials; 256-byte UTF-8-safe cap | `lib.rs` x5 (endpoint Debug, request/response logs, error contexts), `ssrf.rs:443` (redirect fence rejection) | 10 pinned tests in `lib.rs` tests module |
| `mask_base_url_userinfo` | camel-http `lib.rs:2864`, private | userinfo only, Debug surface | byte-preserving `***@` surgery, no parse | `HttpEndpointConfig` manual Debug | covered via config Debug tests |
| `redact_url` | camel-config `config.rs:1390`, private | userinfo only | `***@`, first `@` before first `/` | `CacheRepoConfig` Debug (`config.rs:746`), idempotent repo Debug (`config.rs:1417`) | indirect, via repo Debug tests |
| `redact_url` | camel-jms `component.rs:940`, private | userinfo only | `***@`, first `@` anywhere after `://` | broker URL log line (`component.rs:574`) | 4 tests (`component.rs:1823` block) |
| `redact_broker_url` | camel-jms `config.rs:400`, `pub(crate)`, string-based | userinfo plus sensitive query keys | `***@` userinfo; `<redacted>` per key; substring key allowlist: `password, passwd, secret, credential, token, username, user` | `BrokerConfig` Debug (`config.rs:448`) | 2 tests (`validate_error_redacts_broker_url_credentials`, `masks_userinfo_and_sensitive_query`) |

## Defects confirmed by code reading

### rc-zf06t: fragment credentials survive both arms

The parsed arm checks `u.query()` only. A URL such as
`https://app.example/cb#access_token=SECRET&state=x` has no query. The
fragment rides `u.to_string()` into logs. The parse-failure arm truncates at
`?` only. A fragment on a malformed URL survives the same way. OAuth2
callback URLs put bearer tokens in fragments. This is real credential
exposure in error logs.

### rc-y6101: double-`//` evades the failure-arm window

The failure arm anchors on the first `//` it finds. Input
`scheme:////user:pass@evil/` yields an empty window. The window ends at the
`/` that sits at offset zero of the remainder. The `@` check misses. The raw
string then renders with embedded userinfo intact. Any slash count above two
reproduces this.

### Latent finding: `redact_broker_url` userinfo step misfires

The userinfo step splits on the first `@` of the whole string. An `@` inside
a query or fragment can pull the split out of the authority. The mask then
rewrites the wrong region. This helper also lacks a fragment rule and a
length cap. Fix in Phase C alignment.

### Latent finding: `camel-config::redact_url` keeps query secrets visible

The helper masks userinfo only. A redis URL such as
`redis://h:6379/0?password=x` renders its query verbatim in Debug output.
Phase C alignment drops the query. This change is intentional. A test pins
it. Operators who debug redis connections from Debug output lose the query
bytes. That is the correct trade: query bytes are not trustworthy in logs.

## Reconciliation ruling (Phase C)

The pre-flight expert ruled: converge each helper to the strictest behavior
on hardening dimensions. One dimension keeps a documented weaker exception.

| Dimension | Converge to | Exception |
|---|---|---|
| Userinfo mask | windowed authority scan, both arms, fail-closed sentinel when unparseable | none |
| Authority window terminators | `/`, `?`, `#` | none |
| Slash-run evaders | skip the full slash run before the window scan | none |
| Query | drop whole query, append `?[redacted]` | JMS broker URLs keep per-key allowlist, see below |
| Fragment | drop whole fragment, append `#[redacted]` | none |
| Length cap | 256 bytes on a UTF-8 boundary, per-crate local copy of the truncate helper | none |

JMS broker exception, stated reason: ActiveMQ failover URIs encode
non-secret transport policy in query parameters. Those parameters are the
sole diagnostic value of the broker URL in Debug output. Whole-query drop
would make that Debug impl useless for failover diagnosis. The substring
allowlist stays for the query dimension only. Userinfo, fragment, and
length-cap dimensions converge.

## Adjacent surface, outside the lease

- `camel-api/src/endpoint_uri.rs`: `EndpointUri::to_redacted_string` plus
  `redact_value`. Catalog-driven, metadata-based secret detection. Serves
  authored endpoint URIs. Different threat model. Do not touch.
- `services/camel-auth/src/credential_source.rs:188`:
  `pub fn redact_query_params`. Typed `http::Uri`, caller-supplied key
  allowlist. `camel-ws` consumes it. Do not touch.
- Residual risk, accepted: backslash authority separators on non-special
  schemes can reach the failure arm without a `//` anchor. The url crate
  parses backslashes as separators for special schemes, so those cases take
  the parsed arm. Non-special schemes keep the residual risk. A test pins
  the parsed-arm behavior.

## Outcomes (landed on `feature/redactleaks`)

Phase B, commit `b9775937` plus review fixes `c265007d`: both camel-http
leaks closed. The parsed arm now captures query and fragment state before
it renders, then appends `?[redacted]` and `#[redacted]` in wire order.
Discovery during implementation: rust-url parses `scheme:////user:pass@evil/`
with no host, so the double-slash evader reaches the parsed arm with its
userinfo bytes in the path. A fail-closed guard now sentinels a parsed URL
that carries an authority marker but no host. The window scan visits every
`//` window, not only the first. Empty-host URLs such as `file:///us@r/x`
also trip the guard. That over-redaction is deliberate and follows
ADR-0051. 13 new tests that round; 382 lib tests green at that point.

Phase C, commits `ba7140e5` and `7102bd5f`: camel-config and camel-jms
helpers converged to the ruling table. camel-config now drops query and
fragment (the query change is intentional: the old helper kept
`?password=...` bytes visible in Debug output). camel-jms masks through a
windowed scan shared by both helpers (`mask_authority_windows`), keeps the
broker query allowlist with the stated exception, and gains fragment drop
plus the 256-byte cap. The old broker userinfo step split on the first `@`
of the whole string and mangled URLs whose query carried an `@`; a test
pins the fixed shape. Final count after the follow-up round below: 10 new
tests in camel-config and 13 in camel-jms (23 across the two crates).

Backslash inputs: the url crate parses `http:\\user:pass@evil\path` on the
special-scheme path, so the parsed-arm mask covers it; a test pins that.
Non-special schemes with backslash separators keep the accepted residual
risk recorded above.

Follow-up round: the camel-http parsed arm now applies the same
every-`//`-window mask to its rendered string (`mask_rendered_windows`, a
local duplicate of the jms surgery per the parked deferral), closing the
last later-window leak: rust-url parks the userinfo bytes of
`https://h//user:pass@evil/` in the path, where the accessor mask never
reaches. All failure-style arms (camel-http Err arm, camel-config
`redact_url`, camel-jms `redact_url`) now follow a compose-both sentinel
rule: each distinct `?`/`#` introducer anywhere in the raw URL appends its
sentinel in first-occurrence order, so `?[redacted]#[redacted]` (or the
reverse when `#` comes first) always renders completely;
`redact_broker_url` already composed naturally and keeps its allowlist.
`mask_authority_windows` now builds and returns its own String, which
removes the doc-only `out must equal input` precondition. camel-http pins
the round with 4 new tests; each string-based crate gains later-window and
composition pins.

Sentinel round (e_gpt stage-4 finding): every helper now reserves the
sentinel byte budget before the 256-byte cut, so a sentinel can never
render split. One pin per crate drives the boundary. A second e_gpt
finding — percent-encoded credentials under benign broker query keys —
is out of the original scope and is filed as bd rc-r7v8s for the landing
review. Final counts: camel-http 14 new tests (387 lib), camel-config 11
(303 lib), camel-jms 14 (154 lib).

## Parked proposal (deferral)

Single-crate consolidation needs a canonical home all three zone crates can
import. The only such crate today is `camel-api`. It sits outside the zone
lease. The mission order says: park the proposal, do not cross the line.
The deferral proposes `camel-api` as the canonical home, filed against
rc-eh49. Until it lands, each crate keeps a local helper with aligned
semantics per the table above.

rc-eh49 is therefore partial by design: semantic convergence in-lease,
structural consolidation parked. The close reason must state this.
