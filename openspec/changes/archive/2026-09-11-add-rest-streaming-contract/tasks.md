# Tasks: add-rest-streaming-contract

## camel-dsl

### Task 1.1: DSL-level raw stream ownership pins

**Files:**
- `crates/camel-dsl/tests/rest_stream_contract_e2e.rs` (new — part 1)

**Steps:**
1. Create the test module with the DSL-level rig: a `binding: raw` YAML
   route (`steps` variant with one `set_header` user step, mirroring L1's
   `rest_raw_e2e.rs` rig: `parse_yaml` → `routes[0].steps()`, drive each
   compiled header step via `SetHeader`/`SetHeaderIfAbsent` over
   `IdentityProcessor` with `tower::ServiceExt::oneshot`).
2. Add an instrumented stream helper: a one-chunk stream whose poll closure
   increments an `Arc<AtomicUsize>` poll counter, built over the same
   `StreamBody { stream: Arc<Mutex<Option<BoxStream>>> }` idiom as L1's
   `one_chunk_stream_body`.
3. Write the four DSL-level tests (below), run them, confirm they pass
   against the landed L1 behavior (contract pins, not new behavior — a
   failure means L1 regressed).

**Tests:**
- `raw_pipeline_never_polls_request_stream`
  - arrange: raw route compiled; exchange body = poll-counting stream
    (chunk `own-this-wire`, counter at 0)
  - act: drive all compiled steps in order
  - assert: counter == 0; body variant is `Body::Stream`; `Content-Type`
    header == declared produces; default status 201 injected
- `raw_pipeline_preserves_stream_identity`
  - arrange: exchange body = stream; keep `Arc` clone of the stream mutex
    before driving
  - act: drive all compiled steps
  - assert: `Arc::ptr_eq(kept_handle, body_stream_mutex)` is true
- `request_stream_consumable_exactly_once_after_pipeline`
  - arrange: drive pipeline over one-chunk stream body; keep an `Arc` clone
    of the stream mutex; extract the body
  - act/assert: `into_bytes(64 * 1024)` on the body returns exactly the
    chunk bytes; a second take via the kept handle finds `None` and a
    second `into_bytes` on a pre-cloned `StreamBody` fails with
    `CamelError::AlreadyConsumed`
- `raw_pipeline_preserves_stream_metadata`
  - arrange: stream metadata `{ content_type: Some("image/png"),
    size_hint: Some(9) }`
  - act: drive all compiled steps
  - assert: metadata after == metadata before (both fields)

- [x] 1.1

### Task 1.2: HTTP-boundary rig and stream contract pins

**Files:**
- `crates/camel-dsl/Cargo.toml` (dev-dependencies only)
- `crates/camel-dsl/tests/rest_stream_contract_e2e.rs` (part 2)

**Steps:**
1. Add dev-dependencies: `camel-component-api` (workspace, feature
   `test-support`), `camel-component-http` (workspace), `tokio-util`
   (workspace); extend the tokio dev-dependency features with `net`,
   `time`, `io-util`.
2. Build the rig: `HttpConsumer::new(HttpServerConfig { .. }, Arc::new(
   NoopRuntimeObservability))`, `ConsumerContext::new(mpsc, token, route)`,
   spawn `consumer.start(ctx)`; a hand-rolled `tokio::net::TcpStream`
   HTTP/1.1 client (write request, read response to EOF with
   `Connection: close`); a connect-retry readiness helper (no fixed
   sleeps); a binary-local server-test mutex (process-global
   `ServerRegistry` is shared); the bind-drop free-port idiom; unique path
   per test.
3. Write the seven boundary tests (below). Each fulfills the received
   `ExchangeEnvelope` by hand and answers on its `reply_tx`.

**Tests:**
- `http_request_metadata_carries_content_type_and_length`
  - arrange: server on free port, `method: Some("POST")`, path `/meta`
  - act: real POST with `Content-Type: image/png`, `Content-Length: 4`,
    body `PNG!`
  - assert: envelope exchange body is `Body::Stream`; metadata
    `content_type == Some("image/png")`; `size_hint == Some(4)`
- `chunked_request_over_cap_fails_closed`
  - arrange: server with `max_request_body: 8`
  - act: hand-rolled chunked POST (two 8-byte chunks, no Content-Length);
    handler consumes via `into_bytes(64 * 1024)`
  - assert: consumption errors with the cap error containing
    `exceeds configured limit`
- `in_route_double_consumption_surfaces_already_consumed`
  - arrange: envelope with readable stream
  - act: handler consumes twice (`into_bytes` then `into_bytes` on a
    pre-cloned `StreamBody`)
  - assert: first returns exact bytes; second fails
    `CamelError::AlreadyConsumed`; the error is returned as the reply
    `Err`, and the client observes a 5xx (error-reply mapping), server
    stays up
- `consumed_reply_stream_returns_500`
  - arrange: envelope whose stream the handler fully consumes before
    replying; reply sends the exchange still carrying the consumed
    `StreamBody`
  - act/assert: client response status == 500; body empty
- `materialized_reply_bytes_over_cap_replaced_with_500`
  - arrange: `max_response_body: 8`; handler replies `Body::Bytes` of 32
    bytes with `Content-Type` header set
  - assert: status == 500; body == `Response body exceeds configured
    limit`
- `streamed_reply_over_max_response_body_succeeds`
  - arrange: `max_response_body: 8`; handler replies a new `Body::Stream`
    of 32 bytes
  - assert: status == 200; full 32 bytes on the wire
- `reply_may_stream_original_request_body`
  - arrange: POST body `echo-me`; handler replies the exchange WITHOUT
    touching the body (original stream left in place), `Content-Type:
    application/octet-stream` header set
  - assert: status == 200; body == `echo-me`; response `Content-Type ==
    application/octet-stream`
- `client_disconnect_during_streamed_reply_keeps_server_healthy`
  - arrange: handler replies a stream that yields one chunk then pends
    forever; client reads the first chunk
  - act: client drops the socket; then a second full request to the same
    server
  - assert: second request served with 200 + expected body; cancel token
    tears the server down cleanly

- [x] 1.2

### Task 1.3: docs and change validation

**Files:**
- `docs/src/yaml-dsl/step-verbs.md` (REST DSL section — new subsection)
- `openspec/changes/add-rest-streaming-contract/` (artifacts)

**Steps:**
1. Add a "Raw binding streaming contract" subsection after the raw binding
   paragraphs: single-consumption ownership, no pipeline caching or
   materialization, request metadata (content type + length), original-or-
   new reply stream, request caps (413 pre-check + mid-stream cap),
   response limit policy (materialized bytes only; streamed replies
   uncapped by design), consumed reply stream 500, client-disconnect
   semantics.
2. Run `openspec validate add-rest-streaming-contract --type change
   --json` and fix any delta-structure errors.
3. Cross-check every scenario in the delta spec against a test name in
   `rest_stream_contract_e2e.rs` (1:1 or explicit existing-pin reference).

**Tests:** documentation change — verified by review; no runtime tests.

- [x] 1.3
