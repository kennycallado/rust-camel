# Tasks: r5batteries

Mission containment mandate (applies to EVERY `cargo test` in this
change): run under
`systemd-run --user --scope --collect --unit=fleet-r5batteries -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill --`
with `CARGO_BUILD_JOBS=6` exported and `--test-threads=4` on the test
harness; `systemctl --user stop fleet-r5batteries` after each run;
`pgrep -c compiled_artifa` must report 0 before and after each battery
invocation.

## camel-component-grpc

### Task 1.1: protoFile parse carve-out for materialized absolute paths

**Files:**
- `crates/components/camel-component-grpc/src/config.rs` (modified)

**Steps:**
1. In `parse_grpc_uri`, replace the protoFile guard at lines ~673-680
   (currently: error if `proto.starts_with('/') ||
   proto.contains("..")` with message `proto path '{}' must be relative
   and cannot contain '..'`). New rule — the `contains("..")`
   rejection stays UNCONDITIONAL (relative and absolute alike; the
   spec scenario requires `..` traversal to fail closed in both
   forms, and the materializer never emits `..`), and absolute paths
   gain the confinement branch:
   - any path containing `..` → same error as today (message must
     still contain `proto path` and `..` so the two existing tests
     keep passing).
   - relative path without `..` → accepted, unchanged.
   - absolute path without `..` → accepted iff
     `std::fs::canonicalize(proto)` succeeds AND the canonicalized
     path starts with the canonicalized `std::env::temp_dir()` path
     component-wise (`Path::starts_with`); otherwise error
     `proto path '{proto}' is absolute and outside the OS temp
     directory — only materialized per-boot paths are accepted`
     (message must contain `proto path`; the `/etc/passwd` test
     asserts `contains("proto path")`).
2. Extract the rule into `fn proto_path_is_acceptable(proto: &str) ->
   Result<(), String>` next to `parse_grpc_uri` so the tests can target
   it directly, and call it from `parse_grpc_uri`.
3. Update the guard's explanatory comment: relative paths are the
   source-tree posture (examples, `camel run`); absolute paths exist
   for the sealed-artifact boot where `camel-cli`'s materializer
   rewrites `protoFile` to a per-boot file under the OS temp directory
   (`crates/camel-cli/src/compile/materialize.rs`).

**Tests:** (all in the existing `#[cfg(test)]` module of config.rs;
run with `cargo test -p camel-component-grpc --lib` under the
containment wrapper)
- `test_parse_grpc_uri_proto_absolute_path_rejected` (existing,
  unchanged): `protoFile=/etc/passwd&transport=plaintext` → Err
  containing `proto path`. Must still pass — `/etc/passwd` canonicalize
  target is outside the temp dir.
- `test_parse_grpc_uri_proto_traversal_rejected` (existing,
  unchanged): `protoFile=../secret.proto` → Err containing `..`.
- NEW `test_parse_grpc_uri_proto_absolute_temp_path_accepted`:
  arrange — create `{temp_dir}/camel-proto-carveout-{pid}-{counter}.proto`
  with the helloworld proto text (14 lines; unique name, no tempfile
  dep; delete in a drop guard or explicit removal at test end); act —
  `parse_grpc_uri("grpc://localhost:50051/helloworld.Greeter/SayHello?protoFile={that
  absolute path}&transport=plaintext")`; assert — `Ok`, and the parsed
  config's `proto_file` equals the absolute path.
- NEW `test_parse_grpc_uri_proto_temp_traversal_rejected`: act —
  `parse_grpc_uri` with `protoFile={temp_dir}/../etc/passwd&transport=plaintext`;
  assert — Err whose message contains `proto path` and `..` (the `..`
  rejection is unconditional; canonicalization is never reached).
- NEW `test_parse_grpc_uri_proto_nonexistent_temp_path_rejected`:
  act — `protoFile={temp_dir}/camel-proto-carveout-nonexistent.proto`
  (never created); assert — Err containing `proto path`
  (canonicalize failure = fail closed).

**Acceptance:**
- `cargo test -p camel-component-grpc --lib` exits 0 (under
  containment) — all existing tests plus the three new ones.
- `cargo fmt --check --all` clean for the crate.
- `cargo clippy -p camel-component-grpc --all-targets -- -D warnings`
  exits 0.

- [x] 1.1

