# Tasks: mail-attachments

## Phase 1: raw message access on IMAP consumption

### camel-component-mail

#### Task 1.1: `raw` URI parameter and metadata parity

**Files:**
- `crates/components/camel-mail/src/config.rs` (modified)
- `crates/components/camel-mail/src/metadata.rs` (modified)

**Steps:**
1. Add `pub raw: bool` to `ImapEndpointConfig` (struct at `config.rs:485-500`) and initialize it to `false` in `Default`.
2. Add `"raw"` to the `IMAP_OPTIONS` const (`config.rs:425-438`).
3. Parse it in `ImapEndpointConfig::from_uri` (`config.rs:503`) with the existing `bool_param` helper (`config.rs:355`), storing `raw`.
4. Add a `raw` entry to `ImapMetadataDescriptor` (`metadata.rs:77-113`) with the same `#[uri_param]` attributes as the other boolean params, marked consumer and polling_consumer.
5. Run the existing metadata parity test and fix any mismatch it reports.

**Tests:** (executable spec)
- `raw_defaults_to_false`: `IMAP_OPTIONS` route with no `raw` → `from_uri("imap://h?account=a")` → `cfg.raw == false`; command `cargo test -p camel-component-mail --lib raw_defaults_to_false`.
- `raw_true_and_false_parse`: `?account=a&raw=true` → `true`; `?account=a&raw=false` → `false`.
- `raw_invalid_value_rejected`: `?account=a&raw=maybe` → `Err` naming `raw`.
- `metadata_descriptor_matches_imap_options` (existing parity test): passes with `raw` present in both places.

**Acceptance:**
- `cargo test -p camel-component-mail --lib` passes.
- `cargo clippy -p camel-component-mail --all-targets -- -D warnings` exits 0.

- [x] 1.1

#### Task 1.2: raw inbound body mapping

**Files:**
- `crates/components/camel-mail/src/adapter/inbound.rs` (modified)
- `crates/components/camel-mail/src/poll.rs` (modified)
- `crates/components/camel-mail/src/polling.rs` (modified)

**Steps:**
1. Change `parse_inbound` (`inbound.rs:41-92`) to `parse_inbound(raw_bytes: &[u8], raw_mode: bool) -> Message`.
2. Keep the existing `mail-parser` header extraction for `CamelMailInFrom`, `CamelMailInTo`, `CamelMailInCc`, `CamelMailInSubject`, `CamelMailInMessageId`, `CamelMailInUid`, `CamelMailInDate`, `CamelMailInFolder` in both modes.
3. In `raw_mode`, set the body to `Body::Bytes(Bytes::copy_from_slice(raw_bytes))` and skip text/HTML body selection and the `CAMEL_MAIL_IN_HTML_BODY` header entirely.
4. Pass `cfg.raw` from `Cycle::load` (`poll.rs:381`) into `parse_inbound`; confirm the polling consumer (`polling.rs`) reaches the same call and passes the same flag.
5. Leave disposition, `maxAttempts`, `maxMessageSize`, and idempotency code untouched.

**Tests:**
- `raw_body_is_exact_message_bytes`: `ATTACH_EML` fixture bytes → `parse_inbound(&bytes, true)` → body is `Body::Bytes` byte-equal to `bytes`.
- `raw_omits_html_body_header`: multipart/alternative fixture → raw mode → no `CAMEL_MAIL_IN_HTML_BODY` header, subject/message-id/from headers present.
- `raw_headers_populated`: raw mode → `CAMEL_MAIL_IN_SUBJECT`, `CAMEL_MAIL_IN_FROM`, `CAMEL_MAIL_IN_MESSAGE_ID`, `CAMEL_MAIL_IN_UID` all present.
- `attachments_ignored` and `inbound_sets_only_in_headers` (existing, `inbound.rs:204-211`, `:221`) still pass unmodified, proving the default path.

**Acceptance:**
- `cargo test -p camel-component-mail --lib` passes.
- `cargo clippy -p camel-component-mail --all-targets -- -D warnings` exits 0.

- [x] 1.2

#### Task 1.3: raw disposition and pollEnrich tests

**Files:**
- `crates/components/camel-mail/src/consumer/tests.rs` (modified)
- `crates/components/camel-mail/src/polling/tests.rs` (modified)

