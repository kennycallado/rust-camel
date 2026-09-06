# Tasks: http-contract-surface

## camel-component-http

### Task 1.1: Query composition replaces the CamelHttpQuery silent drop

Covers spec requirements: Outbound query composition (all 5 scenarios) and
Override URI query merge (both scenarios).

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)

**Steps:**
1. Add `fn merge_header_query(higher_precedence: Option<&str>, header_query: &str) -> Result<Option<String>, CamelError>` near `resolve_endpoint_query` (lib.rs:2581). Implementation: if `header_query.is_empty()` return the higher-precedence option unchanged (no extra `?`). Otherwise parse the higher-precedence query with `raw_query_pairs` collecting keys, parse `header_query` with `raw_query_pairs`, call `validate_raw_query_span` on each header span (a forbidden byte errors naming the byte), append header spans whose key is absent from the higher-precedence key set (higher-precedence wins collisions), join with `&`. Return `Ok(None)` when the composed result is empty.
2. Rewrite the base arm of `resolve_url` (lib.rs:2544-2559): when `CamelHttpQuery` is present, call `merge_header_query(resolve_endpoint_query(config)?.as_deref(), query)` and attach the result with a single `?` when `Some`; remove the verbatim early-return that skipped the endpoint query.
3. Rewrite the `CamelHttpUri` arm (lib.rs:2504-2529): split the override URI into base-without-query and its raw query (split at the first `?`; when the override URI has no `?`, the higher-precedence query is `None`, not `Some("")` — an empty `CamelHttpQuery` header then leaves no query component at all); append `CamelHttpPath` to the base-without-query exactly as today (slash fix-up preserved); reattach the result of `merge_header_query(override_query, header_query)` with a single `?` when `Some`. This removes the unconditional `url.push('?')` that produced `?a=1?b=2`.
4. Reconcile the Wave-A pin `resolve_url_camel_http_query_stays_verbatim` (lib.rs:7607): under composition, endpoint `http://h/p?x=1` + header `userFilter=a%2Cb` now resolves to `http://h/p?x=1&userFilter=a%2Cb`. Update the expected URL to the composed form, KEEPING the assertion that the header span bytes `a%2Cb` ride verbatim (not re-encoded). Add a comment in the test noting the policy change (ADR-0071).
5. Verify a test exists asserting that `bridgeEndpoint=true` ignores `CamelHttpUri`, `CamelHttpPath`, and `CamelHttpQuery` WITH an endpoint raw query (existing `resolve_url_bridge_endpoint_true_ignores_camel_http_uri` at lib.rs:7488 covers the headers but on an endpoint without its own query — the spec scenario requires "endpoint base URL plus its own query is sent"); if no test covers that combination, add `bridge_endpoint_ignores_url_headers` asserting the endpoint query rides while all three headers are ignored.

**Tests:** (all under `cargo test -p camel-component-http --lib`, expected to FAIL before step 1-3 and PASS after)
- `header_composes_with_endpoint_query`: endpoint base `http://upstream/api` with raw query `apiKey=secret&lang=en`, exchange header `CamelHttpQuery=lang=es&page=2` → `resolve_url` returns `http://upstream/api?apiKey=secret&lang=en&page=2` (`lang` NOT replaced).
- `header_alone_still_rides`: endpoint `http://upstream/api` with no query, header `CamelHttpQuery=page=2` → `http://upstream/api?page=2`.
- `empty_reflected_query_leaves_endpoint_query_intact`: endpoint raw query `apiKey=secret`, header `CamelHttpQuery=` (empty string) → `http://upstream/api?apiKey=secret`, no trailing `?`.
- `forbidden_byte_in_header_query_errors`: header `CamelHttpQuery=q=ab<cd` → `resolve_url` returns `Err` whose message names the byte `<`; no URL emitted.
- `override_uri_with_query_plus_header_query`: headers `CamelHttpUri=http://host/api?a=1` + `CamelHttpQuery=a=2&b=3` → `http://host/api?a=1&b=3` (single `?`).
- `path_applies_before_query_composition`: headers `CamelHttpUri=http://host/api?a=1` + `CamelHttpPath=/extra` + `CamelHttpQuery=b=2` → `http://host/api/extra?a=1&b=2`.
- `plain_proxy_reflection_composes`: endpoint `http://upstream/api?apiKey=secret`, exchange headers `CamelHttpPath=/in/extra` + `CamelHttpQuery=page=2` (as the consumer installs from the wire) → `http://upstream/api/in/extra?apiKey=secret&page=2`.
- `bridge_endpoint_ignores_url_headers` (verify-or-add): endpoint `bridgeEndpoint=true` with raw query, exchange carrying all three URL headers → headers ignored, endpoint base + own query sent.

