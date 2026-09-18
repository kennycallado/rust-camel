# Tasks: cleartextpolicy

Uniform public-cleartext egress rule per `specs/egress-cleartext-policy/spec.md`: cleartext `http://` to a PUBLIC target is rejected unless `allow_cleartext` is set — independent of `allow_internal` — on all four paths (camel-http literal, camel-http hostname+redirect, MCP literal at config load, MCP hostname at connect). Blessed spec hash: sha256:32edfba5814c1510da3ee75318028b86326be77be35ab94a563fb0eabbd3b2b3.

All cargo commands run with `RUSTC_WRAPPER=` (empty). TDD: write the named tests first, verify they fail against current code, then implement until green.

## Task 1.1 — MCP literal path: `allow_cleartext` field + rule flip in config.rs

**Files:**
- `crates/components/camel-component-mcp/src/config.rs` (modified)
- `crates/components/camel-component-mcp/tests/client_producer_test.rs` (modified — struct literals only)
- `crates/components/camel-component-mcp/tests/server_tool_dispatch_test.rs` (modified — struct literal only)
- `crates/components/camel-component-mcp/tests/server_resource_dispatch_test.rs` (modified — struct literal only)
- `crates/components/camel-component-mcp/tests/server_protocol_test.rs` (modified — struct literal only)
- `crates/components/camel-component-mcp/tests/dsl_e2e_test.rs` (modified — struct literal only)

