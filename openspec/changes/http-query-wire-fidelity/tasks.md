# Tasks: http-query-wire-fidelity

## camel-endpoint

### Task 1: `raw_query` capture + public raw-pair iterator on `UriComponents`

**Files:**
- `crates/camel-endpoint/src/uri.rs` (modified)
- `crates/camel-endpoint/src/config.rs` (modified — exhaustive literal at :208)
- `crates/camel-endpoint/tests/endpoint_macros_derive_integration_test.rs` (modified — exhaustive literal at :365)
- `crates/components/camel-component-api/src/lib.rs` (modified — re-export line :60)

**Steps:**
1. Add `pub raw_query: Option<String>` to `UriComponents` (uri.rs:9-15), documented: verbatim authored query bytes, `Some("")` for a bare `?`, `None` when absent.
2. Capture it in `parse_uri` (uri.rs:99): split the raw query substring off before `parse_query` consumes it; store the exact authored byte string. Do not touch `parse_query` semantics — duplicate-key rejection (uri.rs:152-157) stays exactly as is.
3. Export a raw-pair iterator: `pub fn raw_query_pairs(query: &str) -> Result<Vec<(String, &str)>, CamelError>` in uri.rs (DECISIVE per verified source: `percent_decode` at uri.rs:50-75 returns Result for malformed escapes/UTF-8 — key decoding is genuinely fallible), built on the existing `split_query_pairs` (uri.rs:185-217). Each element = `(decoded_key, raw_pair_span)` where `decoded_key` percent-decodes the KEY portion only (so `connect%54imeout` decodes to `connectTimeout`); `raw_pair_span` is the original `&str` slice of the full `key=value` pair — VALUES are never decoded here. A malformed percent-escape in a key returns `Err(CamelError::InvalidUri)` naming the key. Doc comment names its consumer (camel-http raw filter).
4. Extend the camel-endpoint re-export in camel-component-api (components/camel-component-api/src/lib.rs:60, currently `pub use camel_endpoint::{UriComponents, UriConfig, parse_uri}`) to include `raw_query_pairs` — camel-http has no direct camel-endpoint dependency and must not gain one (`lint-component-deps`).
5. Update the 2 exhaustive `UriComponents {` literals outside uri.rs: `crates/camel-endpoint/src/config.rs:208` and `crates/camel-endpoint/tests/endpoint_macros_derive_integration_test.rs:365` (add `raw_query: None` or the authored bytes as appropriate to each test). camel-http's literals (lib.rs:3157/:5021) are fixed by Task 2, which owns that file.

**Tests** (in `mod tests`, uri.rs:232+):
- `raw_query_preserves_authored_bytes`
  - setup: URI `scheme://host/p?a=1&b=x%2Cy&c=t:1`
  - action: `parse_uri` on it
  - assert: `raw_query == Some("a=1&b=x%2Cy&c=t:1")` byte-for-byte; `params["b"] == "x,y"` (decoded structured view unchanged)
  - command: `cargo test -p camel-endpoint raw_query_preserves_authored_bytes`
  - expected: fails before step 2, passes after
- `raw_query_absent_is_none`
  - setup: URI `scheme://host/p`
  - action: `parse_uri` on it
  - assert: `raw_query.is_none()`
  - command: `cargo test -p camel-endpoint raw_query_absent_is_none`
  - expected: fails before, passes after
- `raw_query_empty_marker_is_empty_string`
  - setup: URI `scheme://host/p?`
  - action: `parse_uri` on it
  - assert: `raw_query == Some("")`
  - command: `cargo test -p camel-endpoint raw_query_empty_marker_is_empty_string`
  - expected: fails before, passes after
- `raw_query_preserves_raw_wrapper_text`
  - setup: URI `scheme://host/p?token=RAW(abc123)`
  - action: `parse_uri` on it
  - assert: `raw_query == Some("token=RAW(abc123)")` — capture never unwraps
  - command: `cargo test -p camel-endpoint raw_query_preserves_raw_wrapper_text`
  - expected: fails before, passes after