**Steps:**
1. Extend the scripted-route harness (`consumer/tests.rs:44-96`, `Route::Fail`) with a test that builds the consumer from a URI carrying `raw=true`.
2. Add a polling-consumer test that calls the `PollingConsumer` against the fake store and asserts the returned exchange body.
3. Do not change production code; this task pins the guarantees. In `raw_failing_route_leaves_message_unseen`, add a doc comment recording that a scripted `Route::Fail` stands in for the spec scenario where a `split mime` limit fails the route: the component cannot depend on camel-processor, so the splitter error path is proven separately in camel-processor and camel-test.

**Tests:**
- `raw_failing_route_leaves_message_unseen`: fake store with one UNSEEN message, `raw=true`, `Route::Fail` → after the poll, the message is still absent from any SEEN disposition and the failed cycle recorded an attempt toward `maxAttempts`.
- `polling_consumer_raw_returns_exact_bytes`: `MailPollingConsumer` over a store serving fixture bytes with `raw=true` → returned body is byte-equal to the served `BODY.PEEK[]` bytes and no `CamelMailInHtmlBody` header is set.
- `no_duplicate_after_success`, `oversize_skipped_before_body_fetch`, `skipped_messages_do_not_starve_new_mail`, `cancellation_stops_polling`, `reconnect_after_connection_loss`, `refused_login_stops_consumer`, `inbound_headers_cannot_retarget` (existing) still pass.

**Acceptance:**
- `cargo test -p camel-component-mail` passes (lib + integration).
- `cargo clippy -p camel-component-mail --all-targets -- -D warnings` exits 0.

- [x] 1.3

#### Task 1.4: mail docs and crate context for `raw`

**Files:**
- `docs/src/components/mail.md` (modified)
- `crates/components/camel-mail/CONTEXT.md` (modified)

**Steps:**
1. Document `raw` in the receiving section (`mail.md:72`): boolean, default `false`, body becomes the exact RFC 822 bytes, `CamelMailIn*` headers kept, `CamelMailInHtmlBody` omitted.
2. Update the `pollEnrich` section (`mail.md:186`) with the same flag.
3. Reword the Not-supported bullet (`mail.md:219`) to "Attachments are ignored unless `raw=true`." (no link to the MIME splitter page: Phase 2 adds it).
4. Add one line to the crate `CONTEXT.md` describing `raw` as the opt-in raw-bytes mode.

**Acceptance:**
- `cd docs && mdbook test` exits 0.
- `cargo xtask lint-context-citations` exits 0.

- [x] 1.4

## Phase 2: reusable MIME splitter

### camel-api

#### Task 2.1: `Mime` variant and mime-only split options

**Files:**
- `crates/camel-api/src/splitter.rs` (modified)
- `crates/camel-api/tests/mime_split_config.rs` (new)
- `crates/camel-core/src/lifecycle/application/route_definition.rs` (modified: exhaustive `StreamSplitConfig` literal at :1695/:1707)
- `crates/camel-core/src/startup_validation.rs` (modified: literal at :677)
- `crates/camel-dsl/src/compile.rs` (modified: literal at :5967)
- `crates/camel-dsl/src/yaml.rs` (modified: literal at :1081)

**Steps:**
1. Add variant `Mime` to `StreamSplitFormat` (`splitter.rs:135-168`) after `TarGz`, with a doc comment: materialized MIME message, one fragment per attachment part.
2. Add to `StreamSplitConfig` (`splitter.rs:186-197`): `pub max_parts: Option<usize>`, `pub max_part_size: Option<usize>`, `pub content_types: Option<Vec<String>>`, each `#[serde(default, skip_serializing_if = "Option::is_none")]`; set them to `None` in `Default`.
3. Extend `validate()` (`splitter.rs:224+`): for `Mime`, reject `chunk_size.is_some()` and `batch_size != 1`; reject `max_parts == Some(0)` and `max_part_size == Some(0)`; for every non-`Mime` format, reject any of the three new options with an error naming the offending option.
4. Fix every existing `StreamSplitConfig {` struct literal in the workspace (grep found 12 files), either adding the new fields or `..Default::default()`; known sites include `crates/camel-dsl/src/yaml.rs`, `crates/camel-dsl/src/compile.rs`, `crates/camel-processor/src/stream_codec/chunks.rs`, `crates/camel-processor/src/stream_codec/lines.rs`, `crates/camel-processor/src/stream_codec/ndjson.rs`, `crates/camel-core/src/startup_validation.rs`, `crates/camel-core/src/lifecycle/application/route_definition.rs`, `crates/camel-api/tests/splitter_schema.rs`, and `crates/camel-test` integration tests.