**Acceptance:**
- `cargo test -p camel-component-http --lib` exits 0 (new tests above pass; the only pre-existing test modified is `resolve_url_camel_http_query_stays_verbatim`, reconciled per step 4; `test_http_producer_uri_override` unchanged and green).
- `cargo clippy -p camel-component-http -- -D warnings` exits 0.
- `cargo fmt --check -p camel-component-http` exits 0.

- [x] 1.1

### Task 1.2: CamelHttpUri host fence — allowedUriHosts option

Covers spec requirement: CamelHttpUri host fence (all 4 scenarios).

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)

**Steps:**
1. Define `struct AllowedUriHost { host: String, port: Option<u16> }` and `fn parse_allowed_uri_hosts(raw: &str) -> Result<Vec<AllowedUriHost>, CamelError>`: split on `,`, trim segments, drop empty segments; each remaining segment parses as `host` or `host:port` (bracketed IPv6 like `[::1]:8443` supported; bare `[::1]` host-only); DNS names lowercased; IPv6 kept in bracketed canonical form. A segment that fails to parse is `Err` (malformed entry → endpoint creation fails). A `raw` that yields zero valid entries (e.g. `allowedUriHosts=,,`) is also `Err`.
2. Register `allowedUriHosts` as a `#[uri_param]` String on `HttpEndpointUriConfig` (parity pin test `endpoint_uri_options_count_parity` at lib.rs:9396 updates its expected count from 21 to 22). Parse it into `allowed_uri_hosts: Option<Vec<AllowedUriHost>>` on `HttpEndpointConfig` via `parse_allowed_uri_hosts` at endpoint construction — parse failure fails endpoint creation. Do NOT add a typed field to `HttpEndpointUriConfig` beyond the macro-generated one; the parsed vector lives on `HttpEndpointConfig` only.
3. Add `fn uri_host_allowed(url_str: &str, fence: &[AllowedUriHost]) -> Result<bool, CamelError>`: parse `url_str` with `url::Url::parse` (parse failure or no host → `Ok(false)`); extract host (lowercased for DNS; bracketed string for IPv6) and effective port (`url.port()` or the scheme default: 443 https, 80 http). Match: host-only entry allows any port; `host:port` matches only equal effective port. 
4. In the `CamelHttpUri` arm of `resolve_url`, when `config.allowed_uri_hosts` is `Some(fence)`: call `uri_host_allowed(uri, fence)` on the raw `CamelHttpUri` header value before any path/query assembly; on `Ok(false)` return an error of variant `CamelError::ProcessorError` whose message includes the URL rendered ONLY through `redact_url_for_diagnostics`. Unarmed endpoints skip the check entirely.
5. Run `cargo xtask schema --check`; if it reports drift caused by the new `uri_options` entry, regenerate with `cargo xtask schema` and stage the regenerated schema in the same commit.