- `raw_query_pairs_decodes_keys_keeps_raw_spans`
  - setup: query string `a=1&connect%54imeout=5s&b=x%2Cy`
  - action: `raw_query_pairs` on it
  - assert: Ok; decoded keys `["a", "connectTimeout", "b"]`; raw spans are the original byte slices `["a=1", "connect%54imeout=5s", "b=x%2Cy"]`
  - command: `cargo test -p camel-endpoint raw_query_pairs_decodes_keys_keeps_raw_spans`
  - expected: fails before step 3, passes after
- `raw_query_pairs_malformed_key_escape_errors`
  - setup: query string `%zz=1` (malformed percent-escape IN the key)
  - action: `raw_query_pairs` on it
  - assert: `Err(CamelError::InvalidUri)` naming the offending key — never a panic
  - command: `cargo test -p camel-endpoint raw_query_pairs_malformed_key_escape_errors`
  - expected: fails before, passes after
- `raw_query_pairs_malformed_value_escape_stays_raw`
  - setup: query string `a=%zz` (malformed escape in the VALUE)
  - action: `raw_query_pairs` on it
  - assert: `Ok(vec![("a".to_string(), "a=%zz")])` — values are never decoded, the raw span carries the malformed escape untouched
  - command: `cargo test -p camel-endpoint raw_query_pairs_malformed_value_escape_stays_raw`
  - expected: fails before, passes after
- `duplicate_keys_still_rejected` (regression pin)
  - setup: URI `scheme://host/p?a=1&a=2`
  - action: `parse_uri` on it
  - assert: `Err(CamelError::InvalidUri)` naming `duplicate query parameter: a` — unchanged behavior
  - command: `cargo test -p camel-endpoint duplicate_keys`
  - expected: passes before AND after (pin)

**Acceptance:**
- `cargo test -p camel-endpoint` exits 0 (lib + integration test literal fixes compile)
- `cargo clippy -p camel-endpoint -- -D warnings` exits 0
- `cargo fmt --check -p camel-endpoint` exits 0
- `grep -n "raw_query_pairs" crates/components/camel-component-api/src/lib.rs` shows the extended re-export
- `grep -rn "camel-endpoint" crates/components/camel-http/Cargo.toml` shows no new direct dependency

- [x] task-1

## camel-component-http

### Task 2: promote `connectTimeout` to uri_param + metadata-driven option predicate + raw-preserving `resolve_url` + error propagation

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified, incl. its test literals at :3157/:5021)

