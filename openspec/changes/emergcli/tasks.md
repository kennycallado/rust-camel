# Tasks: emergcli

## camel-component-http

### Task 1.1: Strict fail-closed terminal + forced rebuild-failure seam

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)
- `crates/components/camel-http/src/tls_harness.rs` (modified)

**Steps:**
1. Add `#[cfg(test)]` thread-local seam beside the existing
   `FORCE_WEBPKI_FALLBACK` (lib.rs ~2298):
   `static FORCE_FALLBACK_REBUILD_FAIL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };`
2. In `tls_harness.rs`, add
   `pub(crate) fn force_webpki_fallback_rebuild_failure<R>(body: impl FnOnce() -> R) -> R`
   mirroring `force_webpki_fallback` (~line 273): a `ResetBoth` guard
   struct whose `Drop` resets BOTH `FORCE_WEBPKI_FALLBACK` and
   `FORCE_FALLBACK_REBUILD_FAIL` to `false` (panic-safe), nesting
   rejected by asserting neither flag is already armed before setting
   both to `true`, then `body()`.
3. In `webpki_fallback_client` (lib.rs ~2811), extract the
   `Err(second_error)` arm of the `build_with_backend` closure into
   `fn fallback_rebuild_failed(config: &HttpConfig, failure: &dyn std::fmt::Display) -> Result<reqwest::Client, CamelError>`:
   first emit the existing `error!` line unchanged (log-policy:
   system-broken, message "webpki fallback client build failed — TLS
   stack broken process-wide", field `error = %failure`); then, when
   `config.tls.as_ref().is_some_and(|tls| tls.strict)`, return
   `Err(CamelError::EndpointCreationFailed(format!("tls.strict/webpki-fallback: webpki fallback client rebuild failed — refusing material-free emergency client under tls.strict: {failure}")))`;
   otherwise return `Ok(emergency_webpki_client())`.
4. In the `build_with_backend` closure: before returning a real build
   `Ok`, honor the seam using the crate's statement-attribute form
   (same pattern as the existing seam at lib.rs ~2878-2881 — no
   `let forced` binding, zero release surface):
   `#[cfg(test)] if FORCE_FALLBACK_REBUILD_FAIL.with(std::cell::Cell::get) { return fallback_rebuild_failed(config, &"forced webpki fallback rebuild failure (test)"); }`
   inside the `Ok` arm; the real `Err(second_error)` arm calls
   `fallback_rebuild_failed(config, &second_error)`. Release builds
   keep byte-identical behavior.
5. Update the "Shared second-error terminal" comment block (~2805) and
   the `webpki_fallback_client` doc comment: the rebuild terminal is
   strict-fail-closed (typed error), non-strict keeps the
   emergency-client degrade; `Err` remains strict-only by construction.
6. Write the two direct tests below AFTER steps 1-2 (they call the
   seam helper) but BEFORE steps 3-4: verify the strict test is red
   against the unmodified terminal, then implement steps 3-4 until
   green. The non-strict test is a regression guard (it passes
   pre-fix because the current terminal already degrades — its value
   is pinning that behavior).

**Tests:** (executable spec — name, arrange, act, assert)
- `strict_rebuild_failure_returns_typed_error`:
  `tls_harness::gen_test_ca()` + `gen_client_identity(&ca)` written to
  temp files as VALID `ca_cert_path`/`client_cert_path`/`client_key_path`
  (material loads cleanly — no material error can precede the
  terminal) → inside `force_webpki_fallback_rebuild_failure`, call
  `build_client(&strict_config, None)` with `tls.enabled=true,
  strict=true` → `Err(CamelError::EndpointCreationFailed(msg))` where
  `msg` contains `tls.strict/webpki-fallback:` AND `rebuild failed`
  AND `forced webpki fallback rebuild failure` (the seam sentinel),
  and `build_client_fallback_count()` delta over the call is exactly 1.
  Assert via `assert_strict_fallback_error(err, &[...])` (lib.rs
  ~14209) with the three fragments.
- `non_strict_rebuild_failure_returns_emergency_client`:
  same CA/identity fixtures, `strict=false`, inside
  `force_webpki_fallback_rebuild_failure` → `build_client` returns
  `Ok(client)`, fallback-count delta exactly 1.
- Command: `cargo test -p camel-component-http --lib strict_rebuild_failure non_strict_rebuild_failure`
  (both fns). Expected: strict test red before steps 3-4, pass after;
  non-strict test green throughout (regression guard).

**Acceptance:**
- `cargo test -p camel-component-http --lib` exits 0 with both new
  tests passing.
- `cargo clippy -p camel-component-http -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.
- `rg -n -B2 'FORCE_FALLBACK_REBUILD_FAIL' crates/components/camel-http/src/lib.rs` shows the static inside the existing `#[cfg(test)] thread_local!` block (no release-build surface).

- [x] 1.1

### Task 1.2: Endpoint surfacing, handshake proof, docs alignment

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)
- `crates/components/camel-http/CONTEXT.md` (modified)

