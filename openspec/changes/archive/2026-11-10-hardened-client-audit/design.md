# Design: hardened-client-audit

The deliverable of this change is the table below plus bd issues. This document is the
durable record; the bd issues are the actionable slice.

## Enumeration method (reproducible)

Run from repo root of any worktree cut from `main`:

```sh
rg -n 'reqwest::Client::builder|ClientBuilder|connect_timeout|read_timeout|\.timeout\(' crates/
rg -ln 'imap|lettre|SmtpTransport|rustls' crates/**/src
rg -n 'Client::new\(\)' crates --type rust
rg -n 'deadline|set_timeout|connect_timeout' crates --type rust   # tonic/tonic-channel vocabulary
rg -n 'connect_with_local_defaults|Client::try_default|bollard|kube' crates --type rust
rg -n 'reqwest::blocking' crates --type rust            # negative check: none exist
rg -n 'object_store|aws-sdk|azure_storage|gcloud' crates -g 'Cargo.toml'  # negative: none
```

Every hit was triaged as: **production client** (row below), **test-only** (excluded, listed),
or **config plumbing** (not a builder, listed). Library-default semantics were answered from
library source in `~/.cargo/registry` where the component sets nothing: reqwest 0.13
(`timeout` = whole response body, `connect_timeout` = TCP connect, `read_timeout` = per-read
AND time-to-headers), lettre 0.11.23, kube-client 5.0.0, bollard 0.20.2, surrealdb 4.0.0,
opentelemetry-otlp 0.33.1, rmcp 3.9.0.

"Documented?" means `docs/**` or the crate `CONTEXT.md` — not code comments, not doc comments.

## The table — production outbound clients