**Steps:**
1. Promote `connectTimeout` to uri_param metadata — METADATA-ONLY. Today `?connectTimeout=5000` is stripped by `HTTP_CAMEL_OPTIONS` (lib.rs:162) and silently DROPPED: `HttpEndpointConfig` (lib.rs:111-153) has no connect-timeout field and no parsing exists anywhere (the `connect_timeout_ms` consumed at lib.rs:2085 belongs to the GLOBAL `HttpConfig` in camel-http/src/config.rs:8 — do NOT touch it and do NOT wire the endpoint option into `build_client`). The promotion adds `#[uri_param(name = "connectTimeout", desc = ...)]` on the private metadata-only mirror `HttpEndpointUriConfig` (lib.rs:377, never constructed — its own doc comment says metadata-only), so `uri_options()` covers the key and the raw filter consumes it exactly as the strip list did. Update the parity pin `endpoint_uri_options_count_parity` (lib.rs:8998) from 20 to 21. If `cargo xtask schema --check` flags drift, regenerate and add the generated schema artifact path to this task's Files list. `cookieHandling` gets NO promotion: `from_uri` rejects it loudly (lib.rs:267-269) so it never reaches the serializer — leave that path untouched.
2. Add `raw_query: Option<String>` to `HttpEndpointConfig`, populated in the constructor path that builds from `UriComponents` (the `from_uri`/`from_components` region, lib.rs:315-330): copy `components.raw_query` through.
3. DELETE the leftover-collection block at lib.rs:326-330 (`Collect remaining params (not Camel options) as query params`) — `query_params` is NO LONGER auto-populated from URI leftovers; `raw_query` is the SOLE carrier of authored pairs. Re-type `query_params` from `HashMap<String, String>` to `Vec<(String, String)>` so declaration order is real. Writers to migrate: the deleted `from_uri` block, the test at lib.rs:7311-7312 (`query_params.insert(...)` → `Vec::push((k, v))` preserving declaration order), and the two `from_uri` tests at lib.rs:4741-4749 (update to assert `raw_query` carries the leftovers instead). `query_params` is now programmatic-only (set via config construction by callers).
4. Replace the handwritten `HTTP_CAMEL_OPTIONS` filter (const lib.rs:156-179) with ONE predicate `fn is_consumed_option(key: &str) -> bool` derived from `HttpEndpointConfig::uri_options()` (lib.rs:491-492) — decoded-key match against the `UriOption` metadata. Delete the `HTTP_CAMEL_OPTIONS` const. The predicate's single RUNTIME call site is the raw filter in step 5. `from_uri` is manually implemented (`#[uri_config(skip_impl)]`, lib.rs:181-330) and its typed parsing stays direct and unchanged — the metadata is the sole owner of OUTBOUND option filtering, it does not re-wire from_uri's parsing (spec-amended wording; no structured filter call site remains — the lib.rs:329 filter dies with the leftover-collection block deleted in step 3).
5. Rework `resolve_url` (lib.rs:2434-2508) to `fn resolve_url(exchange: &Exchange, config: &HttpEndpointConfig) -> Result<String, CamelError>`:
   - **Raw path** (when `config.raw_query` is `Some`): rebuild the query from `raw_query_pairs(&raw)` (re-exported via camel_component_api) — keep each pair's raw span byte-for-byte unless `is_consumed_option(decoded_key)`. Preserve order, separators, and authored escapes (incl. `RAW(...)` text). Emit the query component iff at least one span was kept OR `raw_query == Some("")` (bare marker always preserves `?`; a non-empty query whose every pair was consumed drops the component entirely).
   - **Programmatic merge**: after the raw pairs, append `query_params` entries whose key is ABSENT from the authored raw pairs, in Vec declaration order, minimal RFC-3986 encoding, `%20` — never `+` — for spaces. Authored keys always win (no duplication, no override). With no `raw_query`, `query_params` serializes alone, same encoding rules.
   - **Forbidden bytes**: if any kept raw span contains a byte not legal in a query component per RFC 3986 (e.g. literal space, `#`, non-ASCII), return `Err` naming the byte — never re-encode. This check fires BEFORE the resolved string reaches any consumer (SSRF pre-check at lib.rs:2607, `redact_url_for_diagnostics` at lib.rs:2520).
   - **Precedence** (unchanged semantics, made explicit): `bridgeEndpoint` bridging > `CamelHttpUri` override > `CamelHttpQuery` header applied verbatim (lib.rs:2468-2475/2492-2498 behavior stays) > raw/endpoint base + programmatic merge. The `bridgeEndpoint` and `CamelHttpUri` arms themselves (lib.rs:2438-2475) are UNCHANGED — `raw_query` must not leak into the bridge/override arms.
   - Replace both `.expect(...)` at lib.rs:2445 and lib.rs:2500 with error propagation returning a `CamelError` naming the malformed URL.
6. Update every `resolve_url` call site to handle the `Result` — the PRIMARY producer path is the SSRF pre-check at lib.rs:2607 (`let url = HttpProducer::resolve_url(...)` becomes `?`-propagated through the async producer body; verify `validate_url_for_ssrf`/`resolve_initial_url_for_ssrf` still receive a `&str` and the Result threads compile through the async body) plus any test resolvers. No panic path remains.
7. Fix camel-http's own exhaustive `UriComponents {` literals at lib.rs:3157 and lib.rs:5021 (test module) to include `raw_query`.
8. Update pinned tests asserting the OLD form-encoded output (the `resolve_url_*` tests at lib.rs:7308+ and the producer URI-override pin near lib.rs:3794) to assert authored bytes instead. The bridge tests keep asserting pre-raw behavior for the bridge/override arms.