**Tests:** (all under `cargo test -p camel-component-http --lib`)
- `armed_fence_rejects_unknown_host_redacted`: endpoint with `allowedUriHosts=api.internal:8443,cdn.example.com`, header `CamelHttpUri=http://user:pass@evil.example.com/x?token=s3cret` → `resolve_url` `Err`; assert the error message does NOT contain `pass` nor `s3cret`.
- `armed_fence_allows_listed_host`: same endpoint, header `CamelHttpUri=http://cdn.example.com/x` → `http://cdn.example.com/x`.
- `host_only_entry_permits_any_port`: endpoint with `allowedUriHosts=cdn.example.com`, header `CamelHttpUri=http://cdn.example.com:9443/x` → override honored.
- `unarmed_endpoint_unchanged`: endpoint without the option, header `CamelHttpUri=http://any.example.com/path` → override honored (companion to existing `test_http_producer_uri_override`, which must also stay green unmodified).
- `empty_allowlist_fails_endpoint_creation`: endpoint URI with `allowedUriHosts=,,` → endpoint construction returns `Err`.
- `malformed_entry_fails_endpoint_creation`: `allowedUriHosts=not a host!` → endpoint construction `Err`.
- `fence_option_is_consumed`: endpoint with `allowedUriHosts=cdn.example.com` AND a raw query on the base URI; no override header; assert the outbound query does not contain `allowedUriHosts`.
- `endpoint_uri_options_count_parity`: now expects 22 (update the existing pin).

**Acceptance:**
- `cargo test -p camel-component-http --lib` exits 0.
- `cargo clippy -p camel-component-http -- -D warnings` exits 0.
- `cargo xtask schema --check` exits 0 (after regeneration if needed).
- `cargo xtask lint-non-exhaustive` exits 0.

- [x] 1.2

## docs

### Task 1.3: ADR-0071 + camel-http CONTEXT.md contract surface

Covers: documentation of all four requirements; the Default inbound
reflection requirement is documented behavior (its executable scenario is
owned by Task 1.1's `plain_proxy_reflection_composes`).

**Files:**
- `docs/adr/0071-http-outbound-url-policy.md` (new)
- `crates/components/camel-http/CONTEXT.md` (modified)

**Steps:**
1. Write `docs/adr/0071-http-outbound-url-policy.md` following the section structure of `docs/adr/0070-staged-listener-port-determinism.md` (read it first as the house-format reference). Content: context (three bd findings rc-rbfxq/rc-k3pir/rc-69fie, ADR-0032 untrusted exchange data, ADR-0034 control-bus fence precedent); decision — (a) outbound query composition with arm-specific higher-precedence source, deliberately diverging from Apache Camel header-wins-verbatim; (b) default inbound reflection retained (parity) with the composition rule; (c) opt-in `allowedUriHosts` fence, fail-closed when armed; the explicit compatibility exception versus ADR-0034's mandatory fence and ADR-0032's per-route validation rule (unarmed endpoints keep pre-fence override behavior — hardening, not incident response); consequences (migration note for Apache Camel users relying on header-wins, fence adoption path, Wire-A law references).
2. Amend `crates/components/camel-http/CONTEXT.md` section "Outbound query fidelity": replace the CamelHttpQuery precedence bullet (currently "applied verbatim, wins over the endpoint's raw/programmatic query base") with the composition rule and arm-specific higher-precedence semantics; add the empty-header rule; add the override-URI merge fix (no double `?`); add the fence paragraph next to the `bridgeEndpoint` bullet (option semantics, matching rules, fail-closed, redacted rejection); name the default inbound reflection (consumer-installed `CamelHttpPath`/`CamelHttpQuery` ride by default and compose per the rule).
3. Cross-reference ADR-0071 from the amended CONTEXT.md paragraphs.

**Tests:**
- Docs task — executable checks are the gates: `cargo xtask lint-context-citations` exits 0 (CONTEXT.md citations well-formed).

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- ADR file exists, English, house format, cites rc-rbfxq/rc-k3pir/rc-69fie and ADR-0032/0034/0071-external references correctly.
- CONTEXT.md contains the composition rule, the reflection default, and the fence documentation; no stale "applied verbatim, wins over" text remains (`grep -c "wins over the endpoint" crates/components/camel-http/CONTEXT.md` returns 0).

- [x] 1.3