## camel-cli compile pipeline

### Task 1.2: protoFile joins the R2 asset matrix

**Files:**
- `crates/camel-cli/src/compile/policy.rs` (modified)
- `crates/camel-cli/tests/compile_asset_test.rs` (modified)

**Steps:**
1. In `policy.rs::tls_uri_param_class`, add the match arm
   `"protoFile" if grpc => Some("proto file")` alongside the existing
   gRPC TLS arms. The function's doc comment (lines ~193-203) gains one
   sentence: `protoFile` is the gRPC proto-descriptor parameter — a
   non-secret file class embedded and substituted like the TLS family.
2. Extend the module-header comment (lines ~25-28) that enumerates the
   URI-parameter families with `protoFile` on gRPC endpoints.
3. No other code change: `collect_uri` already routes query pairs
   through `tls_uri_param_class` → `checked` (rejects `${` and
   leading `/`) → `SubstitutionContext::Uri`; `sources`, `materialize`,
   and `runtime` are class-agnostic.

**Tests:** (in `compile_asset_test.rs`; run with
`cargo test -p camel-cli --test compile_asset_test` under the
containment wrapper; reuse the file's existing `project()`, `compile`,
`decode_v2`, `asset_entries`, `substitution`, `assert_spans_match`
helpers exactly as `compile_collects_grpc_tls_uri_params` does)
- NEW `compile_collects_grpc_protofile_uri_param`: arrange — source
  dir with `protos/helloworld.proto` (the 14-line proto from
  `examples/grpc-example/protos/helloworld.proto`) and `app.yaml`:
  `routes:\n  - id: grpc-server\n    from: 'grpc://127.0.0.1:50051/helloworld.Greeter/SayHello?protoFile=protos/helloworld.proto&transport=plaintext'\n    steps:\n      - to: log:server\n`;
  act — compile WITHOUT `--embed-secrets`; assert — exit 0;
  `asset_entries(&store)` equals
  `[("assets/protos/helloworld.proto".into(), Some("proto file".into()))]`;
  `substitution(&store, "app.yaml", "protos/helloworld.proto")` has
  `context == SubstitutionContext::Uri` and
  `assert_spans_match(&store, entry)` holds.
- NEW `compile_rejects_absolute_grpc_protofile`: arrange — same doc
  with `protoFile=/etc/svc.proto`; act — compile; assert — exit 2,
  stderr contains `protoFile` and `root-relative`.

**Acceptance:**
- `cargo test -p camel-cli --test compile_asset_test` exits 0 (under
  containment), including the two existing protoFile-adjacent suites
  (`compile_collects_grpc_tls_uri_params` unchanged and green).
- `cargo fmt --check --all` clean.
- `cargo clippy -p camel-cli --all-targets -- -D warnings` exits 0.

- [x] 1.2

## camel-cli batteries

### Task 2.1: gRPC serve battery (sealed artifact, h2 wire probe)

**Files:**
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. Generalize deployment: add
   `fn compile_and_deploy_listener_files(files: &[(&str, &str)]) ->
   Result<(tempfile::TempDir, PathBuf), String>` — same body as the
   current `compile_and_deploy_listener` but writing each
   `(name, content)` pair into the source tempdir, calling
   `std::fs::create_dir_all` on each entry's parent directory first
   (e.g. `protos/helloworld.proto` needs `protos/` created), and
   compiling the `.yaml` entry (first pair's name with a `.yaml`
   extension is the entry document; compile call shape unchanged:
   `compile(src.path(), doc_name, "rest.bin", &[])` — the output name
   is irrelevant, keep `rest.bin`). Rewrite the existing
   `compile_and_deploy_listener(doc)` as a one-line delegate passing
   `&[("rest.yaml", doc)]` so all existing call sites stay untouched.
2. Add `const GRPC_SERVE_TEST: &str =
   "route_server_serves_grpc_listener_until_sigterm";` next to
   `SERVE_TEST`.
3. Add `fn hello_world_proto() -> &'static str` returning verbatim the
   14-line proto3 text of `examples/grpc-example/protos/helloworld.proto`
   (syntax/package helloworld/service Greeter/rpc SayHello/messages
   HelloRequest{name=1}, HelloReply{message=1}).