**Tests:**
- `mime_variant_serializes_snake_case`: `serde_json::to_value(StreamSplitFormat::Mime) == json!("mime")`.
- `mime_options_accepted`: `validate()` Ok with all three options set.
- `mime_options_rejected_on_zip`: `format: Zip` + `max_parts: Some(5)` → `Err` containing `max_parts`.
- `mime_rejects_chunk_size` and `mime_rejects_batch_size`: `Err` naming the option.
- `zero_limits_rejected`: `max_parts: Some(0)` and `max_part_size: Some(0)` each `Err`.
- `existing_config_deserializes_without_new_fields`: JSON without the new keys parses with `None`s and re-serializes without the keys.

**Acceptance:**
- `cargo test -p camel-api` passes.
- `cargo check --workspace --all-targets` exits 0 (every literal site fixed).
- `cargo clippy -p camel-api --all-targets -- -D warnings` exits 0.

- [x] 2.1

### camel-dsl

#### Task 2.2: `format: mime` YAML parsing

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/tests/mime_split_yaml.rs` (new)

**Steps:**
1. Add `max_parts: Option<usize>`, `max_part_size: Option<usize>`, `content_types: Option<Vec<String>>` to `StreamConfigYaml` (`route_ast.rs:970-980`).
2. In the `RouteDslStep::Split` arm (`yaml.rs:1078-1109`) add `Some("mime") => StreamSplitFormat::Mime` and pass the three parsed options into `StreamSplitConfig`.
3. Leave the unknown-format error path unchanged; `validate()` still runs after construction.

**Tests:**
- `parse_stream_split_mime_with_options`: YAML with `format: mime`, `max_parts: 5`, `content_types: [application/zip]` → config format `Mime`, options populated.
- `parse_stream_split_mime_defaults`: no options → `None`s.
- `old_zip_yaml_still_parses`: existing zip YAML without new keys → Ok.
- `mime_options_with_zip_rejected`: zip + `max_parts` → route parse error naming the option.

**Acceptance:**
- `cargo test -p camel-dsl` passes.
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.

- [x] 2.2

### camel-processor

#### Task 2.3: MIME splitter core emission and decoding

**Files:**
- `crates/camel-processor/Cargo.toml` (modified)
- `crates/camel-processor/src/mime_splitter.rs` (new)
- `crates/camel-processor/src/lib.rs` (modified)

**Steps:**
1. Add `mail-parser = { workspace = true }` to `[dependencies]`.
2. Register `pub mod mime_splitter;` in `lib.rs`.
3. Define `pub struct MimeSplitConfig { pub max_parts: usize, pub max_part_size: Option<usize>, pub content_types: Option<Vec<String>> }` with `Default` = `{ 100, None, None }`, `Clone`, `Debug`.
4. Define `pub fn split_mime_bytes(parent: Exchange, bytes: Bytes, config: MimeSplitConfig) -> Pin<Box<dyn Stream<Item = Result<Exchange, CamelError>> + Send>>` using the same `async_stream` shape as `split_zip_bytes` (`zip_splitter.rs:72`).
5. Parse with `mail_parser::MessageParser::default().parse(bytes.as_ref())`; `None` → `Err(CamelError::TypeConversionFailed("MIME message could not be parsed".into()))`. Compute the effective per-part limit as `config.max_part_size.unwrap_or(bytes.len())` (the spec default is the input length, not unlimited).
6. Iterate `message.attachments()`: for `PartType::Text`/`PartType::Html` use the charset-converted text as UTF-8 bytes (the parser's text result, not the original octets); `PartType::Binary`/`PartType::InlineBinary` use the transfer-encoding-decoded and NOT decompressed bytes; `PartType::Message(m)` emits `m.raw_message` as one fragment without recursing. Skip `PartType::Multipart`.
7. Declare header constants in `mime_splitter.rs`: `CAMEL_MIME_PART_INDEX`, `_NAME`, `_CONTENT_TYPE`, `_SIZE`, `_DISPOSITION`, `_CONTENT_ID` (`CamelMimePart*`).
8. Build each fragment with `camel_api::fragment_exchange(&parent, Body::Bytes(decoded))`, then remove any inherited `CamelMimePart*` headers and insert index, sanitized name, declared content type, size, disposition (`attachment`/`inline` via `Content-Disposition`), and content id when present.
9. Do not set `CamelSplitIndex`/`CamelSplitSize`: `StreamingSplitSegment` (`streaming_split_segment.rs:107-112`) sets them.

**Tests:**
- `emits_zero_fragments_for_plain_text`
- `emits_one_and_three_in_document_order`
- `alternative_bodies_not_emitted`
- `text_csv_attachment_emitted`: a `text/csv` part with attachment disposition and a non-UTF-8 charset is emitted as UTF-8-converted bytes
- `inline_image_emitted_with_content_id`
- `message_rfc822_one_raw_fragment_no_descendants`
- `base64_aaec_decodes_to_0_1_2`: decoded body is `[0,1,2]`, `CamelMimePartSize == 3`, `CamelMimePartDisposition == "attachment"`
- `default_max_part_size_is_input_length`: a Latin-1 text attachment whose UTF-8 conversion exceeds the input length fails with the default config
- `gzip_attachment_not_decompressed`: an `application/gzip` part decodes to bytes starting with `1f 8b` and equal to the compressed payload
- `quoted_printable_decoded`
- `parent_headers_copied`

**Acceptance:**
- `cargo test -p camel-processor mime_splitter` passes.
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0.

- [x] 2.3

#### Task 2.4: limits, allowlist, sanitization, malformed input

**Files:**
- `crates/camel-processor/src/mime_splitter.rs` (modified)

**Steps:**
1. Apply `content_types` before counting: case-insensitive `type/subtype` match ignoring parameters; no wildcard; a part without a declared Content-Type never matches a non-empty list; absent/empty list matches all.
2. Check `max_parts` before emitting fragment `max_parts + 1`; check `max_part_size` (decoded length) before building the fragment body; both yield a `CamelError::TypeConversionFailed` error naming the limit.
3. Error when `part.is_encoding_problem` is true (malformed transfer encoding).
4. Implement `sanitize_part_name(raw: Option<&str>, index: usize) -> String`: RFC 2231-decoded name comes from `MessagePart::attachment_name()`, else Content-Type `name` attribute; keep basename after last `/` or `\`; strip NUL/C0/C1/DEL; trim leading/trailing spaces and dots; empty/`.`/`..` → `part-{index}`; cap 255 bytes on a UTF-8 boundary.
5. Control-strip and cap `CamelMimePartContentType` and `CamelMimePartContentId` values at 255 bytes; never interpolate them into logs or error strings.

**Tests:**
- `max_parts_excess_fails`
- `filtered_parts_do_not_consume_limit` (3 zips then 1 PDF, allowlist PDF, `max_parts: 2` → PDF emitted at index 0)
- `max_part_size_excess_fails`
- `allowlist_drops_non_matching`
- `all_parts_filtered_succeeds_empty`
- `allowlist_case_insensitive_parameter_blind`
- `sanitization_table`: `../../etc/passwd`→`passwd`, `a\x00b.txt`→`ab.txt`, `  .hidden.  `→`hidden`, `..`→`part-0`, ` report .zip `→`report .zip`
- `multibyte_name_capped_at_utf8_boundary`
- `unparseable_message_fails`
- `malformed_transfer_encoding_fails`
- `inherited_content_id_not_leaked`
- `content_type_and_id_control_stripped_and_capped`: content-type and content-id header values carrying control characters and more than 300 bytes are stripped and capped at 255 bytes
- `name_falls_back_to_content_type_name`: no Content-Disposition filename, but `Content-Type; name=x.pdf` yields `x.pdf`
- `rfc2231_filename_decoded`: `filename*=UTF-8''caf%C3%A9.zip` yields `café.zip`

**Acceptance:**
- `cargo test -p camel-processor mime_splitter` passes.
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0.

- [x] 2.4

### camel-core and integration

#### Task 2.5: compile arm and end-to-end routes

**Files:**
- `crates/camel-core/src/lifecycle/adapters/step_compilers/splitting.rs` (modified)
- `crates/camel-test/tests/mime_split_integration.rs` (new)
- `crates/camel-test/tests/zip_split_integration.rs` (no edit expected: already uses `..Default::default()`)
- `crates/camel-test/tests/tar_split_integration.rs` (no edit expected: already uses `..Default::default()`)

**Steps:**
1. In the `BuilderStep::DeclarativeStreamSplit` arm (`splitting.rs:266`), add a `StreamSplitFormat::Mime` arm that builds `MimeSplitConfig` from `max_parts.unwrap_or(100)`, `max_part_size`, `content_types`; extract bytes with `extract_bytes_from_body(&exchange, "MIME")` (accepts `Body::Bytes`/`Body::Text`, errors otherwise); set `CamelSplitMaterialized` and `CamelSplitMaterializedBytes` properties like the ZIP arm; call `camel_processor::mime_splitter::split_mime_bytes`.
2. Insert the `Mime` arm before the existing wildcard arm of `match config.format`; `StreamSplitFormat` is `#[non_exhaustive]` so the wildcard stays — add no new wildcard.
3. Build the integration tests on the `zip_split_integration.rs` harness (`CamelTestContext`, `BuilderStep::DeclarativeStreamSplit`, mock endpoint). Use `convert_body_to(BodyType::Bytes)` where the source body is a stream.