| # | crate | builder site (`file:line`) | constant | what it bounds | documented? | error text names the limit that fired? |
|---|-------|----------------------------|----------|----------------|-------------|----------------------------------------|
| 1 | camel-component-llm | `src/lib.rs:117-133` `hardened_http_client_with_policy` (chat via `hardened_http_client_with_pinning`) | connect 10s; read-idle 600s (chat), none (decision) | TCP connect; transport read-idle. No client-wide total — configured `timeout_secs` owns the total (request-level) | YES — `docs/src/components/llm.md` + ADR-0021 + `llm-timeout-deadline` change + spec `llm-provider-adapter` Timeout contract | YES — `TransportTimeout { phase, after }` names the layer that lapsed (rc-9pfby fix, `f862b0f2b`) |
| 2 | camel-component-llm | `src/decision/http.rs:93` | request `.timeout(self.timeout)` from `timeout_secs` config | per-request whole response | YES | YES |
| 3 | camel-component-keycloak | `src/lib.rs:226-237` `hardened_http_client` | connect 10s, client total 30s, redirect none | TCP connect; every admin/consumer request (no per-request override, no config key) | YES — `docs/src/components/keycloak.md:169` | NO — `"keycloak admin request failed: {e}"`, `"events HTTP error: {e}"` name neither constant (vague, not wrong) |
| 4 | camel-component-keycloak | `src/uma.rs:69-70` (via `build_ssrf_pinned_client`) | connect 5s, request 30s | TCP connect; whole UMA token POST | YES — `docs/src/components/keycloak.md:165` | NO — `"UMA permission request failed: {e}"` (vague) |
| 5 | camel-auth | `src/http_client.rs:95-106` `build_ssrf_pinned_client` | from `SsrfClientOptions` (values set at call sites, rows 6-8) | plumbing | — (plumbing; build error honest: `"failed to build {label} HTTP client: {e}"`) | — |
| 6 | camel-auth (DNS pin) | `src/http_client.rs:64-70` | 5s DNS-resolution budget | hostname resolution before IP pinning | NO | YES — `"{label} DNS resolution timed out (5s)"` names the budget |
| 7 | camel-auth (oauth2) | `src/oauth2.rs:113-116` | connect 10s, request 30s | TCP connect; whole token POST | NO | NO — `"OAuth2 request failed: {e}"` |
| 8 | camel-auth (jwks) | `src/jwks.rs:72-76` | connect 5s, request 10s | TCP connect; whole JWKS GET | NO | NO — `"JWKS fetch failed: {e}"` |
| 9 | camel-auth (introspection) | `src/introspection.rs:89-95` | connect 5s, request 10s | TCP connect; whole introspection POST | NO | NO — `"introspection request failed: {e}"` (a second, `#[doc(hidden)] new_unchecked_for_test` at `introspection.rs:105-125` hardcodes the same 5/10 and is production-compiled despite the name) |
| 10 | camel-http | `src/tls.rs:39-58` `client_builder` | connect = `config.connect_timeout_ms` (default 5000ms), pool limits, redirect none, optional DNS pin | TCP connect only — no client-wide total/read timeout exists | YES — `docs/src/configuration/schema.md:67,82` (`connect_timeout_ms`) | build error honest |
| 11 | camel-http (DNS pin) | `src/ssrf.rs:137-143` | 5s DNS-resolution budget | hostname resolution before IP pinning | NO | YES — `"DNS resolution timed out (5s)"` |
| 12 | camel-http | `src/lib.rs:3493-3495`, `src/ssrf.rs:355` | request `.timeout(response_timeout)` from `responseTimeout` URI param (default: none = unbounded) | whole request incl. body (non-streaming) | NO — param absent from `docs/src/components/http.md` (in-code `#[uri_param]` desc only) | PARTIAL — `"HTTP request failed: {e}"` (reqwest says "operation timed out"; param is user-set) |
| 13 | camel-http | `src/lib.rs:3543-3579` | `read_timeout_ms` (default 30000ms) as tokio wrapper around body read; `max_response_bytes` guard | body-read budget (whole body, not per-read); size guard | NO — same docs gap as row 12 | YES — `"Read timeout after 30000ms"` names the exact configured value; size guard names both numbers |
| 14 | camel-opensearch | `src/producer/mod.rs:90-99` `TransportBuilder` | none on the transport (opensearch-rs reqwest client has no timeout set) | — | — (deadline lives in row 15) | — |
| 15 | camel-opensearch | `src/producer/mod.rs:26-38` `run_with_timeout_pe` | `timeout_ms` config (default 30000) as tokio wrapper | per-operation deadline | YES — `docs/src/components/opensearch.md:131` | PARTIAL — `"opensearch request timed out"` names the operation, not the value/key |
| 16 | camel-opensearch | `src/health.rs:19,66-70` | `PROBE_TIMEOUT` 10s hardcoded | cluster-health probe | NO | NO — unhealthy text does not name it |
| 17 | camel-component-mcp | `src/adapter/dns_pin.rs:196-205` `hardened_client_builder` + `:176-180` resolve; transport wired at `adapter/client.rs:72` | connect 5s (= `RESOLVE_TIMEOUT`, also the DNS-lookup budget); no total, no read timeout | TCP connect + DNS resolution only; rmcp tool calls are unbounded post-connect (rmcp 3.9.0 bounds control POSTs and session recovery at 5s defaults — not requests) | NO — `docs/src/components/mcp.md` has no timeout mention | PARTIAL — `"timed out resolving remote host '{host}'"` names the operation, not the 5s |
| 18 | camel-mail (IMAP) | `src/adapter/imap.rs:28-29,102-127` | `CONNECT_TIMEOUT` 30s, `IO_TIMEOUT` 60s; override exists but is `#[cfg(test)]`-only | whole connect; per-IMAP-operation IO | NO | NO — `"connect timed out"`, `"IMAP operation timed out"` name neither |
| 19 | camel-mail (SMTP) | `src/adapter/smtp.rs:122-161` `LettreSmtpTransport::new` | connect only: lettre 0.11.23 `DEFAULT_TIMEOUT` 60s (`lettre-0.11.23/.../smtp/mod.rs:239,268`) applied to TCP connect; nothing on post-connect command IO (MAIL FROM, DATA streaming); not surfaced in our config | TCP connect (60s); whole SMTP conversation after connect: **unbounded** | NO | NO — no post-connect limit exists to name |
| 20 | camel-cxf | `src/producer.rs:292-310` | `CamelCxfTimeoutMs` header > `timeout_ms` endpoint config > **0 = bridge default**: `normalizedTimeout = timeout_ms > 0 ? timeout_ms : bridgeConfig.connectionTimeoutMs()` (`CxfBridgeService.java:141`), and that SAME value is applied to BOTH `jakarta.xml.ws.client.connectionTimeout` and `client.receiveTimeout` (`CxfClientManager.java:304-306`); default 30000 (`cxf.connection.timeout.ms`, `BridgeConfig.java:50`). The 60000 `cxf.consumer.timeout.ms` (`BridgeConfig.java:56`) is the INBOUND consumer timeout, not the outbound receive | outbound SOAP call via Java bridge: connect and receive phases share one configurable-or-30000 value; 32-bit ms ceiling, rejected above 2147483647 | PARTIAL — `docs/src/components/ws-soap.md:129` documents `timeout_ms` but not the fall-through default or the connect/receive coupling | NO — failures surface as bridge gRPC status text |
| 21 | camel-cxf | `src/config.rs:13-15` | `bridge_start_timeout_ms` default 30000 | JVM bridge startup wait | YES — `ws-soap.md:197` (config key list) | startup failures name the budget |
| 22 | camel-otel | `src/service.rs:227-275` `build_{span,metric,log}_exporter` | none set — `with_tonic()`/`with_http()` + `RetryPolicy::disabled()`; otlp 0.33.1 default applies: **10s** (`OTEL_EXPORTER_OTLP_TIMEOUT_DEFAULT`, env `OTEL_EXPORTER_OTLP_TIMEOUT` honored) | each export POST/gRPC send | NO — `docs/src/services/metrics.md` silent | NO — exporter errors are opaque library text |
| 23 | camel-function | `src/protocol/client.rs:14-22` `ProtocolClient::new` | client total 30s, no connect timeout | health/register/unregister/shutdown requests; `invoke` overrides per-call with the function's own timeout (row 24) | NO — `CONTEXT.md` and docs silent on the 30s | NO — `"register request failed: {e}"` etc. |
| 24 | camel-function | `src/protocol/client.rs:101` | request `.timeout(timeout)` = function definition timeout (caller-supplied) | per-invoke whole response | YES — function `timeout` is a documented Function field | NO — `"invoke request failed: {e}"` (acceptable: caller chose the value) |
| 25 | camel-component-grpc | `src/producer/mod.rs:212-214` channel | `connect_timeout_ms` default 10000 (`config.rs:376-378`) | TCP/HTTP2 connect | NO — `docs/src/components/grpc.md` has no timeout mention | tonic status names connect failure class |
| 26 | camel-component-grpc | `src/producer/mod.rs:105-114,408,417-419` `effective_deadline` | `default_deadline_ms` default 30000 (`config.rs:380-382`); per-call `deadline_ms` Option overrides; `default_deadline_ms == 0` = opt-out; applied via `req.set_timeout` | deadline METADATA propagated to the server. Unary: tonic's local timer covers the whole response future. Streaming: the local timer covers the response future (headers/initial response) only — the body loop `stream.next()` (`producer/mod.rs:498-504`) has NO local timer, so enforcement after headers depends on the remote server honouring the propagated deadline. Per-message pacing not bounded locally | NO — same grpc.md gap | PARTIAL — tonic `DEADLINE_EXCEEDED` status names the class, not the configured value |
| 27 | camel-ws | `src/client_consumer.rs:298-309` | connect 10s (`connectTimeoutMs`, default 10000) | TCP/WebSocket connect | YES — `ws-soap.md:75` | YES — `"WebSocket connect timeout ({connect_timeout:?}) to {url}"` names the value |
| 28 | camel-ws | `src/lib.rs:2178` | reply 30s (`responseTimeoutMs`, default 30000) | wait for first reply in client mode | YES — `ws-soap.md:76` | names the phase |
| 29 | camel-component-surrealdb | `src/pool_factory.rs:150-161` `connect(endpoint)` (schemes `ws/wss/http/https/mem`, `pool_factory.rs:257-259`) | **none** — component sets no timeout; surrealdb 4.0.0 native HTTP engine builds `reqwest` via `ClientBuilder::new().default_headers(..).resolve(..)` with no timeout (`surrealdb-4.0.0/src/engine/remote/http/native.rs:65`) | nothing — connect, query, and signin are all unbounded (only attempt-count retry wraps connect) | NO | NO |
| 30 | camel-platform-kubernetes | `src/platform_service.rs:574` `Client::try_default()` | kube-client 5.0.0 defaults: connect 30s (`DEFAULT_CONNECT_TIMEOUT`), write 295s, read **None** (`kube-client-5.0.0/src/config/mod.rs:445-446`) | connect; write side; reads (leases/watches) unbounded by design (watch idle timeouts are the watcher's job) | NO | library status text |
| 31 | camel-function (container provider) | `src/provider/container.rs:96` `Docker::connect_with_local_defaults()` | bollard 0.20.2 `client_timeout` default 120s ("Default timeout for all requests is 2 minutes", `bollard-0.20.2/src/docker.rs:85`) | each Docker API request over the local socket | NO | bollard error text |
| 32 | camel-bridge | `src/channel.rs:21-44` `connect_channel` | **no wall-clock connect deadline** — 10 attempts × 100ms sleep bounds ATTEMPTS, not time; each `endpoint.connect().await` can stall on its own; no per-request deadline at this layer. Caller-level wrappers differ per stack: XSLT transform bounded 60s per RPC (`camel-xslt/src/client.rs:195`) but XSLT compile has NO deadline (`client.rs:161-175`, no outer caller budget); JMS send sets none (`camel-jms/src/producer.rs:220-224`); XSD validator sets none (`camel-validator/src/xsd_bridge.rs:664-684`, health wait 10s at `:212`); control traffic (announce, logs) is bounded by the startup budget | bridge control/data channel connect (localhost mTLS) | NO (bridge startup budget documented at `ws-soap.md:197`, channel itself not) | retry-loop exhaustion names transport error |
| 33 | camel-redis | `src/topology.rs:385-445` | redis `connection_timeout_secs` 10s (SentinelClientBuilder) | redis connection — **not HTTP**; listed for completeness | YES — `docs/src/components/redis.md:296` | classification is transient/timeout-aware |
| 34 | camel-container | `src/lib.rs:80-97` `connect_docker_from_host` | `DOCKER_CONNECT_TIMEOUT_SECS` = 120 (`src/lib.rs:152`) passed to `connect_with_socket`; `connect_with_local_defaults()` path relies on the bollard 120s default | Docker daemon connect AND each API request (bollard client_timeout) | NO — `docs/src/components/container.md` has no timeout mention | bollard error text |
| 35 | camel-xslt transform (bridge RPC) | `src/client.rs:195` | 60s gRPC deadline per transform RPC (`request.set_timeout`) | XSLT/XJ transform calls to the Java bridge. Compile/`compile_stylesheet` (`client.rs:161-175`) has NO deadline and no outer caller budget (the 30s tokio timeout at `component.rs:635` is test code) — see row 38 | NO — `docs/src/components/xml-transform.md` has no timeout/deadline mention | tonic `DEADLINE_EXCEEDED` class |
| 36 | camel-jms (bridge RPC) | `src/producer.rs:220-224` | **none** on the send RPC — `client.send(request).await` has no gRPC deadline and no tokio wrapper (health probes DO have `health_timeout_ms`, default 3s, documented `jms.md:267`) | Rust→bridge send hop; actual broker IO is the Java side's responsibility, but the RPC itself can stall a route indefinitely | PARTIAL — health deadline documented, send deadline does not exist | NO |
| 37 | camel-validator (bridge RPC) | `src/xsd_bridge.rs:664-684` `validate` (+ `register` at `:630-662`, `register_with_channel` at `:488`) | **none** on validate/register RPCs — `rpc.validate_with(...).await` has no gRPC deadline; only a 17 MiB size cap and transport-retry; health wait bounded 10s (`:212`) | Rust→bridge schema register/validate hops | NO | NO |
| 38 | camel-xslt compile (bridge RPC) | `src/client.rs:161-175` | **none** — `compile_stylesheet(...).await` has no gRPC deadline; no tokio wrapper at callers; the 30s timeout at `component.rs:635` is inside `#[cfg(test)]` | Rust→bridge stylesheet compile/recompile hop — can stall a route start indefinitely | NO | NO |

## Read-only reference (owned by mission 560 — do not edit)

- `camel-component-llm/**` rows 1-2 are recorded because they are the workspace's only fully
  honest pattern (constant documented in docs/ADR/spec, error names the firing deadline). They
  are the shape the deferred fixes below should converge on, not a copy target for other crates
  (`decision/http.rs`, `read_capped` is one valid decision path; Keycloak and mail own their
  own).

## Excluded from the table (with reasons)

- **Test-only builders**: `camel-health/src/server.rs:187` (`Client::new()` in `#[cfg(test)]`
  poller), `camel-api` multicast/dynamic-router timeout builder tests, `camel-xslt
  producer.rs:1226` (test-internal 5s), `camel-auth oauth2.rs:287+` (tests),
  `camel-prometheus/server.rs:231` (test), `camel-test/*`, `camel-integration-test
  adapters/http.rs` (harnesses), `camel-http tls_harness.rs` (harness).
- **Config plumbing, not builders**: `camel-dsl/src/compile.rs:2011`, `camel-builder
  lib.rs:1860`, `camel-api` builder tests.
- **Non-HTTP stacks** (mission scope is HTTP): redis-rs (row 33, documented anyway), MQTT
  (`camel-mqtt` rumqttc 10s connect — comment at `src/consumer.rs:133`), rustls provider
  installs in `camel-fake-bridge`.
- **Inbound/server-side bounds** (not outbound clients): MCP `TOOL_DISPATCH_TIMEOUT` 10s
  (`adapter/server.rs:91`), camel-http consumer read timeout.

## Classification rule (what is a finding)

A row is a **finding** if any of:
1. **Unbounded phase** — a production request phase with no deadline anywhere (rows 19
   post-connect, 29 whole, 17 post-connect, 36 send hop, 37 validate/register, 38 compile).
2. **Hidden cap** — a constant or library default bounds traffic and no doc states it (rows
   22, 20's fall-through default, 9's second constructor, 16, 31, 34, 18).
3. **Undocumented constant** — the code is honest but docs/CONTEXT never state the value
   (rows 7-9, 12-13 docs gap, 17, 23, 25-26 docs gap, 6, 11, 35).
4. **Dishonest error text** — the message names a value that is not the limit that fired.
   **Zero instances found.** (The `rc-9pfby` class is fixed as of `f862b0f2b`.)

NOT a finding by itself: configurable caps with honest text (rows 2, 10, 13, 15, 24, 27, 28,
33); vague-but-not-wrong texts on already-documented constants (rows 3-4 — explicitly
non-findings; if wording cleanup ever happens it rides along with future docs work, no
standalone issue is filed).

## Fixed vs deferred

**Fixed in this change: nothing.** The bounded-fix clause in the mission order *permits*
small fixes — typically an error message reporting the wrong limit — and requires them to be
unambiguous and behaviour-neutral. **No error-text fixes are selected**: the audit found zero
wrong-limit messages, and no other permitted wording fix is unambiguous enough to land here;
every defect above needs either a docs edit (a design decision: where and how much to
document) or a behaviour/design change (new config keys, new deadlines) → issues, not fixes.
Consistent with "Not a refactor mission".

**Deferred to bd** (created with `--deps discovered-from:rc-s8itn`, each quoting its row):

| priority | issue | rows | rationale |
|----------|-------|------|-----------|
| P2 | surrealdb calls fully unbounded (component + library set nothing) | 29 | data-plane, genuinely unbounded |
| P2 | mail SMTP unbounded after TCP connect (extends open rc-jpcqt) | 19 | ops-reported class, DATA phase unbounded |
| P2 | mcp tool calls unbounded post-connect | 17 | connect bounded at 5s, but the request phase can stall an Exchange exactly like SMTP/surrealdb — same class, same rank |
| P3 | camel-auth hardcoded timeouts undocumented/non-configurable, vague errors | 5-9 | includes its honest DNS cap for completeness |
| P3 | cxf timeout_ms fall-through to undocumented bridge phase defaults | 20 | |
| P3 | http.md missing responseTimeout/readTimeout producer params (and DNS cap row) | 12-13, 11 | docs gap on the most-used component |
| P3 | jms send RPC unbounded at the Rust→bridge hop | 36 | |
| P3 | validator validate/register RPCs unbounded | 37 | |
| P3 | xslt compile RPC unbounded (transform is bounded at 60s) | 38 | |
| P3 | otel hidden 10s OTLP default, undocumented, opaque errors | 22 | |
| P4 | grpc.md missing connect/deadline params | 25-26 | configurable + bounded; docs gap only |
| P4 | function ProtocolClient 30s undocumented | 23-24 | |
| P4 | opensearch health probe 10s undocumented; timeout text could name value | 15-16 | |
| P4 | kubernetes leadership client: library defaults undocumented, read unbounded by design | 30 | control plane |
| P4 | docker client defaults undocumented (camel-container + camel-function provider) | 31, 34 | |
| P4 | xslt 60s transform deadline undocumented | 35 | bounded; docs gap |
| P4 | camel-bridge channel connect semantics undocumented | 32 | localhost-only |

Rows 3-4 (keycloak) are explicitly non-findings per the classification rule; no issue is
filed for them.

`rc-jpcqt` (open) is **still accurate in substance** — IMAP/SMTP timeouts are not
configurable — but its quoted constants are stale: code has connect **30s** / io **60s**
(`imap.rs:28-29`), the issue says "fixed 60s connect/io". A comment on the issue records this,
and the SMTP post-connect-unbounded finding is filed as a separate issue linked to it rather
than a duplicate. `rc-9pfby` (closed) verified fixed in the tree (rows 1-2); linked, not
duplicated.

## `.openspec.yaml`

`skip_specs: true`. Spec canons do own timeout contracts per component —
`openspec/specs/llm-provider-adapter/spec.md:147-153` (Timeout contract),
`openspec/specs/mail-component/spec.md` — but no canon owns a workspace-wide client
hardening/timeout policy, and this change modifies no behaviour, so no delta spec is
authored. If the fleet later wants a cross-crate client policy canon, that is a separate
spec change informed by this table.