**Tests** (in `mod tests`, lib.rs:3051+):
- `resolve_url_preserves_authored_query_order_and_bytes`
  - setup: endpoint URI `http://h/p?a=1&b=x,y&c=t:1&connectTimeout=5000` parsed via `from_uri` (connectTimeout now a uri_param, consumed)
  - action: `resolve_url(&exchange, &config)`
  - assert: Ok; query == `a=1&b=x,y&c=t:1` — no `%2C`/`%3A`, authored order
  - command: `cargo test -p camel-component-http resolve_url_preserves_authored_query_order_and_bytes`
  - expected: fails before step 4, passes after
- `resolve_url_consumes_encoded_option_key`
  - setup: endpoint URI `http://h/p?connect%54imeout=5000&a=1` via `from_uri`
  - action: `resolve_url`
  - assert: Ok; query == `a=1` (decoded-key match consumed the encoded pair)
  - command: `cargo test -p camel-component-http resolve_url_consumes_encoded_option_key`
  - expected: fails before, passes after
- `resolve_url_all_options_consumed_drops_query`
  - setup: endpoint URI `http://h/p?connectTimeout=5000` via `from_uri`
  - action: `resolve_url`
  - assert: Ok; URL ends at the path, no `?`
  - command: `cargo test -p camel-component-http resolve_url_all_options_consumed_drops_query`
  - expected: fails before, passes after
- `resolve_url_preserves_empty_query_marker`
  - setup: config whose `raw_query == Some("")` (endpoint URI `http://h/p?`)
  - action: `resolve_url`
  - assert: Ok; URL keeps a trailing `?`
  - command: `cargo test -p camel-component-http resolve_url_preserves_empty_query_marker`
  - expected: fails before, passes after
- `resolve_url_raw_wrapper_not_re_encoded`
  - setup: config whose `raw_query == Some("token=RAW(abc)")`
  - action: `resolve_url`
  - assert: Ok; query == `token=RAW(abc)` — never `RAW%28abc%29` (rc-g4isv pin)
  - command: `cargo test -p camel-component-http resolve_url_raw_wrapper_not_re_encoded`
  - expected: fails before, passes after
- `resolve_url_camel_http_query_stays_verbatim`
  - setup: exchange header `CamelHttpQuery: userFilter=a%2Cb`, endpoint raw_query `Some("x=1")`
  - action: `resolve_url`
  - assert: header value applied verbatim and wins over the raw base (no structured re-serialization)
  - command: `cargo test -p camel-component-http resolve_url_camel_http_query_stays_verbatim`
  - expected: passes before AND after if verbatim behavior is preserved (pin)
- `resolve_url_programmatic_params_use_percent20_deterministic`
  - setup: config `query_params` vec `[("b", "x y"), ("a", "1")]` in that declaration order, no raw_query (Vec re-type makes declaration order real)
  - action: `resolve_url`
  - assert: Ok; query == `b=x%20y&a=1` in declaration order, no `+`
  - command: `cargo test -p camel-component-http resolve_url_programmatic_params_use_percent20_deterministic`
  - expected: fails before (HashMap order + `+`), passes after
- `resolve_url_authored_and_programmatic_merge`
  - setup: config raw_query `Some("a=1&c=t:1")` and `query_params` vec `[("b", "2"), ("a", "9")]`
  - action: `resolve_url`
  - assert: Ok; query == `a=1&c=t:1&b=2` — programmatic `b` appended (absent from raw), programmatic `a=9` IGNORED (authored key wins), no duplication
  - command: `cargo test -p camel-component-http resolve_url_authored_and_programmatic_merge`
  - expected: fails before, passes after
- `from_uri_no_longer_fills_query_params_from_uri` (provenance pin)
  - setup: URI `http://h/p?a=1&connectTimeout=5000`
  - action: `HttpEndpointConfig::from_uri`
  - assert: `query_params` is EMPTY (no auto-population) and `raw_query == Some("a=1&connectTimeout=5000")` (leftovers live in raw only)
  - command: `cargo test -p camel-component-http from_uri_no_longer_fills_query_params`
  - expected: fails before, passes after
- `resolve_url_forbidden_raw_byte_errors`
  - setup: config whose `raw_query == Some("a=x y")` (literal space)
  - action: `resolve_url`
  - assert: Err naming the forbidden byte; no output string produced
  - command: `cargo test -p camel-component-http resolve_url_forbidden_raw_byte_errors`
  - expected: fails before, passes after