**Tests:**
- `test_mime_split_multi_part`: timer → set MIME body → streaming split `format: Mime` → mock: two fragments, decoded bodies, `CamelMimePartIndex`/`CamelMimePartName` headers, `CamelSplitSize == 2` on the last fragment.
- `test_mime_split_max_parts_plumbs`: `max_parts: Some(1)` with two attachments → the route fails with an error message naming `max_parts`; the splitter streams, so the first fragment (permitted by `max_parts: 1`) may be delivered before the failure (proves options reach the splitter).
- `test_poll_enrich_mime_limit_does_not_roll_back`: a `pollEnrich` step over `file:inbox?noop=true` (the file component returns `Body::Stream` and moves consumed files to `.camel` by default) with a two-attachment file, a `convert_body_to(BodyType::Bytes)` step before the split, and `max_parts: Some(1)` → the route fails with an error naming `max_parts`, the source file is still in place under `noop=true`, and nothing restores or re-reads it.

**Acceptance:**
- `cargo test -p camel-test mime_split` passes.
- `cargo test -p camel-core` passes.
- `cargo clippy -p camel-core --all-targets -- -D warnings` exits 0.

- [x] 2.5

### schemas

#### Task 2.6: regenerate schemas

**Files:**
- `schemas/canonical-route-spec.json` (modified)
- `schemas/dsl/route-schema.json` (modified)
- `schemas/ts/StreamSplitFormat.ts` (modified)
- `schemas/ts/StreamSplitConfig.ts` (modified)

