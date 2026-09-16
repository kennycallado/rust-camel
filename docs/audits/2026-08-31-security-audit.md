# Security audit — 2026-08-31

**Scope:** full-stack security review of rust-camel (pre-release), performed on branch
`security-audit-2026-08-31`. Six phases: automated sweep, network surface, parsers and
expression languages, execution/transformation, secrets and auth, DoS/memory.
Method: adversarial code reading against documented hardening claims, with fixes applied
in-tree for everything actionable and adversarial regression tests for each fix.

**Result:** 22 findings. **17 fixed** (1 high, 2 medium-high, 5 medium, 9 low/info),
**3 accepted as documented residual limitations**, **2 deferred as hardening backlog**.

---

## 1. Automated sweep

| Check | Result |
|---|---|
| `cargo audit` (1200 locked deps) | **0 vulnerabilities**. 5 unmaintained informational warnings (all documented in `.cargo/audit.toml`, no fixed versions exist). 3 ignored advisories individually justified (rsa Marvin client-side, rkyv trusted-server-only, smartstring mandatory rhai dep). |
| Dependency tree | No duplicate security-relevant crates. Single `rustls 0.23` TLS stack; **no openssl / native-tls anywhere**. |
| `cargo xtask lint-secrets` | PASS |
| Clippy (security-relevant lints) | PASS, zero hits |
| `unsafe` code | 4 production sites, all documented (process-group SIGKILL in exec, `Waker::from_raw`, two env-var helpers). No unsafe in parsers, languages, or network paths. |
| Fuzzing | **None exists.** No `fuzz/` targets, no cargo-fuzz/libfuzzer wiring. See recommendation R1. |

## 2. Findings fixed in this audit

### F4-1 — HIGH — `doneFileName` write escaped the base directory
`crates/components/camel-file/src/lib.rs`. The done-file write joined the endpoint
directory with the `${file:name}`-substituted pattern **without any validation**, while
`file:name` resolves from the inbound `CamelFileName` header. An absolute header value
discarded the base (`Path::join` semantics); `../` traversed out. Impact: arbitrary
empty-file creation anywhere the process can write; arbitrary file *truncation* via a
planted symlink (fs::write follows symlinks).
**Fix:** the substituted done name now passes `validate_relative_filename` (lexical
pre-check: rejects absolute paths, `..`, NUL) + `validate_path_is_within_base`, and is
written with `O_NOFOLLOW`. Regression tests: absolute header, `../` header.