- `resolve_url_malformed_base_url_errors_no_panic`
  - setup: producer instance; a config whose base URL `url::Url::parse` rejects (e.g. `http://[::1:bad`)
  - action: drive the call THROUGH `HttpProducer::call` (the real producer path, not the private resolver), then drive a valid request through the SAME producer
  - assert: first call returns Err (propagated, no panic, producer task alive); the SUBSEQUENT valid call on the same producer succeeds — no poisoned state (rc-ph7z2 pin)
  - command: `cargo test -p camel-component-http resolve_url_malformed_base_url_errors_no_panic`
  - expected: fails before (panic), passes after
- `endpoint_uri_options_count_parity` (updated)
  - setup: promoted connectTimeout uri_param
  - action: run the existing pin
  - assert: counts match at 21 (updated constant)
  - command: `cargo test -p camel-component-http endpoint_uri_options_count_parity`
  - expected: passes after step 1

**Acceptance:**
- `cargo test -p camel-component-http` exits 0
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0
- `grep -c "HTTP_CAMEL_OPTIONS" crates/components/camel-http/src/lib.rs` outputs 0
- Neither `expect("base URL must be valid")` site remains (both replaced by error propagation)
- `cargo xtask schema --check` exits 0 (covers the `connectTimeout` promotion AND the `query_params` Vec re-type derives)
- `cargo xtask lint-unwrap` exits 0
- SSRF pre-check (lib.rs:2607) compiles against the Result signature and its tests still pass — `url::Url::parse` of raw-preserving query bytes does not reinterpret the host
- `redact_url_for_diagnostics` (lib.rs:2520) tests still pass with raw-query bytes

**Dependencies:** Task 2 depends on Task 1 (`raw_query` field + `raw_query_pairs` re-export via camel_component_api).

- [x] task-2

## camel-integration-test

### Task 3: apparatus error for empty harness path + wire-path diagnostics with positive secret redaction

**Files:**
- `crates/camel-integration-test/src/adapters/http.rs` (modified)
- `crates/camel-integration-test/src/adapters.rs` (modified)
- `crates/camel-integration-test/src/runner.rs` (modified)
- `crates/camel-cli/src/commands/test/scenario.rs` (modified — ~5 lines, post-boot injection; see step 4. Verified chain: HttpPartner construction happens PRE-boot at scenario.rs:185-199 where no ctx exists; the CamelContext is born at scenario.rs:414 from `run.ctx`; partner injection therefore happens post-boot, pre-run, alongside the router-completion loop at :416-435. Change A merges FIRST (human-approved), so no wave-B conflict.)