**Steps:**
1. Write test `strict_rebuild_failure_surfaces_at_endpoint_creation`:
   same VALID strict config as task 1.1; inside
   `force_webpki_fallback_rebuild_failure`, construct the HTTP
   component exactly as the existing strict-fold test does (the test
   near lib.rs ~14750 that documents "folds into strict_tls_error and
   surfaces at endpoint") → construction completes without panic;
   `create_endpoint(...)` returns `Err(CamelError::EndpointCreationFailed(msg))`
   whose `msg` contains `tls.strict/webpki-fallback:` and
   `rebuild failed` (mirrors spec scenario 2).
2. Write test `non_strict_rebuild_failure_drops_custom_trust`:
   `gen_test_ca()` + `gen_leaf_signed_by_ca(&ca, 127.0.0.1)` server
   fixtures started exactly as the existing forced-fallback handshake
   tests do (the `tls_handshake_*` suite at lib.rs ~14811-15300; model
   test `tls_handshake_strict_material_free_fails_same_server` at
   ~14856, with the `send_result.is_err()` assertion pattern at
   ~14874); inside
   `force_webpki_fallback_rebuild_failure` with `strict=false` and
   `ca_cert_path` = the CA, build the client → `Ok`; request
   `https://` from that server → the handshake FAILS (the returned
   emergency client carries Mozilla roots only — custom trust
   provably dropped, mirroring spec scenario 3's THEN). This test is
   genuinely red pre-fix: the real fallback client would carry the CA
   and the handshake would succeed. Assert the
   terminal's exact system-broken log via the crate's existing
   `tracing-test` dev-dependency (`#[traced_test]` +
   `logs_contain("webpki fallback client build failed")`) — the
   existing `error!` line text is unchanged by this change.
3. `CONTEXT.md` "Outbound SSRF and TLS defaults" section: add one
   sentence after the existing strict-fallback paragraph — the
   fallback REBUILD terminal also fails closed under `tls.strict`
   with the `tls.strict/webpki-fallback:` typed error, while
   non-strict keeps the emergency-client degrade (bd rc-3x5qj).
4. Run the full mission gate set and fix any fallout.

**Tests:** (executable spec — name, arrange, act, assert)
- `strict_rebuild_failure_surfaces_at_endpoint_creation`:
  valid strict config + armed guard → construct component (no panic) →
  `create_endpoint` → `Err` with `tls.strict/webpki-fallback:` +
  `rebuild failed`. Command:
  `cargo test -p camel-component-http --lib strict_rebuild_failure_surfaces`.
  Expected: red before task 1.1 lands, pass after.
- `non_strict_rebuild_failure_drops_custom_trust`:
  local CA-signed TLS server + armed guard + strict=false + valid CA
  path → `build_client` `Ok` → HTTPS request to that server →
  handshake error (assert the request error variant used by the
  existing `tls_handshake_strict_material_free_fails_same_server`
  model test near lib.rs ~14856-14874).
  Command: `cargo test -p camel-component-http --lib non_strict_rebuild_failure_drops_custom_trust`.
  Expected: red before task 1.1 lands, pass after.

**Acceptance:**
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-component-http -- -D warnings` exits 0.
- `cargo test -p camel-component-http` exits 0 (all four new tests +
  existing suite green).
- `CONTEXT.md` carries the rebuild-terminal sentence referencing bd
  rc-3x5qj.

- [x] 1.2