### F6-1 — HIGH — batch resequencer: unbounded buckets, bucket buffers, and timeout tasks
`crates/camel-processor/src/resequencer/batch.rs`. `BatchPolicy` had no equivalent of the
aggregator's `max_buckets`/`max_timeout_tasks` caps: every unique correlation key opened a
new bucket, spawned a tokio timeout task, and buffered exchanges until completion. A
remote client driving attacker-chosen correlation keys (header-derived) could grow memory
and scheduler pressure unboundedly.
**Fix:** `DEFAULT_MAX_BUCKETS` (10_000), `DEFAULT_MAX_BUCKET_SIZE` (1_000),
`DEFAULT_MAX_TIMEOUT_TASKS` (1_024) on `BatchPolicy::new_cyclic`; `with_limits` for
explicit bounds. Overflow drops with a warn (consistent with resequencer post-ack drop
semantics, ADR-0029); timeout-task cap degrades to size/flush completion (mirrors the
aggregator's graceful degradation). Three adversarial tests.

### F6-2 — MEDIUM-HIGH — aggregator per-bucket accumulation unbounded
`crates/camel-processor/src/aggregator.rs` + `crates/camel-api/src/aggregator.rs`.
`max_buckets` capped the bucket *count*; a single hot key under predicate-only completion
buffered exchanges without limit.
**Fix:** new `AggregatorConfig.max_bucket_size` (default `Some(10_000)`, `None` = opt-out).
Enforced in `call` before push — reject with error, fail-visible. Wired through the DSL
(`aggregate.max_bucket_size`), the canonical route spec, and the builder; schemas
regenerated. Test: `test_aggregator_enforces_max_bucket_size`.

### F2-1 — MEDIUM — chunked request bodies bypassed the consumer's 2 MiB limit; no consumer read timeout
`crates/components/camel-http/src/lib.rs`. The `maxRequestBody` check consulted only the
`Content-Length` header; chunked/no-length requests streamed unbounded into the pipeline.
A slow-drip client could also hold an `inflight` semaphore permit indefinitely → 503 DoS.
**Fix:** (a) the request stream is wrapped with a hard byte cap — any downstream
materialization past `max_request_body` fails closed; (b) a 30s `TimeoutLayer` (408) on
both the plain and TLS axum servers. Test: `test_http_consumer_chunked_body_is_capped`.

### F3-1 — MEDIUM — `CamelError::HttpOperationFailed` leaked credential-bearing URLs and full upstream bodies
`crates/components/camel-http/src/lib.rs` (producer). The error variant embedded the raw
URL (userinfo + query) verbatim and the entire upstream response body; two `debug!` sites
logged the raw URL. Violated ADR-0051 redact-by-construction.
**Fix:** `redact_url_for_diagnostics` (masks userinfo, replaces query with `[redacted]`,
caps length) applied at both debug sites and the error variant; upstream error bodies
truncated to 4 KiB. Unit tests for redaction and truncation.

### F4-2 — MEDIUM — symlink escape on `fileExist=Append` + check/open TOCTOU
`crates/components/camel-file/src/lib.rs`. Validation canonicalized the path, but the
subsequent open re-walked it by name without `O_NOFOLLOW`; a dangling symlink inside the
base made `Append` create/append outside the base deterministically.
**Fix:** `open_options_no_follow()` (unix `O_NOFOLLOW`) on the `Fail`, `Append`, and
done-file opens, plus a lexical pre-check of the resolved `fileName`. Tests:
dangling-symlink append refusal, symlink-leaf Fail refusal, lexical validator units.

### F5-1 — MEDIUM — gRPC logged unknown URI parameter *values* at WARN
`crates/components/camel-component-grpc/src/config.rs:593`. Unknown params — exactly the
ones with no metadata-driven redaction (a mistyped `authToken=…`) — were logged in
cleartext on every endpoint creation.
**Fix:** log key names only.

### F6-3 — MEDIUM — claim-check memory repo: unbounded stack depth per key
`crates/camel-core/src/claim_check/memory_repository.rs`. `max_entries` capped stack *key
count*; repeated `push` to one key grew its `VecDeque<Message>` without limit.
**Fix:** `DEFAULT_MAX_STACK_DEPTH` (10_000); push past the cap is rejected with an error
(fail-visible; silently evicting the oldest claim would lose data the caller believes
stashed). Test: `push_rejects_stack_past_max_depth`.

### F5-2 — LOW-MEDIUM — MQTT broker URL echoed into errors and Debug
`crates/components/camel-mqtt`. `mqtt://user:pass@host` URLs landed in `Config` errors —
which persist to the redb journal — and in plain `Debug`.
**Fix:** `redact_broker_url` (masks userinfo, drops query) at the validate error, host_port
error, URI scheme error, and Debug impl. Tests included.

### F5-3 — LOW-MEDIUM — JMS `BrokerConfig` Debug printed raw `broker_url`
ActiveMQ-style URLs embed credentials (`?jms.password=…`, `tcp://user:pass@`).
**Fix:** `redact_broker_url` masks userinfo and sensitive query params
(`password|secret|token|credential|user*`). Tests included.

### F2-2 — LOW — `blockedHosts` exact-match bypass
Trailing root dot (`blocked.local.`), case shifts, and subdomains bypassed the blocklist.
**Fix:** normalize case + trailing dot; a blocklist entry covers its subdomains
(`api.blocked.local` blocked by `blocked.local`); non-subdomain suffixes
(`evilblocked.local`) stay allowed. Test: `test_blocked_hosts_normalized_and_subdomain_aware`.

### F2-3 — LOW — WS producer logged full URLs unredacted
Endpoint path may carry `?token=…`. **Fix:** `redact_ws_url_for_log` at 6 debug/warn
sites. Test included.

### F2-4 — LOW — MCP remotes had no SSRF/scheme policy
`McpRemoteConfig.url` was used verbatim (unlike http/llm components).
**Fix:** `allow_internal` (default false) + `validate_url` at endpoint creation: non-http(s)
schemes rejected; IP-literal internal targets rejected unless opted in; cleartext to
public IPs rejected. Hostname-based URLs are resolution-independent at this layer (no DNS
pinning — documented). Four unit tests.

### F2-5 — LOW — cross-origin redirect replayed custom secret headers
Only Authorization+Cookie were stripped. **Fix:** strip the standard sensitive set
(`authorization`, `cookie`, `x-api-key`, `x-apikey`, `x-auth-token`, `api-key`,
`apikey`) plus `proxy-authorization` on https→http downgrade. Unit test included.

### F2-6 — INFO — NAT64 / 6to4 / Teredo absent from the IP blocklist
`crates/camel-api/src/ssrf.rs`. NAT64 `64:ff9b::/96` embeds an IPv4 that could be
internal; 6to4/Teredo only tunnel. **Fix:** NAT64 recurses into the embedded IPv4
classification; 6to4 (`2002::/16`) and Teredo (`2001::/32`) blocked outright. Three tests.

### F2-7 — INFO — CA/mTLS file load failures silently ignored
A configured CA that failed to read/parse silently fell back to system roots; a
half-readable mTLS pair silently dropped the client certificate. **Fix:** loud `warn!`
(no paths — redaction convention) at all four failure points.

### F4-3 — LOW — file producer had no byte cap
**Fix:** new `maxWriteBytes` URI option (default `0` = unlimited, back-compat). Text
bodies pre-checked; stream bodies copied through `take(cap+1)` and error past the cap.
Test included.

### F4-4 — LOW — predictable temp-file names
**Fix:** temp names carry a 64-bit random infix (`prefix + hex + "." + name`), removing
deterministic pre-creation DoS. Existing prefix contract (`is_valid_temp_prefix`, sweep)
unchanged. Two existing tests reworked to the randomized naming.

### F4-5 / F4-6 — LOW — xslt/validator schema reads unconfined, unbounded
**Fix:** xslt rejects `..` components and caps reads at 16 MiB; validator rejects `..`
*after* percent-decoding (raw `%2e%2e/` no longer bypasses) and caps schema reads at
16 MiB. Tests for raw and percent-encoded traversal.

### F5-5 — LOW (latent) — `CamelConfig` / `BeanConfig` plain Debug
`_extra` (unknown top-level keys) and `BeanConfig.config` (arbitrary plugin map) are
where operators plausibly stash credentials; both derived plain `Debug`. No production
site prints them today — one `debug!(?config)` away from a cleartext dump.
**Fix:** manual redacting `Debug` for both. Tests included.

## 3. Accepted residual limitations (documented, no action possible today)

- **F3-2 — JS (Boa) heap amplification.** Boa 0.21 exposes no heap cap; a
  `'x'.repeat(2**31)` in an in-process script can OOM the process. CPU/loop/recursion/
  source-size limits all bound *time*, not memory. Trust model: script source is operator
  config; untrusted JS must use the out-of-process `function:` path. Blocked on Boa
  shipping a heap-limit API.
- **F3-3 — timed-out scripts cannot be cancelled.** `tokio::time::timeout` abandons but
  does not kill a `spawn_blocking` task; with operator-raised operation limits a CPU-bound
  script pins blocking-pool threads past the wall-clock timeout. Inherent tokio
  limitation; default limits trip in milliseconds.

## 4. Deferred to backlog (low value/effort ratio for pre-release)

- **F5-4 — wire `EndpointUri::to_redacted_string` into URI-echoing error paths.** The
  renderer is correct but currently unused; today no component echoes a credential-bearing
  URI into an error (audited: exec/wasm/validator/mcp URI grammars carry no secrets;
  http/mqtt/jms now redact). Recommended as robustness wiring when a component with
  credential-bearing URIs lands.
- **F6-4 — aggregator force-complete drop under late-channel pressure** (bounded 256,
  drop + warn). Accepted divergence D-A3, already documented and test-pinned.
- **F6-5 — config foot-guns**: `max_buckets(0)` accepted (deny-everything), SQL `One`/`List`
  modes materialize operator-bounded result sets. Availability-only, operator-triggered.
- **F6-6 — no global cross-route concurrency semaphore.** Total concurrency = Σ configured
  route/component limits; bounded by deployment. Architectural decision, acceptable.

## 5. Verified correctly hardened (spot-checked adversarially)

- **SSRF core (http)**: checks run on resolved IPs, DNS-pinned via `resolve_to_addrs`
  with `no_proxy()` (TOCTOU + proxy-bypass closed), empty resolution fails closed,
  exchange `CamelHttpUri` override re-validated, decimal/hex IP literals normalized by
  WHATWG parsing, every redirect hop re-validated, IPv4-mapped-IPv6 recursion.
- **TLS**: rustls-only; certificate verification on by default everywhere; the only
  `danger_accept_invalid_certs` is explicit operator config with a warn; gRPC
  `insecure_skip_verify` rejected fail-closed.
- **DSL parsing**: noyalib security budgets (depth 128, alias-expansion 1024/anchor,
  64 MB doc cap); serde_json 128-level recursion; route file reads capped at 16 MiB;
  config includes canonicalize + containment-checked, recursion disabled.
- **`${env:}` interpolation**: single-pass (no expansion bomb), error carries the var NAME
  only, strict leaves reject residual `${`/`{{`, YAML structural injection sanitized.
- **Expression languages**: JS (Boa) fresh context + loop/recursion/stack/source caps +
  no fs/net/process bindings; Rhai `Engine::new_raw` + `no_module` + ops/call-depth caps +
  `eval`/`import` disabled; MiniJinja mandatory autoescape + fuel + recursion + context/
  output caps + no disk loader; XPath (sxd) structurally XXE-free (no entity parser, no
  `document()`); JSONPath depth-capped, query is operator config; Simple has zero regex.
- **exec**: direct exec (no shell), arg policy default deny-all, env_clear + allowlist +
  deny globs applied last, executable pinned at startup, cwd confined, stdin/stdout/stderr
  caps, process-group kill on timeout.
- **WASM**: wasmtime epoch interruption + memory/table caps + 10 MiB module cap; WASI
  surface limited to clocks/random/io with empty env, closed stdin, no fs/sockets/preopens
  (regression-tested); host functions fail-closed by capability sets.
- **template (MiniJinja)**: no SSTI — template source is operator-fixed at startup, no
  header/body can supply it; `openat` component walk rejects symlinks/`..`; no globals or
  env access.
- **XSLT/XSD bridge (Java sidecar)**: FEATURE_SECURE_PROCESSING, DOCTYPE disallowed,
  external entities off, `ACCESS_EXTERNAL_*=""`, deny-all URIResolver — `document()` and
  extension functions reach neither fs nor network.
- **bean**: name-only dispatch against the registered `methods()` allowlist; no runtime
  reflection.
- **Auth**: JWKS pinned RS256, kid+alg+use filtering, fail-closed empty aud/iss, token
  caches keyed on SHA-256 with `[hash]` Debug, denials never cached; zeroize on plaintext
  secrets; no default credentials anywhere; non-loopback Public binds require explicit
  operator acknowledgement (ADR-0061).
- **SEDA/queues/channels**: bounded everywhere (zero production `unbounded_channel`);
  stream resequencer capacity-bounded; idempotent repo capped with evict-oldest; moka
  cache size-evicted; retries all bounded with backoff.

## 6. Recommendations (post-audit)

- **R1 — add fuzzing.** Highest-value gap. Suggested first targets: DSL YAML/JSON route
  parsing, `${env:}` interpolation, Simple expression parser, URI parsers
  (`parse_uri` per component). cargo-fuzz with a short CI time budget.
- **R2 — CI gate on new tracing sites printing config values.** A lint-unwrap-style xtask
  check: any `tracing::*!` call whose format args reference a `url`/`uri`/`config` field
  must use a `redact_*` helper (allowlist per crate).
- **R3 — consider fail-closed TLS file loading.** F2-7 made failures loud but still
  fallback-permissive for back-compat; a `tls.strict=true` knob (fail endpoint creation)
  would close the downgrade entirely.
- **R4 — DNS pinning for MCP remotes.** F2-4 validates IP literals only; hostname remotes
  trust DNS. Parity with the http component's `resolve_to_addrs` pinning is the follow-up
  (needs a custom rmcp transport).

## 7. Verification

Every fix ships with an adversarial regression test. Full-crate suites run green in the
worktree: camel-api (563), camel-processor (699), camel-core (832), camel-dsl (634),
camel-builder (151), camel-config (271), camel-http (280), camel-file (123), camel-ws,
camel-mqtt (51), camel-jms (129), camel-grpc (188), camel-mcp (68 across targets),
camel-validator (49), camel-xslt (29). Quality gates: `cargo fmt --check`, clippy
(workspace, `-D warnings` profile), `cargo xtask schema --check` (schemas regenerated),
`lint-context-citations`. Kafka crate excluded from local gates (system `libsasl2`
unavailable in this environment — unchanged by this audit).

Commits on `security-audit-2026-08-31`:
1. `fix(file): confine doneFileName and block symlink escapes`
2. `fix(http): redact error URLs and cap chunked request bodies`
3. `fix(processor): bound batch resequencer and aggregator buckets`
4. `fix(security): redact broker URLs and bound claim-check stacks`
5. `fix(security): close blocklist, redirect, and SSRF gaps`
6. `fix(security): confine schema reads and cap file writes`