**Steps:**
1. Add field `pub allow_cleartext: bool` to `McpRemoteConfig` directly after `allow_internal`, with `#[serde(default)]`, and a doc comment: default `false`; when `true`, permits cleartext `http://` to a PUBLIC target for this remote (the only escape hatch for the uniform rule, ADR-0081); independent of `allow_internal`.
2. Rewrite the `allow_internal` field doc comment to the new semantics: internal-target reachability only; no longer mentions permitting or rejecting public cleartext (that is `allow_cleartext`'s job).
3. Flip the literal-branch conjunct in `validate_url` (currently `if !blocked && lower.starts_with("http://") && self.allow_internal`) to fire when `!blocked && lower.starts_with("http://") && !self.allow_cleartext` — no `allow_internal` conjunct. Update the comment above it to cite ADR-0081.
4. Update the rejection message to: `"remote '{name}' uses cleartext http:// to a public address; set allow_cleartext=true or use https://"`. The message must not echo the URL (it already only prints `name` — keep it that way).
5. Update the `remote` test helper to take a third `allow_cleartext: bool` argument — keep all existing call sites compiling by passing `false`.
6. The six `McpRemoteConfig` struct literals in the five integration-test files listed above break when the field is added — update each with `allow_cleartext: false` (no behavior change intended there).

**Tests (write first, expect FAIL on the flipped cells):**
- `remote_url_rejects_public_cleartext_literal_by_default`
  - setup: `McpRemoteConfig` url `http://93.184.216.34/mcp`, `allow_internal=false`, `allow_cleartext=false`
  - action: `validate_url("r")`
  - assert: `Err`, message contains `cleartext` and `allow_cleartext`
  - command: `RUSTC_WRAPPER= cargo test -p camel-component-mcp --lib remote_url_rejects_public_cleartext_literal_by_default`
  - expected: fails before step 3 (today it passes validation)
- `remote_url_rejects_public_cleartext_literal_when_allow_internal` (existing — keep, extend)
  - setup: same URL, `allow_internal=true`, `allow_cleartext=false`
  - action: `validate_url("r")`
  - assert: still `Err` with `cleartext` (ADR-0079 cell unchanged)
  - command: `RUSTC_WRAPPER= cargo test -p camel-component-mcp --lib remote_url_rejects_public_cleartext_literal_when_allow_internal`
  - expected: passes before AND after (regression pin)
- `remote_url_permits_public_cleartext_literal_with_allow_cleartext`
  - setup: url `http://93.184.216.34/mcp`, `allow_cleartext=true`, BOTH `allow_internal=false` and (second assert) `allow_internal=true`
  - action: `validate_url("r")` for both
  - assert: `Ok(())` both times
  - command: `RUSTC_WRAPPER= cargo test -p camel-component-mcp --lib remote_url_permits_public_cleartext_literal_with_allow_cleartext`
  - expected: fails before implementation
- `remote_url_cleartext_rejection_message_masks_userinfo`
  - setup: url `http://user:pass@93.184.216.34/mcp`, `allow_internal=false`, `allow_cleartext=false`
  - action: `validate_url("r")` → `Err`
  - assert: message contains `cleartext`; message does NOT contain `user` or `pass` (ADR-0051)
  - command: `RUSTC_WRAPPER= cargo test -p camel-component-mcp --lib remote_url_cleartext_rejection_message_masks_userinfo`
  - expected: fails before implementation (today this URL passes validation entirely)
- `remote_url_absent_allow_cleartext_defaults_false`
  - setup: JSON `{"url": "https://mcp.example.com/mcp", "transport": "streamable-http"}` (no new field)
  - action: `serde_json::from_str::<McpRemoteConfig>`
  - assert: deserializes, `allow_cleartext == false`; and `{"url": "https://mcp.example.com/mcp", "transport": "streamable-http", "allow_cleartext": true}` gives `true`
  - command: `RUSTC_WRAPPER= cargo test -p camel-component-mcp --lib remote_url_absent_allow_cleartext_defaults_false`
  - expected: fails before the field exists (compile error counts as the failing state)
- Rename the now-inverted pin `remote_url_permits_cleartext_public_ip_by_default` → delete it (its cell is now covered by `remote_url_rejects_public_cleartext_literal_by_default`); keep its https assertion by adding `https://93.184.216.34/mcp` + `allow_internal=false` → `Ok` inside `remote_url_absent_allow_cleartext_defaults_false` or the default-rejection test.

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-component-mcp --lib` exits 0
- `RUSTC_WRAPPER= cargo test -p camel-component-mcp --no-run` exits 0 (integration tests compile with the new field)
- `rg -n "allow_internal && http|http.*&& self.allow_internal" crates/components/camel-component-mcp/src/config.rs` finds no cleartext conjunct gated on `allow_internal`
- Existing tests `remote_url_blocks_private_ip_literals_by_default`, `remote_url_hostname_cleartext_passes_config_layer`, `remote_config_unknown_field_rejected` stay green (deny_unknown_fields intact)

- [x] 1.1

## Task 1.2 — MCP hostname path: rule flip + flag threading in dns_pin.rs and client.rs

**Files:**
- `crates/components/camel-component-mcp/src/adapter/dns_pin.rs` (modified)
- `crates/components/camel-component-mcp/src/adapter/client.rs` (modified)

**Steps:**
1. Change `validate_resolution` signature to `validate_resolution(addrs: &[SocketAddr], scheme: &str, allow_internal: bool, allow_cleartext: bool)`.
2. Flip the cleartext conjunct (currently `if allow_internal && scheme == "http" && let Some(public)`) to `if scheme == "http" && !allow_cleartext && let Some(public)` — drop the `allow_internal` gate. Keep the message shape: `remote host resolves to public address {} — not allowed over cleartext http:// (set allow_cleartext=true or use https://)`.
3. Change `build_pinned_http_client` signature to take `allow_cleartext: bool` after `allow_internal`; thread it into `validate_resolution`.
4. Update the module doc bullet list (lines ~14-19): the bullet "`allow_internal = true` + cleartext `http://`: any resolved public IP rejects" becomes "cleartext `http://` with any resolved public IP rejects unless `allow_cleartext=true` (uniform rule, ADR-0081; independent of `allow_internal`)".
5. Update the single call site `adapter/client.rs:59` to pass `config.allow_cleartext`.

**Tests (write first; existing `validate_resolution` tests updated in the same commit):**
- `resolution_public_over_http_rejected_by_default`
  - setup: addrs `[93.184.216.34:80]`, `allow_internal=false`, `allow_cleartext=false`
  - action: `validate_resolution(&addrs, "http", false, false)`
  - assert: `Err` containing `cleartext`
  - command: `RUSTC_WRAPPER= cargo test -p camel-component-mcp --lib resolution_public_over_http_rejected_by_default`
  - expected: fails before step 2 (new 4-arg signature makes old 3-arg tests fail to compile — update them to pass `false` for `allow_cleartext` in the same edit, then this test fails on the assertion)
- `resolution_public_over_http_rejected_when_allow_internal` (existing — keep semantics)
  - setup: addrs `[93.184.216.34:80]`
  - action: `validate_resolution(&addrs, "http", true, false)`
  - assert: `Err` containing `cleartext` (ADR-0079 cell unchanged)
  - command: `RUSTC_WRAPPER= cargo test -p camel-component-mcp --lib resolution_public_over_http_rejected_when_allow_internal`
  - expected: passes after signature update (regression pin)
- `resolution_public_over_http_allowed_with_allow_cleartext`
  - setup: addrs `[93.184.216.34:80]`
  - action: `validate_resolution(&addrs, "http", false, true)` and `validate_resolution(&addrs, "http", true, true)`
  - assert: both `Ok(())`
  - command: `RUSTC_WRAPPER= cargo test -p camel-component-mcp --lib resolution_public_over_http_allowed_with_allow_cleartext`
  - expected: fails before step 2
- `resolution_https_public_ok_by_default_both_flags` (regression)
  - setup: addrs `[93.184.216.34:443]`
  - action: `validate_resolution(&addrs, "https", false, false)`
  - assert: `Ok(())`
  - command: `RUSTC_WRAPPER= cargo test -p camel-component-mcp --lib resolution_https_public_ok_by_default_both_flags`
  - expected: passes (rename/extend of existing `resolution_all_public_ok_by_default`)
- `resolution_internal_http_ok_when_allow_internal` (regression — internal cleartext unchanged)
  - setup: addrs `[10.0.0.5:80, 127.0.0.1:80]`
  - action: `validate_resolution(&addrs, "http", true, false)`
  - assert: `Ok(())` (cleartext to internal targets is not governed by the new rule; only the https variant exists today)
- Existing tests `resolution_with_blocked_ip_rejected_by_default`, `resolution_blocked_ok_when_allow_internal_https`, `empty_resolution_rejected`, `ip_literal_url_builds_client_without_dns` updated for the new signature (`build_pinned_http_client` gains a 3rd argument) and stay green.

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-component-mcp --lib` exits 0
- `rg -n "allow_internal\s*&&\s*scheme" crates/components/camel-component-mcp/src/adapter/dns_pin.rs` returns nothing
- `rg -n "build_pinned_http_client\(" crates/components/camel-component-mcp/src` shows exactly one non-test call site, passing both flags

- [x] 1.2

## Task 1.3 — camel-http: `allowCleartext` endpoint option + rule flips in lib.rs and ssrf.rs

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)
- `crates/components/camel-http/src/ssrf.rs` (modified)

**Steps:**
1. In `lib.rs`, add `pub allow_cleartext: bool` to `HttpEndpointConfig` next to `allow_internal` (line ~126): doc comment (public-cleartext transport consent, default false, ADR-0081, independent of `allow_internal`), add to the manual `Debug` impl (line ~179) after `allow_internal`, parse the `allowCleartext` URI option in `from_uri` exactly like `allowInternal` (line ~280: bool parse with `CamelError::InvalidUri` naming `allowCleartext` on bad value, default `false` when absent), thread it into the struct construction (line ~398).
2. Add the metadata entry next to the `allowInternal` `#[uri_param]` (line ~471): `name = "allowCleartext"`, `default = "false"`, `desc = "Allow cleartext http:// to public targets (transport consent; ADR-0081)"`.
3. Check the `contains_key("allowInternal")` site at line ~616: it is a global-`HttpConfig` fallback for `allow_internal`. Do NOT extend this treatment to `allow_cleartext` — consent is per-endpoint only (a global cleartext lever is explicitly rejected in ADR-0081's escape-hatch comparison). Leave the site untouched; note this decision in the task result.
4. In `ssrf.rs` `validate_url_for_ssrf`: for both the Ipv4 and Ipv6 arms, replace the two rules (`!config.allow_internal && is_blocked → reject` stays; `config.allow_internal && !is_blocked && scheme=="http" → reject` becomes `!is_blocked && scheme=="http" && !config.allow_cleartext → reject`). Message: `Public IP '{}' not allowed over cleartext HTTP (set allowCleartext=true or use HTTPS)`.
5. In `ssrf.rs` `validate_redirect_target_for_ssrf`: add `allow_cleartext: bool` parameter after `allow_internal`; apply the same literal-arm flip; in the `Domain` arm replace `if allow_internal && url.scheme()=="http" && let Some(public)` with `if url.scheme()=="http" && !allow_cleartext && let Some(public)`. Update BOTH cleartext messages in this fn (literal arm "Redirect to public IP 'IP' not allowed over HTTP" and Domain arm "Redirect host 'host' resolves to public IP — not allowed over HTTP") to end with the remedy: `(set allowCleartext=true or use HTTPS)`.
6. In `ssrf.rs` `resolve_initial_url_for_ssrf`: add `allow_cleartext: bool` parameter after `allow_internal`; same hostname-arm flip. Update its cleartext message ("Host 'host' resolves to public IP — not allowed over HTTP") to end with the same remedy: `(set allowCleartext=true or use HTTPS)`.
7. Update all callers in `lib.rs` (producer send path passes `endpoint_config.allow_internal, endpoint_config.allow_cleartext`; `send_with_ssrf_safe_redirects` passes both to `validate_redirect_target_for_ssrf`). VERIFY: the `CamelHttpUri` override path routes override URLs through `validate_url_for_ssrf` (grep the override resolution in `resolve_url`/producer call). If it does NOT route through it, STOP and report `test-design-gap: CamelHttpUri override bypasses validate_url_for_ssrf` — do not invent a new validation site.
8. Confirm `is_sensitive_redirect_header` and the downgrade-stripping logic are untouched by the flag threading (no code change; existing test `test_sensitive_redirect_headers` must stay green unmodified).

**Tests (write first):**
- `test_validate_url_rejects_public_http_by_default`
  - setup: `HttpEndpointConfig::from_uri("http://example.com").unwrap()`, `allow_internal=false`, `allow_cleartext=false`
  - action: `validate_url_for_ssrf("http://1.1.1.1/api", &cfg)`; then with query credential `http://1.1.1.1/api?token=sekrit99`, and userinfo variant `http://user:pass@1.1.1.1/api`
  - assert: all three `Err`; first message contains `allowCleartext` (remedy) and `1.1.1.1`; NO message contains `sekrit99`, `user`, or `pass` (query + userinfo never echoed, ADR-0051)
  - command: `RUSTC_WRAPPER= cargo test -p camel-http --lib test_validate_url_rejects_public_http_by_default`
  - expected: fails before step 4
- `test_validate_url_public_https_literals_ok` (regression)
  - setup: default flags
  - action: `validate_url_for_ssrf("https://1.1.1.1/api", &cfg)`
  - assert: `Ok` (https public literal stays allowed by default)
- `test_validate_url_rejects_public_http_under_allow_internal` (existing — keep, extend)
  - setup: `allow_internal=true`, `allow_cleartext=false`
  - assert: `http://1.1.1.1/api` still `Err` (ADR-0079 cell); `http://127.0.0.1/api` still `Ok` (internal cleartext unchanged)
  - command: `RUSTC_WRAPPER= cargo test -p camel-http --lib test_validate_url_rejects_public_http_under_allow_internal`
  - expected: passes before AND after (update only if message assert needs the new remedy text)
- `test_validate_url_allows_public_http_with_allow_cleartext`
  - setup: `allow_cleartext=true` set directly on config
  - action: `validate_url_for_ssrf("http://1.1.1.1/api", &cfg)` with `allow_internal=false`, and again with `allow_internal=true`
  - assert: both `Ok`
  - command: `RUSTC_WRAPPER= cargo test -p camel-http --lib test_validate_url_allows_public_http_with_allow_cleartext`
  - expected: fails before step 4
- `test_uri_option_allow_cleartext_parses`
  - setup/action: `HttpEndpointConfig::from_uri("http://example.com/?allowCleartext=true")` and plain `from_uri("http://example.com")`
  - assert: first has `allow_cleartext==true`; second `false`; `from_uri("http://example.com/?allowCleartext=banana")` yields `InvalidUri` naming `allowCleartext`
  - command: `RUSTC_WRAPPER= cargo test -p camel-http --lib test_uri_option_allow_cleartext_parses`
  - expected: fails before steps 1-2
- `test_resolve_initial_url_rejects_public_http_by_default`
  - setup: default flags
  - action: `resolve_initial_url_for_ssrf("http://example.com/", false, false).await` AND `resolve_initial_url_for_ssrf("http://example.com/", true, false).await`
  - assert: both `Err` AND each message contains the `allowCleartext` remedy string (the old "use HTTPS"-only message must be gone) (default AND allow_internal=true hostname cells; DNS-dependent: `example.com` resolves public; existing tests already rely on real DNS)
  - command: `RUSTC_WRAPPER= cargo test -p camel-http --lib test_resolve_initial_url_rejects_public_http_by_default`
  - expected: fails before step 6 (the `(true,false)` half already rejects today — the `(false,false)` half is the flip; both fail on the remedy-string assert before the message step lands)
- `test_resolve_initial_url_public_https_host_returns_addrs`
  - setup/action: flip the existing `test_resolve_initial_url_public_host_returns_addrs` URL to `https://example.com/` (http form now rejects)
  - assert: `Some((host, addrs))`, addrs non-empty (https regression pin)
  - command: `RUSTC_WRAPPER= cargo test -p camel-http --lib test_resolve_initial_url_public_https_host_returns_addrs`
  - expected: passes after rename+flip
- `test_resolve_initial_url_allows_public_http_with_allow_cleartext`
  - action: `resolve_initial_url_for_ssrf("http://example.com/", false, true).await` and `("http://example.com/", true, true).await`
  - assert: both return `Ok` with `Some` pinning output (hostname + addrs non-empty)
  - command: `RUSTC_WRAPPER= cargo test -p camel-http --lib test_resolve_initial_url_allows_public_http_with_allow_cleartext`
  - expected: fails before step 6
- `test_validate_redirect_target_rejects_public_http_by_default`
  - setup: `url::Url::parse("http://1.1.1.1:80/x")`
  - action: `validate_redirect_target_for_ssrf(&url, false, false).await` and `(&url, true, false).await`
  - assert: both `Err` AND each message contains the `allowCleartext` remedy string; and with `(false, true)` → `Ok` with one addr `1.1.1.1:80` (literal escape + allow_internal cell pin)
  - command: `RUSTC_WRAPPER= cargo test -p camel-http --lib test_validate_redirect_target_rejects_public_http_by_default`
  - expected: fails before step 5
- `test_validate_redirect_target_public_domain_http_rule`
  - setup: `url::Url::parse("http://example.com/x")` (real DNS, public)
  - action: `validate_redirect_target_for_ssrf(&url, false, false).await`, `(&url, true, false).await`, `(&url, false, true).await`
  - assert: first two `Err` with the `allowCleartext` remedy string; third `Ok` (the redirect Domain arm has its own copy of the conjunct and its own message — this pins both on all three cells)
  - command: `RUSTC_WRAPPER= cargo test -p camel-http --lib test_validate_redirect_target_public_domain_http_rule`
  - expected: fails before step 5
- `test_validate_redirect_target_public_https_ok` (regression)
  - setup: `url::Url::parse("https://1.1.1.1:443/x")`, default flags
  - action: `validate_redirect_target_for_ssrf(&url, false, false).await`
  - assert: `Ok` with one addr `1.1.1.1:443` (https redirect target stays allowed by default)
- `test_redirect_followed_hop_strips_credentials_unaffected_by_allow_cleartext`
  - setup: entry responder on `http://localhost:{entry_port}` 302-redirecting to `http://127.0.0.1:{hop_port}/hop` (cross-origin: different host); hop responder CAPTURES the request headers it receives (extend `spawn_200_responder` with a header-recording variant writing into a `std::sync::Mutex<Vec<(String,String)>>`); endpoint `from_uri("http://localhost/?allowInternal=true&allowCleartext=true")` (both flags set — the scenario's "downgrade hop is followed with allow_cleartext=true" condition; localhost/127.0.0.1 are internal so the cleartext rule passes on reachability grounds while the flag is exercised); request sends `Authorization`, `Cookie`, `X-API-Key` headers
  - action: call `send_with_ssrf_safe_redirects` with those headers (same argument shape as the existing `redirect_hop_to_allowed_host_followed_by_fence` test); await completion; read captured hop headers
  - assert: response 200; captured hop headers contain NONE of `authorization`, `cookie`, `x-api-key` (the cross-origin strip fires in the real followed loop — `is_sensitive_redirect_header` behavior is not relaxed by `allow_cleartext`; the downgrade-specific `proxy-authorization` arm stays pinned by the unmodified unit test `test_sensitive_redirect_headers`)
  - command: `RUSTC_WRAPPER= cargo test -p camel-http --lib test_redirect_followed_hop_strips_credentials_unaffected_by_allow_cleartext`
  - expected: fails before implementation only if the harness variant is missing; after implementation it must pass with stripping intact (this is a both-flags regression pin for the spec scenario "https-to-http redirect downgrade keeps stripping sensitive headers")
- `test_camel_http_uri_override_public_cleartext_follows_endpoint_flags`
  - setup: `HttpEndpointConfig::from_uri("http://localhost/?allowCleartext=false").unwrap()`; an exchange-level `CamelHttpUri` header value `http://93.184.216.34/exfil` (the override is routed through `validate_url_for_ssrf` at lib.rs ~3128→3131)
  - action: run the override URL through the same producer validation the override path uses — `validate_url_for_ssrf(&override_url, &cfg)` — asserting the wired behavior; then repeat with `from_uri("http://localhost/?allowCleartext=true")`
  - assert: first config → `Err` containing `allowCleartext`; second → `Ok` (spec scenario "CamelHttpUri override to public cleartext follows the endpoint flags" — pinned at the validator the override path actually calls; no network needed, validation precedes dispatch)
  - command: `RUSTC_WRAPPER= cargo test -p camel-http --lib test_camel_http_uri_override_public_cleartext_follows_endpoint_flags`
  - expected: fails before steps 1+4 (option does not parse yet; verdict flips)
- Existing redirect tests updated for the new signature: `test_validate_redirect_target_blocks_private_ip` `(false→false,false)`, `test_validate_redirect_target_allows_private_ip_when_configured` `(true→true,false)`, IPv6 tests `(true/false→…,false)`, `redirect_*` end-to-end tests pass `endpoint_config.allow_cleartext` — all stay green with unchanged verdicts (they use localhost/127.0.0.1 = internal).

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-http --lib` exits 0
- `RUSTC_WRAPPER= cargo clippy -p camel-http --all-targets -- -D warnings` exits 0
- `test_camel_http_uri_override_public_cleartext_follows_endpoint_flags` exists and passes (override scenario pinned)
- `RUSTC_WRAPPER= cargo xtask schema --check` — if it reports drift caused by the new `allowCleartext` uri_param, regenerate with `RUSTC_WRAPPER= cargo xtask schema` and include the regenerated files in this task's diff (sanctioned regen; the deptree golden is the one that must NOT be regenerated); if no drift, record "schema clean"
- `rg -n "allow_internal && !is_blocked|allow_internal\s*&&\s*url.scheme" crates/components/camel-http/src/ssrf.rs` returns nothing

Result notes: schema --check = "OK: all schemas and TS types match" (schema clean, no regen; deptree golden untouched). Step 3 decision: the global-HttpConfig allowInternal fallback (from_uri_with_defaults) was NOT extended to allow_cleartext — per-endpoint consent only per ADR-0081. CamelHttpUri override verified routed through validate_url_for_ssrf at the producer call site (lib.rs:3153-3155).

- [x] 1.3

## Task 1.4 — ADR-0081, ADR-0079 superseded-by note, CONTEXT.md updates

**Files:**
- `docs/adr/0081-uniform-public-cleartext-egress-policy.md` (new)
- `docs/adr/0079-mcp-allow-internal-cleartext-alignment.md` (modified)
- `crates/components/camel-http/CONTEXT.md` (modified)
- `crates/components/camel-component-mcp/CONTEXT.md` (modified)

**Steps:**
1. LOAD the `ste-writing` skill before writing any prose (docs/AGENTS.md rule). Author ADR-0081 in ASD-STE100 style:
   - Title: `ADR-0081: Uniform public-cleartext egress policy (camel-http + MCP)`
   - Status: Accepted (decided 2026-09-18, mission 118 / bd rc-hb4fr; decides bd rc-poeg8)
   - Context: the post-ADR-0079 4×2 matrix (all four paths allow public cleartext by default); `SsrfPolicy::PublicHttpsOnly` already requires HTTPS on camel-auth/llm/keycloak egress; `CamelHttpUri` override is untrusted exchange data (ADR-0032/0071) so default-open is an attacker-steerable exfil cell with an unarmed fence.
   - Decision: the uniform rule of `specs/egress-cleartext-policy` — public cleartext rejected unless per-endpoint `allowCleartext` / per-remote `allow_cleartext` is set; independent of `allow_internal`; supersedes the DEFAULT-row semantics of ADR-0079 (its `allow_internal=true` cell and trap-closure are reaffirmed, not reversed).
   - Include: the full matrix table (before/after per path), deployment survey (honest: no inventory exists; breakage loud at send/config-load; internal-only deployments unaffected), escape-hatch comparison (per-endpoint bool vs global lever vs allowlist vs none — why per-endpoint bool), migration note (set the flag or use https), Consequences (breaking default; remedy named in errors; `allowedUriHosts` remains the host-confinement tool), References (rc-hb4fr, rc-poeg8, ADR-0079, ADR-0032, ADR-0071, ADR-0051, mission 118, e_opus ruling 2026-09-17).
2. In ADR-0079, add one line to the header bullet list: `- Superseded (default row): ADR-0081 rejects what this ADR's default row allowed — public cleartext without flags now requires allow_cleartext/allowCleartext; the allow_internal=true cell stands`. Do NOT rewrite any other ADR-0079 content.
3. In `crates/components/camel-http/CONTEXT.md`, "Outbound SSRF and TLS defaults" section: replace the sentence `When allow_internal=true, cleartext HTTP to public addresses remains forbidden.` with two sentences: `Cleartext HTTP to public addresses is rejected by default and under allow_internal=true; the allowCleartext endpoint option is the per-endpoint consent that permits it (ADR-0081).` Keep the rest of the section.
4. In `crates/components/camel-component-mcp/CONTEXT.md`, "Config keys" section, the `mcp.remotes.<name>` line: the field list currently reads "`url`, `transport`" (it already omits `allow_internal`) — make it "`url`, `transport`, `allow_internal`, `allow_cleartext`", and add one sentence after it: `Cleartext http:// remotes to public targets are rejected at load unless allow_cleartext = true (ADR-0081).`

**Tests:** documentation-only task — executable checks are the acceptance gates below (no Rust tests).

**Acceptance:**
- `rg -n "superseded|ADR-0081" docs/adr/0079-mcp-allow-internal-cleartext-alignment.md` shows the note
- `cd /home/shared/rust-camel-worktrees/cleartextpolicy && openspec validate cleartextpolicy --type change --json` reports `"valid": true` (no accidental spec/delta damage from doc edits)
- `rg -n "allowCleartext" crates/components/camel-http/CONTEXT.md` and `rg -n "allow_cleartext" crates/components/camel-component-mcp/CONTEXT.md` both hit
- ADR-0081 cites rc-hb4fr and mentions rc-poeg8 as decided
- Prose follows ste-writing (skill loaded before authoring)


Result notes: r_glm review approved with one wording fix applied — the MCP CONTEXT.md sentence now reads "rejected (IP literals at config load, hostnames at connect)" instead of "rejected at load" (hostnames reject at connect via dns_pin, not at load; the plan's prescribed sentence was inaccurate). openspec validate valid:true recorded by implementer.

- [x] 1.4