**Steps:**
1. `ParsedTarget::parse` (http.rs:551-590): replace the silent `/` fallback at http.rs:587 with an apparatus-class error — an empty or absent path in a harness target declaration fails parsing with a message naming the declaration. Do the same for the serve-path fallback at http.rs:345. LEAVE http.rs:758 untouched: that site defaults the recorded path for authority-form requests (a wire-recording concern, not a declaration parse) and is out of spec scope.
2. Receive-timeout diagnostics: the receive-timeout error is constructed in adapters/http.rs:354 and :536 (type/message template at runner.rs:202-204, mapped at runner.rs:479). Extend the construction sites that can see the lane registry to append the list of wire `path_and_query` strings that arrived in the matching lane group (`lane_key_for`, adapters.rs:268). Message shape: `receive-timeout: no arrival matched; lanes recorded: [/api?x=1, /api?x=2]`.
3. Count-mismatch detail: extend `partner_mismatch_detail` (runner.rs:739) to append the recorded paths — `, recorded: [/a?b=1, /c]` — instead of counts only.
4. Redaction — POSITIVE secret rule (design layer 3), ONE entry-point mechanism (e_gpt-prescribed, conductor-verified): before any wire path reaches a diagnostic string, mask (as `***`) the value of any key that matches a secret-marked `UriOption` (`.secret`, camel-api component_metadata) in scheme `http`'s `ComponentMetadata`.
   - **Single entry point**: `PartnerRouter::set_secret_query_keys(&self, keys: Vec<String>)` (adapters.rs:214-232). It (a) stores the set on the router itself as `Arc<RwLock<Vec<String>>>` — readable later by the rendering path for `partner_mismatch_detail` (runner.rs partner_validate_action holds `&PartnerRouter`), (b) sets it on the router-owned `client_lane: ClientLane` (the `await_parked` timeout site, http.rs:528-546), and (c) calls `set_secret_query_keys(&self, keys: &[String])` — a NEW default-noop method on the `PartnerAdapter` trait — on every boxed adapter; the HTTP adapter's override stores the set into its `HttpPartner` inner server (the arrivals-lane timeout site, http.rs:354 region). Both timeout sites read the stored set when rendering the recorded-paths portion of the message. Keys NOT marked secret stay visible, including unknown keys (apparatus-internal diagnostics).
   - **CLI side** (~4 lines, scenario.rs): post-boot pre-run, after `PartnerRouter::new(adapters)` at :435 and before `run_scenario_document` at :446: read `ctx.lock().await.component_metadata("http")` (context.rs:830) → collect entries of the `.uri_options` PUBLIC FIELD (camel-api component_metadata.rs:189 — it is a field, not a method) with `secret == true` → ONE call `router.set_secret_query_keys(keys)`. (HttpPartner instances are already moved into `Box<dyn PartnerAdapter>` by then — the trait method is how the box is reached. Verified chain: construction PRE-boot scenario.rs:185-199; ctx born :414; router built :435; run :446. Change A merges FIRST, human-approved — no wave-B conflict.)
   - **Key matching decodes**: the shared diagnostic redactor compares DECODED keys — it walks the recorded path's query with `raw_query_pairs` (available to itest via the camel-component-api re-export from Task 1) and masks a pair when its decoded key matches the secret set, preserving every other pair's raw span. A percent-encoded secret key (`%61uthPassword=hunter2` → decoded `authPassword`) MUST match and mask. If `raw_query_pairs` returns Err (malformed key escape), the redactor masks the ENTIRE query portion fail-safe — never panics, never prints undecoded secrets.
   - **Unit tests** (itest): construct `ComponentMetadata` directly with a secret-marked option, derive the key set, call `router.set_secret_query_keys` on a router holding a started HTTP partner — no context boot needed. Do NOT use `EndpointUri::to_redacted_string` (fail-safe blanket masking hides unknown keys) and do NOT add new crate dependencies (camel-integration-test already depends on camel-api, camel-core, and camel-component-api; the set crosses the CLI→itest boundary as plain `Vec<String>`).
5. Lane key stays STRICT wire bytes — do NOT canonicalize `lane_key_for` (adapters.rs:268-271); only diagnostics change.

**Tests** (in `mod tests`, http.rs:903+, with `--features http`; runner tests where `partner_mismatch_detail` lives):
- `parsed_target_empty_path_is_apparatus_error`
  - setup: target declaration string `http://host` (no path)
  - action: parse the target
  - assert: apparatus-class error naming the declaration; no `/` lane created
  - command: `cargo test -p camel-integration-test --features http parsed_target_empty_path_is_apparatus_error`
  - expected: fails before step 1, passes after
- `partner_mismatch_detail_lists_recorded_paths`
  - setup: expected count 2, actual 1, recorded paths `["/a?b=1", "/c"]`
  - action: `partner_mismatch_detail`
  - assert: message contains both paths and `expected 2, actual 1`
  - command: `cargo test -p camel-integration-test --features http partner_mismatch_detail_lists_recorded_paths`
  - expected: fails before step 3, passes after
- `receive_timeout_message_lists_arrived_wire_paths`
  - setup: lane group with arrivals `[/api?x=1]` (`x` is not a secret-marked option), receive matching a different path
  - action: run the receive with an already-expired timeout budget
  - assert: timeout error contains `/api?x=1` in clear
  - command: `cargo test -p camel-integration-test --features http receive_timeout_message_lists_arrived_wire_paths`
  - expected: fails before step 2, passes after