4. Add `fn grpc_listener_doc(port: u16) -> String`:
   `routes:\n  - id: grpc-serve\n    from: grpc://127.0.0.1:{port}/helloworld.Greeter/SayHello?protoFile=protos/helloworld.proto&transport=plaintext\n    steps:\n      - log: "grpc-request"\n`.
5. Add `fn h2_settings_probe(port: u16) -> Option<()>`: TcpStream
   connect to `127.0.0.1:port` with a 5 s read timeout; write the
   HTTP/2 client preface bytes `PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n`
   followed by the 9-byte empty SETTINGS frame header
   `[0x00,0x00,0x00, 0x04, 0x00, 0x00,0x00,0x00,0x00]`; read exactly 9
   bytes of the first server frame header; return `Some(())` iff
   byte index 3 (frame type) == `0x04` (SETTINGS), else `None`.
   Follow the `http_get` helper's error style (`.ok()?`).
6. Add `fn grpc_serve_flow() -> Result<(), String>` mirroring
   `serve_listener_flow`:
   - `let port = free_port();`
   - deploy via `compile_and_deploy_listener_files(&[("doc.yaml",
     &grpc_listener_doc(port)), ("protos/helloworld.proto",
     hello_world_proto())])`
   - `spawn_child(GRPC_SERVE_TEST, deploy.path(), &artifact,
     &["--report", "grpc-report.json"], &[])` + `spawn_drained`
   - `wait_for_marker(&drained, "context started", 60 s)` else Err
     with `drained.captured()`
   - `wait_for_marker(&drained, "grpc consumer started, waiting for
     requests", 20 s)` else Err with captured (this marker is logged
     only after `GrpcServerRegistry::get_or_spawn` binds the
     listener).
   - `h2_settings_probe(port)` must be `Some(())` else Err with
     captured.
   - liveness: `child.0.try_wait()` must be `Ok(None)` else Err
     ("artifact must stay alive while serving").
   - `send_signal(&child.0, "-TERM")`; `wait_exit_code(&mut child, 30)`
     must be 0 else Err with captured.
   - `deploy.path().join("grpc-report.json")` content equals exactly
     `{"kind":"route","status":"completed","error":null}` (mirror the
     REST flow's report assertion formatting).
   - manifest run: use the same helper the REST serve flow uses for
     its manifest step (`common::run_binary`, compiled_artifact_test.rs
     ~2304) with `--manifest`; assert exit 0 and that the parsed
     output reports `artifact_kind` `server` with `grpc` among the
     components (match the REST flow's JSON-parse assertion shape).
7. Add the test (with a doc comment citing bd rc-z332y, the
   "gRPC consumer serves from a sealed artifact until SIGTERM"
   scenario, and that the probe is wire-level h2):

```rust
#[test]
fn route_server_serves_grpc_listener_until_sigterm() {
    child_guard();
    with_bind_race_retry(grpc_serve_flow);
}
```

**Tests:**
- `route_server_serves_grpc_listener_until_sigterm`: arrange — Tasks
  1.1+1.2 landed (without them the sealed boot cannot resolve the
  proto and this battery is red); act — run under containment
  `systemd-run --user --scope --collect --unit=fleet-r5batteries
  -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p
  OOMPolicy=kill -- env CARGO_BUILD_JOBS=6 cargo test -p camel-cli
  --test compiled_artifact_test route_server_serves_grpc_listener_until_sigterm
  -- --exact --nocapture --test-threads=4`; assert — exit 0,
  `pgrep -c compiled_artifa` == 0 before and after.
- Regression guard: `route_server_serves_listener_until_sigterm` (the
  REST battery) still green after the
  `compile_and_deploy_listener` delegate change.

**Acceptance:**
- Both batteries above exit 0 under the containment wrapper.
- `#[test]` count in `compiled_artifact_test.rs` == 74.
- `cargo fmt --check --all` clean; `cargo clippy -p camel-cli
  --all-targets -- -D warnings` exits 0.

- [x] 2.1

### Task 2.2: WS drain battery (in-flight exchange survives SIGTERM)

**Files:**
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. Add `const WS_DRAIN_TEST: &str =
   "route_server_drains_inflight_ws_exchange";` next to `DRAIN_TEST`.
2. Add `fn ws_slow_doc(port: u16, delay_ms: u64) -> String`:
   `routes:\n  - id: ws-slow-echo\n    from: ws://127.0.0.1:{port}/echo\n    steps:\n      - log: "slow-enter"\n      - delay: {delay_ms}\n      - set_body: "slow-pong"\n      - to: ws://127.0.0.1:{port}/echo\n`
   (the `to:` producer runs in server-send mode because the local
   consumer exists — it echoes back on the sender's connection key,
   the same shape as `examples/ws-server`).
3. Add `fn ws_connect(port: u16) -> Option<std::net::TcpStream>`:
   bounded-retry connect loop (up to 50 attempts, 100 ms apart — same
   bounded-poll posture as `wait_for_marker`'s 20 ms steps); on
   connect, write the HTTP/1.1 upgrade request
   `GET /echo HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade:
   websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key:
   dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n`;
   read into a buffer until it contains `\r\n\r\n` (5 s read
   timeout); return the stream iff the response starts with
   `HTTP/1.1 101`, else `None`.
4. Add `fn ws_send_text(stream: &mut std::net::TcpStream, text: &str)`:
   write one masked text frame — `0x81`, `0x80 | len` (len < 126),
   mask bytes `[0x37, 0xfa, 0x21, 0x3d]`, payload XOR mask (RFC 6455
   client framing).
5. Add `fn ws_read_text(stream: &mut std::net::TcpStream) ->
   Option<String>`: read the 2-byte header, assert no mask bit
   (server frames are unmasked) else `None`; payload length =
   `byte1 & 0x7f` (< 126 for this battery), read that many bytes,
   return UTF-8 `String`.
6. Add `fn ws_drain_flow() -> Result<(), String>` mirroring
   `drain_inflight_flow`:
   - `let port = free_port();`
   - deploy `ws_slow_doc(port, 3000)` via
     `compile_and_deploy_listener_files(&[("doc.yaml", &doc)])`
   - `spawn_child(WS_DRAIN_TEST, deploy.path(), &artifact,
     &["--report", "drain-ws.json"], &[])` + `spawn_drained`
   - `wait_for_marker(&drained, "context started", 60 s)` else Err
     with captured.
   - `ws_connect(port)` else Err with captured ("ws listener never
     accepted the upgrade").
   - `ws_send_text(&mut stream, "hello")`.
   - spawn a reader thread owning the stream:
     `let (tx, rx) = std::sync::mpsc::channel();` thread sends
     `tx.send(ws_read_text(&mut stream))`.
   - `wait_for_marker_tight(&mut child, &drained, "slow-enter", 20 s)`
     else Err with captured (the exchange is provably inside the
     delay step, not merely queued).
   - `send_signal(&child.0, "-TERM")` (lands ≈0 s into the 3 s delay;
     10 s default drain budget).
   - `rx.recv_timeout(20 s)` must be `Ok(Some(s))` with
     `s.contains("slow-pong")` else Err with captured.
   - `wait_exit_code(&mut child, 30)` must be 0 else Err.
   - `deploy.path().join("drain-ws.json")` content equals exactly
     `{"kind":"route","status":"completed","error":null}`.
7. Add the test (doc comment citing bd rc-z332y and the "WebSocket
   consumer drains an in-flight exchange" scenario):

```rust
#[test]
fn route_server_drains_inflight_ws_exchange() {
    child_guard();
    with_bind_race_retry(ws_drain_flow);
}
```

**Tests:**
- `route_server_drains_inflight_ws_exchange`: act — run under
  containment: `systemd-run --user --scope --collect
  --unit=fleet-r5batteries -p MemoryMax=12G -p MemorySwapMax=2G -p
  TasksMax=1024 -p OOMPolicy=kill -- env CARGO_BUILD_JOBS=6 cargo
  test -p camel-cli --test compiled_artifact_test
  route_server_drains_inflight_ws_exchange -- --exact --nocapture
  --test-threads=4`; assert — exit 0, `pgrep -c compiled_artifa` == 0
  before and after.
- Regression guard: `route_server_drains_inflight_request` (the REST
  drain battery) still green.

**Acceptance:**
- Both batteries above exit 0 under the containment wrapper.
- `#[test]` count in `compiled_artifact_test.rs` == 75.
- `cargo fmt --check --all` clean; `cargo clippy -p camel-cli
  --all-targets -- -D warnings` exits 0.

- [x] 2.2