**Steps:**
1. Run `RUSTC_WRAPPER= TMPDIR=/home/shared/tmp cargo xtask schema`.
2. Inspect the diff: `Mime` appears in the format enum, the three new optional fields appear on the config, no existing entries disappear.
3. Run `cargo xtask schema --check`.

**Tests:**
- `cargo test -p camel-api --test splitter_schema` passes (schema drift test).
- `cargo xtask schema --check` exits 0.

**Acceptance:**
- Both commands above pass; `schemas/ts/StreamSplitFormat.ts` contains `"mime"` and `StreamSplitConfig.ts` contains the new fields.

- [x] 2.6

### docs and ADR

#### Task 2.7: MIME splitter docs, DMARC recipe, ADR-0094

**Files:**
- `docs/src/eip/mime-splitter.md` (new)
- `docs/src/SUMMARY.md` (modified)
- `docs/src/eip/streaming-splitter.md` (modified)
- `docs/src/components/mail.md` (modified)
- `docs/adr/0094-untrusted-mime-parts.md` (new)
- `crates/camel-processor/CONTEXT.md` (modified)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Write `mime-splitter.md` (mirror `zip-splitter.md` structure): options (`max_parts` 100, `max_part_size` default input length, `content_types`), fragment headers, decoded bodies, inline images, `message/rfc822` non-recursion, no decompression, sanitized names, declared content type untrusted, text/HTML parts arrive charset-converted to UTF-8, note that DMARC reports often use `application/octet-stream`.
2. Add the page to `SUMMARY.md` and a one-line pointer in `streaming-splitter.md` next to zip/tar.
3. Add a DMARC recipe to `mail.md` (`imap(raw=true)` → `split` `format: mime` → `unmarshal zip|gzip` → `unmarshal xml`) and add the MIME splitter page link to the attachments bullet (Phase 1 only rewords that bullet).
4. Write ADR-0094 "Untrusted MIME parts": exchange data (names, content types, content ids) is untrusted (ADR-0032), splitter emits decoded bytes without decompression, limits bound emission, `message/rfc822` is not recursed.
5. Add a MIME splitter line to `crates/camel-processor/CONTEXT.md`; add the key term to `CONTEXT-MAP.md` Key Terms with the ADR citation.

**Acceptance:**
- `cd docs && mdbook test` exits 0.
- `cargo xtask lint-context-citations` exits 0.
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-processor --no-deps` exits 0.

- [x] 2.7