- `diagnostics_redact_sensitive_query_values` — THREE named sinks, one test each, all with router.set_secret_query_keys(vec!["authPassword"]) (unit test derives the set from a directly-constructed `ComponentMetadata` with a secret-marked option) and recorded paths `/login?authPassword=hunter2&x=1`:
  - `arrivals_lane_timeout_redacts_secrets` — drives an arrivals-lane receive into timeout (http.rs:354 region); asserts the timeout message shows `authPassword=***`, does NOT contain `hunter2`, and DOES show `x=1` in clear
  - `await_parked_timeout_redacts_secrets` — drives a `ClientLane::await_parked` call into timeout (http.rs:528-546); same three assertions
  - `count_mismatch_redacts_secrets` — drives a partner count mismatch through `partner_mismatch_detail` (runner.rs:739); same three assertions
  - command: `cargo test -p camel-integration-test --features http redacts_secrets`
  - expected: all three fail before step 4, pass after
- `encoded_secret_query_key_redacts`
  - setup: router with set_secret_query_keys(vec!["authPassword"]); recorded path `/login?%61uthPassword=hunter2&x=1` (percent-encoded secret key)
  - action: render through the shared diagnostic redactor
  - assert: message shows `%61uthPassword=***&x=1`; `hunter2` absent; unknown key `x=1` visible
  - command: `cargo test -p camel-integration-test --features http encoded_secret_query_key_redacts`
  - expected: fails before the decode-match rule, passes after
- `query_bearing_receive_matches_end_to_end` (the scenario e2e)
  - setup: harness HTTP partner declared `from: http://127.0.0.1:PORT/api?flag=a&x=1`, a send action targeting the same authored query bytes
  - action: run the scenario document through the runner
  - assert: the request lands in the matching lane and the receive asserts against it (arrival key equals declared wire `path_and_query`)
  - command: `cargo test -p camel-integration-test --features http query_bearing_receive_matches_end_to_end`
  - expected: fails before Task 2's serializer lands (lane-key mismatch), passes after both

**Acceptance:**
- `cargo test -p camel-integration-test --features http` exits 0
- `cargo clippy -p camel-integration-test --features http -- -D warnings` exits 0
- `grep -n '"/".to_string()' crates/camel-integration-test/src/adapters/http.rs` outputs exactly one line — the documented authority-form recording site near :758 (the ParsedTarget :587 and serve :345 fallbacks are gone)

**Dependencies:** Task 3 depends on Task 2 (the `query_bearing_receive_matches_end_to_end` e2e test requires the raw-preserving serializer).

- [x] task-3

## docs

### Task 4: CONTEXT.md alignment for the three touched crates

**Files:**
- `crates/camel-endpoint/CONTEXT.md` (modified)
- `crates/components/camel-http/CONTEXT.md` (modified)
- `crates/camel-integration-test/CONTEXT.md` (modified)

**Steps:**
1. camel-endpoint CONTEXT.md: document the `raw_query` contract (structured view strict / raw view verbatim; `Some("")` vs `None`) and the exported `raw_query_pairs` helper (incl. the camel-component-api re-export) in the crate's contract-surface section.
2. camel-http CONTEXT.md: document the raw-preserving outbound query semantics (precedence chain bridgeEndpoint > CamelHttpUri > CamelHttpQuery verbatim > raw_query base > programmatic `query_params`; forbidden-byte error; metadata-driven option consumption incl. the `connectTimeout` promotion) in the producer section.
3. camel-integration-test CONTEXT.md: document the strict wire lane key stance, the new mismatch/timeout diagnostics with the positive secret-redaction rule, and the empty-path apparatus error.
4. All prose in English per language policy; keep entries in the existing CONTEXT.md style (no new sections where the file has none).

**Tests:**
- `name`: docs entries present and accurate
  - setup: the three CONTEXT.md files as modified
  - action: `cargo xtask lint-context-citations`
  - assert: exits 0
  - command: `cargo xtask lint-context-citations`
  - expected: pass after implementation

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0
- Each of the three CONTEXT.md files mentions the new contract by the exact symbol names introduced in tasks 1-3 (`raw_query`, `raw_query_pairs`, `is_consumed_option`, the diagnostics)

**Dependencies:** Task 4 depends on Tasks 1-3 (documents all their symbols).

- [x] task-4
