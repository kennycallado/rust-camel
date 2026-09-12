# Tasks: jsonreply

## camel-http

### Task 1.1: Extract shared JSON error reply construction

**Files:**
- `crates/components/camel-http/src/lib.rs` (modified)

**Steps:**
1. Add a private `json_error_reply(status: u16, code: &str, message: String) -> HttpReply`
   beside `pipeline_error_to_reply`, using the existing JSON object, application/json
   header, `Bytes` body, and `unwrap_or_else(|_| "{}".to_string()) // allow-unwrap`
   fallback unchanged.
2. Replace the body-construction blocks in the `TypeConversionFailed`,
   `ValidationError`, `UnsupportedMediaType`, and `NotAcceptable` arms with calls to
   the helper, leaving each arm's existing warning invocation and message formatting
   in place.
3. Extend the existing four HTTP finalizer unit tests to assert parsed JSON error
   codes and messages without asserting object key order; add one helper-level
   empty-message test.
4. Run formatting, the focused camel-http tests, and the affected-crate clippy check.

**Tests:** (executable spec — name, arrange, act, assert)
- `type_conversion_failed_maps_to_400`: arrange the existing finalizer test inputs;
  act by calling the mapper; assert status 400, one application/json header, and
  parsed fields `bad_request` plus the existing message; command
  `cargo test -p camel-component-http --lib -- error_reply maps` — expected pass after
  implementation, with the pre-refactor test baseline passing before edits.
- `validation_error_maps_to_400`: arrange the existing finalizer test inputs; act by
  calling the mapper; assert status 400, application/json, and parsed fields
  `validation_error` plus the existing message; command
  `cargo test -p camel-component-http --lib -- error_reply maps` — expected pass.
- `finalizer_maps_unsupported_media_type`: arrange the existing test with
  `CamelError::UnsupportedMediaType { consumed: "text/plain", declared: "application/json" }`;
  act by calling the mapper; assert status 415, application/json, error code
  `unsupported_media_type`, and exact message `consumed text/plain, declared application/json`;
  command `cargo test -p camel-component-http --lib -- error_reply maps` — expected pass.
- `finalizer_maps_not_acceptable`: arrange the existing test with
  `CamelError::NotAcceptable { accept: "application/xml", produced: "application/json" }`;
  act by calling the mapper; assert status 406, application/json, error code
  `not_acceptable`, and exact message `accept application/xml, produced application/json`;
  command `cargo test -p camel-component-http --lib -- error_reply maps` — expected pass.
- `json_error_reply_preserves_empty_message`: arrange helper inputs `(400, "bad_request", "".to_string())`;
  act by constructing the reply; assert valid JSON with empty `message`, status 400,
  and application/json; command `cargo test -p camel-component-http --lib -- error_reply maps` — expected pass.

**Acceptance:**
- `cargo fmt --check --all` exits 0.
- `cargo test -p camel-component-http --lib -- error_reply maps` exits 0 and exercises all five parity tests.
- `cargo clippy -p camel-component-http --lib -- -D warnings` exits 0.
- All four match arms call the one private helper; warning text and structured fields remain unchanged.
- `cargo xtask lint-unwrap` reports no new violation and the existing fallback marker remains in place.

- [x] 1.1
