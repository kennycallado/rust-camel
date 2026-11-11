# Tasks: pooled-race-harness

Conventions for every task: work in the mission worktree; `export TMPDIR=/home/shared/tmp`;
never build in the main checkout. Migration tasks MUST NOT change test behavior (same
requests, same assertions, same timing; no sleeps). The blessed spec deltas live in
`specs/http-test-harness/spec.md` and `specs/no-raw-http-lint/spec.md`.

## Phase 1: Harness

### camel-component-api: harness core

#### Task 1.1: Harness module with framing, recording, routing

**Files:**
- `crates/components/camel-component-api/src/test_support/http.rs` (new)
- `crates/components/camel-component-api/src/test_support.rs` (modified — add `pub mod http;`)
- `crates/components/camel-component-api/src/test_support/http_tests.rs` (new — unit tests, `#[cfg(test)]`)

**Steps:**
1. Create `test_support/http.rs` gated by the parent module's existing
   `#[cfg(any(test, feature = "test-support"))]` (no new gate, no new deps: tokio,
   tokio-util, std only — the crate does NOT depend on `bytes`, use `Vec<u8>` for all
   byte buffers). At the top of `test_support/http.rs` declare
   `#[path = "http_tests.rs"] mod http_tests;` (the `#[path]` keeps the test file at
   the sibling location `src/test_support/http_tests.rs`, since a bare `mod` would
   resolve to `test_support/http/http_tests.rs`) so the unit-test file actually
   compiles. Define:
   `pub struct HttpTestServer` (handle:
   `local_addr() -> SocketAddr`, `port() -> u16`, `recorded() -> Vec<RecordedRequest>`,
   `pub async fn shutdown(self)`), `pub struct RecordedRequest` (fields: `method:
   String`, `target: String` unnormalized incl. query, `headers: Vec<(String, String)>`
   lowercased names, `body: Vec<u8>`, `wire_head: Vec<u8>` exact request
   line + head bytes), `pub struct Response` with constructors `Response::ok(body)`,
   `Response::with_status(u16, reason: &str, body)`, `Response::raw(Vec<u8>)`.
2. Implement the connection task: read request line, headers until `\r\n\r\n`, then
   read exactly `Content-Length` body bytes; record `RecordedRequest` (wire_head
   captured before parsing); then write the response with injected
   `Connection: close` header and correct `Content-Length`, then close the socket.
   For `Response::raw`, write the bytes verbatim but inject `Connection: close` when
   the raw bytes contain no `connection:` header line.
3. Implement `HttpTestServer::start(responder)` where responder is
   `pub enum Responder { Fixed(Response), Route(Arc<dyn Fn(&RecordedRequest) -> Response + Send + Sync>) }`
   with an associated constructor `Responder::route(f)` (no free `route` fn).
   Bind `127.0.0.1:0`, spawn the accept loop with a
   `tokio_util::sync::CancellationToken` (tokio-util is already a
   camel-component-api dependency).
4. Write the unit tests listed below into `test_support/http_tests.rs`, each driving a
   real `tokio::net::TcpStream` client against a started harness server.

**Tests:** (in `test_support/http_tests.rs`, all `#[tokio::test]`)
- `roundtrip_answers_and_closes`: harness with `Responder::Fixed(Response::ok("ok"))`
  → client connects, sends `GET /x HTTP/1.1\r\nHost: h\r\n\r\n`, reads to EOF →
  response contains status `200 OK`, `Content-Length: 2`, header line
  `Connection: close`, body `ok`, and EOF follows the response.
- `post_body_drained_before_reply`: client writes head declaring
  `Content-Length: 5`, then body `hello` in a SECOND write after reading a
  channel-ready signal the test controls → response arrives only after the second
  write; `recorded()[0].body` == `hello`.
- `segmented_body_recorded_fully`: same as above asserting
  `recorded()[0].wire_head` is byte-identical to the head bytes the client sent and
  `recorded()[0].target` == the unnormalized path incl. query (`/x?a=1&b=2`).
- `path_routing_answers_per_path`: `Responder::route` mapping `/fail` → 500 and
  other → 200 → client requests `/fail` then `/ok` on separate connections →
  statuses 500 then 200, `recorded()` has both entries in order.
- `raw_response_gets_close_injected`: `Response::raw(b"HTTP/1.1 204 No
  Content\r\nContent-Length: 0\r\n\r\n")` → client reads a response carrying an
  injected `Connection: close` header line before the blank line.
- command: `cargo test -p camel-component-api --features test-support --lib test_support::http`
- expected: FAIL to compile before Task 1.1 (module absent); all listed tests PASS
  after.

**Acceptance:**
- The five tests above pass with a NONZERO executed count and every listed test name
  present in the `cargo test` output (the `#[path]`-wired `http_tests` module is what
  makes them compile — a filtered run that compiles zero tests is not evidence).
- `cargo clippy -p camel-component-api --features test-support --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.
- No NEWLY added dependency in camel-component-api (the crate already carries rustls
  and rcgen behind features — that is pre-existing; grep Cargo.toml against the base
  revision to confirm the dependency set is unchanged, and no hyper).

- [x] 1.1

#### Task 1.2: Keep-alive, staged/chunked modes, 400 outcomes, teardown

**Files:**
- `crates/components/camel-component-api/src/test_support/http.rs` (modified)
- `crates/components/camel-component-api/src/test_support/http_tests.rs` (modified)

**Steps:**
1. Add `HttpTestServer::builder()` returning a `ConnectionConfig` builder (new pub
   struct: keep-alive flag + `reason: &'static str`, surfaced in its `Debug`) with
   `.keep_alive(reason: &'static str)` and `.start(responder)`. In keep-alive mode one
   connection serves sequential requests: after each response (still `Connection: close`-free —
   keep-alive responses carry `Connection: keep-alive`), loop to parse the next
   request on the same socket; pipelined bytes after a completed request are parsed
   as the next request. Default (non-keep-alive) responses inject `Connection: close`.
 2. Add framing modes to `Response`: `Response::chunked(Vec<Vec<u8>>)` emitting legal
    `Transfer-Encoding: chunked` framing (chunk-size lines, terminal `0\r\n\r\n`, no
    Content-Length, `Connection: close` injected) and
    `Response::staged(ResponseHeaders, tokio::sync::mpsc::Receiver<Vec<u8>>)` writing the
    header block immediately, then each received chunk as a chunk continuation
    (channel-signaled, never sleeps inside the harness), terminal chunk at channel
    close. Define
    `pub struct ResponseHeaders` here: fields `status: u16`, `reason: String`,
    `headers: Vec<(String, String)>`, with constructor
    `ResponseHeaders::new(status, reason, headers)` and a
    `fn encode(&self, connection_header: &str) -> Vec<u8>` producing the status line +
    header block + terminating blank line, where the caller passes the connection
    header for the mode (`"Connection: close"` by default, `"Connection: keep-alive"`
    in keep-alive mode).
 3. Implement deterministic error outcomes: unparseable head, invalid Content-Length,
    premature EOF mid-body, unsupported `Transfer-Encoding` → write `HTTP/1.1 400 Bad
    Request` with `Connection: close`, close socket. Delivery to a fully-disconnected
    peer is best-effort (write error swallowed, socket still closed, no hang).
 4. Implement teardown: `Drop` for `HttpTestServer` cancels the
    `tokio_util::sync::CancellationToken` and detaches — NEVER blocks/joins.
    `shutdown(self)` awaits listener end + bounded (`Duration::from_secs(5)`) join of
    connection tasks, aborting stragglers.
 5. Add the unit tests below.

**Tests:** (all `#[tokio::test]`, including one `#[tokio::test(flavor =
"current_thread")]`)
- `keep_alive_serves_two_on_one_connection`: builder with `.keep_alive("test
  asserts reuse")`, Fixed responder → client sends GET, reads response (contains
  `Connection: keep-alive`, no close), sends second GET on the same stream, reads
  second response → both 200, `recorded()` has two entries.
- `staged_headers_then_signaled_chunks`: `Response::staged` with an mpsc channel →
  client reads header block before sender sends chunk one; test sends chunk `a`,
  signals, asserts bytes arrive in order; dropping the sender ends the message.
- `chunked_framing_is_legal`: `Response::chunked(vec!["a", "bc"])` → client reads to
  EOF and assembles body `abc`; response has `Transfer-Encoding: chunked`, no
  `Content-Length`.
- `malformed_head_gets_400`: client writes `NOT-A-REQUEST\r\n\r\n` → reads 400 with
  `Connection: close`, then EOF.
- `truncated_body_gets_400`: client declares `Content-Length: 10`, sends 3 bytes,
  shuts down its WRITE half only → reads 400 (read half retained), then EOF.
- `unsupported_transfer_encoding_gets_400`: head declares `Transfer-Encoding: gzip`
  → reads 400.
- `drop_is_non_blocking_and_releases_listener`: current_thread runtime; start
  server, connect a client, send half a head, drop the handle → drop returns (no
  await); a fresh `TcpStream::connect` to the port eventually fails; then a second
  current-thread test `shutdown_confirms_reclamation` with an idle keep-alive
  connection: `handle.shutdown().await` completes and the listener port is released.
- command: `cargo test -p camel-component-api --features test-support --lib test_support::http`
- expected: all PASS after this task (1.1 tests keep passing).

**Acceptance:**
- All Task 1.1 + 1.2 tests pass.
- `cargo clippy -p camel-component-api --features test-support --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.

- [x] 1.2

#### Task 1.3: serve_connection for TLS-owning tests

**Files:**
- `crates/components/camel-component-api/src/test_support/http.rs` (modified)
- `crates/components/camel-component-api/src/test_support/http_tests.rs` (modified)

**Steps:**
1. Refactor the per-connection serving logic into
   `pub async fn serve_connection<IO>(io: IO, responder: Responder, cfg: ConnectionConfig)`
   where `IO: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static`.
   `ConnectionConfig` is the SAME type introduced in Task 1.2 — one struct serves
   both roles: its builder methods (`keep_alive`) consume and return `Self`, and the
   finished value is passed to `serve_connection` by reference/value. The accept loop
   calls it with the raw TcpStream; behavior is unchanged.
2. Unit test drives `serve_connection` over a `tokio::io::duplex` pair — the same
   path a TLS test uses after wrapping its own `TlsAcceptor` stream (no rustls in
   this crate; the duplex pair stands in for the generic IO bound).

**Tests:**
- `serve_connection_over_duplex`: harness not used; spawn `serve_connection` on one
  duplex half with Fixed responder → client writes a GET on the other half, reads
  the close-mandated response, asserts status + `Connection: close`.
- command: `cargo test -p camel-component-api --features test-support --lib test_support::http`
- expected: PASS after this task.

**Acceptance:**
- All harness tests (1.1–1.3) pass.
- `cargo clippy -p camel-component-api --features test-support --all-targets -- -D warnings` exits 0.
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-component-api --no-deps --features test-support` exits 0.

- [x] 1.3

## Phase 2: Lint

### xtask: no-raw-http-test-server

#### Task 2.1: Lint scanner with fixtures

**Files:**
- `scripts/xtask/src/lint_no_raw_http_test_server.rs` (new)
- `scripts/xtask/src/main.rs` (modified — `mod lint_no_raw_http_test_server;` + `Commands::LintNoRawHttpTestServer` clap variant and match arm wired in THIS task, so nothing is dead code)

**Steps:**
1. Mirror `lint_no_free_port.rs` structure: `Finding { file, line }`,
   `ScanError::{Parse, Read}`, `MEMBER_ROOTS: [&str; 5] = ["crates", "scripts",
   "examples", "benchmarks", "fuzz"]`, `pub const RATCHET_FILE: &str =
   "ratchet-raw-http-test-server.max"`, fail-closed `read_ratchet_max`, `run(workspace_root)`
   with the same enforcement output shape (findings list, marker report, ratchet
   verdict).
2. Wire the clap command `lint-no-raw-http-test-server` in `Commands` + the `main()`
   match arm in the SAME task, reusing the ratchet enforcement output of
   `LintNoFreePort` (name the ratchet file in over/under messages; nonzero exit when
   findings exceed ceiling or ratchet missing).
2. Scanner: syn `Visit` over every `.rs` file; for each `LitStr`/`LitByteStr` AND each
   literal found inside macro token streams (`token_stream` recursion into
   `Macro`/`Attribute` tokens), evaluate: text starts with `HTTP/1.0 ` or
   `HTTP/1.1 ` (exact prefix) AND the status token after the prefix begins with a
   digit or `{` AND the literal does not carry a structurally valid
   `Connection: close` header (a line whose name token is `connection`
   case-insensitive, value contains `close`, positioned before the first `\r\n\r\n`;
   for literals without any `\r\n\r\n` the whole literal is header-block scope).
3. Exclusions: (a) the literal is a DIRECT argument of an `ExprMethodCall` whose
   `.method` identifier is one of `starts_with`, `ends_with`, `contains` (match the
   method-call node's method ident — NOT a textual "receiver ends with" heuristic,
   which targets the wrong AST field); (b) literal is an
   argument (direct or nested inside struct/tuple/vec/constructor-call arguments) of
   a call whose final path segment is one of `start`, `start_with`, `start_permissive`,
   `raw`, `chunked`, `staged`, `ok`, `with_status` on a type path containing
   `test_support::http` OR whose expression type is inferred from
   `test_support::http` re-exports — implement as: the call chain's receiver or
   function path contains `Response` or `HttpTestServer` (path-segment match), which
   is the import-agnostic form; (c) a `// allow-raw-http: <non-empty reason>` comment
   on the line immediately above suppresses the site and records it for the marker
   report.
4. Test scope: `crate::scan_state::TestScopeTracker` (existing module
   `scripts/xtask/src/scan_state.rs`, used by `lint_cancel_tokens.rs`): construct
   `TestScopeTracker::new()` and gate each line with `line_in_test_scope(&mut self,
   trimmed)` — this covers `#[cfg(test)]` modules, `#[test]`/`#[tokio::test]` fns
   (incl. attribute arguments), plus the file-level rules: `tests/**` dirs and
   `*_test.rs`/`*_tests.rs` file names are test scope wholesale. Findings outside
   test scope are ignored.
5. Unit tests as a fixture module inside the lint file (pattern of
   `lint_no_free_port.rs` `mod tests`), covering every scenario in
   `specs/no-raw-http-lint/spec.md`: plain-no-close, valid-close, close-in-body
   negative, format-macro, `{status_line}` placeholder, `{} {}` placeholder,
   starts_with exclusion, request-literal negative, harness-constructor exemption,
   marker suppress, empty-reason marker, marker report listing, ratchet
   exceeded/at-baseline/missing (fail-closed).

**Tests:** (fixture-based unit tests in the lint's `mod tests`)
- `plain_response_literal_without_close_is_reported`
- `literal_with_valid_close_header_is_clean`
- `close_token_in_body_does_not_exempt`
- `format_macro_literal_is_reported`
- `dynamic_whole_status_placeholder_is_reported`
- `dynamic_status_and_reason_placeholders_are_reported`
- `starts_with_argument_is_excluded`
- `ends_with_argument_is_excluded`
- `contains_argument_is_excluded`
- `assertion_macro_nesting_is_excluded` (literal nested inside
  `assert!(head.starts_with(..))` — the exclusion must see through macro nesting)
- `request_literal_is_not_reported`
- `harness_constructor_argument_is_exempt`
- `marker_suppresses_site` / `empty_reason_marker_does_not_suppress` /
  `marker_report_lists_locations`
- `ratchet_exceeded_fails` / `ratchet_at_baseline_passes` / `missing_ratchet_fails_closed`
- command: `cargo test -p xtask lint_no_raw_http_test_server`
- expected: FAIL to compile before; all PASS after.

**Acceptance:**
- All fixture tests pass.
- `cargo clippy -p xtask --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.

- [x] 2.1

#### Task 2.2: Seeded ratchet, CI + AGENTS wiring

**Files:**
- `scripts/xtask/ratchet-raw-http-test-server.max` (new — seeded at the measured baseline)
- `.github/workflows/ci.yml` (modified — gate after `lint-no-free-port`)
- `AGENTS.md` (modified — row in `## QUALITY GATES`)

**Steps:**
1. Bootstrap the ratchet WITHOUT weakening enforcement: the command is
   fail-closed (missing ratchet = error), so first write
   `ratchet-raw-http-test-server.max` with a provisional ceiling large enough to run
   (`# ratchet\n999`), run `cargo xtask lint-no-raw-http-test-server` to measure the
   exact unadjudicated-finding count, then rewrite the file with that exact count as
   the seed. Both writes land in the SAME commit; the committed state is the exact
   measured baseline (expected shape: `# ratchet\n<count>`); record the count in the
   task report.
2. Add the CI step `- name: lint-no-raw-http-test-server` /
   `run: cargo xtask lint-no-raw-http-test-server` immediately after the
   `lint-no-free-port` step in `.github/workflows/ci.yml`.
3. Add `- name: lint-no-raw-http-test-server` /
   `run: cargo xtask lint-no-raw-http-test-server` to the AGENTS.md `## QUALITY
   GATES` block between `lint-no-free-port` and `lint-unbounded-wait`.

**Tests:**
- `baseline_seed_matches_measured_count`: run
  `cargo xtask lint-no-raw-http-test-server` → exit 0 with findings == seeded count;
  temporarily set the ratchet to count-1 → nonzero exit naming the file; restore.
- command: `cargo test -p xtask lint_no_raw_http_test_server && cargo xtask lint-no-raw-http-test-server`
- expected: lint fixture tests pass; gate exits 0 at the seeded baseline.

**Acceptance:**
- `cargo xtask lint-no-raw-http-test-server` exits 0 with the seeded baseline; the
  measured baseline count is reported.
- CI yml and AGENTS.md contain the new gate (grep-verified).
- `cargo clippy -p xtask --all-targets -- -D warnings` and `cargo fmt --check` exit 0.

- [x] 2.2

## Phase 3: camel-component-http migration

### camel-component-http: ssrf + client_cache

#### Task 3.1: ssrf responders onto the harness

**Files:**
- `crates/components/camel-http/src/ssrf.rs` (modified — test module responders at ~916-960, ~1180-1195)
- `crates/components/camel-http/Cargo.toml` (modified only if `camel-component-api` test-support dev-dep needs adding; verify it is already present first)

**Steps:**
1. Replace `spawn_302_responder` (format! 302 literal, ~928) with a harness server:
   `Responder::route` answering `Response::with_status(302, "Found", Vec::new())`
   with a `Location` header — pooling is under test here (rc-u3aw family): the REAL
   listener stays (the harness IS a real listener); assertions unchanged.
2. Replace `spawn_200_responder` (~952) with `HttpTestServer::start(Responder::Fixed(
   Response::ok("ok")))`; keep the old fn signature/return (port or addr) so callers
   are untouched beyond the body.
3. Replace `spawn_200_responder_capturing_headers` (~1188) with the harness and map
   its header-capture to `server.recorded()` (assertion sites keep asserting the same
   header facts; adapt the capture accessor, not the assertions).
4. Preserve every call site's observable behavior: same requests, same assertions,
   same timeouts. No sleeps. The `spawn_*` helpers keep their signatures (including
   returning `JoinHandle`s): the `HttpTestServer` handle is moved INTO the spawned
   task (Drop is non-blocking detach, so ownership inside the task is safe), and test
   teardown keeps joining the JoinHandle exactly as before — no caller-visible
   lifetime change.

**Tests:**
- Existing ssrf tests keep passing unchanged (names unchanged): run
  `cargo test -p camel-component-http --lib ssrf`
- expected: PASS before (raw servers) and PASS after (harness); zero new sleeps;
  lint findings for ssrf.rs drop from 3 to 0 (`cargo xtask
  lint-no-raw-http-test-server` count reflects it).

**Acceptance:**
- `cargo test -p camel-component-http --lib ssrf` green.
- `cargo xtask lint-no-raw-http-test-server` exits 0 with the ratchet LOWERED by the
  resolved count (3) in `ratchet-raw-http-test-server.max`.
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0;
  `cargo fmt --check` exits 0.

- [x] 3.1

#### Task 3.2: client_cache recorder onto the harness

**Files:**
- `crates/components/camel-http/src/client_cache.rs` (modified — `spawn_recorder` ~230-260)

**Steps:**
1. `spawn_recorder` receives an ALREADY-BOUND listener plus an `AtomicBool` hit flag
   and must keep both (callers own the destination). Do NOT call
   `HttpTestServer::start` (it would bind its own socket). Instead keep the accept
   loop on the supplied listener and hand each accepted stream to the harness's
   `serve_connection` with `Responder::Fixed(Response::ok("ok"))`; keep the existing
   `AtomicBool` observation untouched (tests assert on it, not on harness
   recording).
2. Keep the fn signature and call sites unchanged.

**Tests:**
- Existing client_cache tests keep passing: `cargo test -p camel-component-http --lib client_cache`
- expected: PASS before and after.

**Acceptance:**
- `cargo test -p camel-component-http --lib client_cache` green.
- Lint ratchet lowered by the resolved count (1).
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0;
  `cargo fmt --check` exits 0.

- [x] 3.2

### camel-component-http: lib_tests helpers

#### Task 3.3: Migrate lib_tests helper servers

**Files:**
- `crates/components/camel-http/src/lib_tests.rs` (modified — helper fns: `start_test_server` ~1105-1143 (method-dependent JSON + the `HTTP/1.1 {} {}` dynamic-status literal), `start_status_server` (the dynamic-status helper owning the ~1143 shape — verify exact fn name in-tree), `start_request_capturing_server` ~1179, `start_redirect_server` ~1773, `start_capture_server` ~1919, `start_test_server_with_header_capture` ~5468, `start_host_capturing_destination` ~9074)

**Steps:**
1. For each helper fn: keep the SIGNATURE and every call site; replace the raw
   accept/read/write body with the harness:
   - `start_test_server`: `Responder::route` preserving its EXACT method-dependent
     behavior (JSON body + `Content-Type`/`X-Custom` headers as the current literal
     at ~1105-1111 builds them) — NOT a fixed-ok replacement.
   - `start_status_server` (owner of the `HTTP/1.1 {} {}` literal ~1143): migrate to
     `Responder::route` returning `Response::with_status(status, reason, error_body)`
     with the same status, reason, and error body the current construction produces
     per request.
   - `start_request_capturing_server` / `start_test_server_with_header_capture` /
     `start_capture_server` / `start_host_capturing_destination`: map their custom
     capture channels to `server.recorded()` (bridge into the existing channel types
     the tests consume; assertions unchanged).
   - `start_redirect_server`: `Responder::route` with the same 302/Location mapping.
2. If a helper returns a join handle the tests drop, return the `HttpTestServer`
   handle instead and adapt ONLY the call sites' variable usage (no assertion
   changes).
3. Re-run the lint after the batch; the lib_tests finding count drops accordingly
   (do not touch the inline sites of Task 3.4 yet).

**Tests:**
- Existing camel-http lib tests keep passing: `cargo test -p camel-component-http --lib`
- expected: PASS before and after; names and assertions unchanged.

**Acceptance:**
- `cargo test -p camel-component-http --lib` green.
- Lint count reduced by the helpers' findings: run the lint BEFORE and AFTER this
  task, record both counts in the task report, and LOWER
  `ratchet-raw-http-test-server.max` by the resolved difference (expected ≥ 6 of the
  lib_tests sites resolved).
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0;
  `cargo fmt --check` exits 0.

- [x] 3.3

#### Task 3.4: Migrate lib_tests inline responders + TLS site; crate ratchet to zero

**Files:**
- `crates/components/camel-http/src/lib_tests.rs` (modified — inline responders in `test_producer_with_token_provider` ~1055, `test_redirect_to_private_ip_blocked_when_ssrf_guard_active` ~2520, `start_slow_server` ~2709, the redirect status-map block ~1976-1979, `spawn_multi_accept_200` ~9457-9474, `spawn_tls_multi_accept_200` ~9520)
- `crates/components/camel-http/src/tls_harness.rs` (modified only if the TLS site migrates through it)
- `scripts/xtask/ratchet-raw-http-test-server.max` (modified — lower by resolved count)

**Steps:**
1. `test_producer_with_token_provider` (~1055) and
   `test_redirect_to_private_ip_blocked_when_ssrf_guard_active` (~2520): replace the
   inline raw servers with harness servers carrying the same status/body/Location
   semantics.
2. Redirect status-map block (~1976-1979: `"/hop307" => ("HTTP/1.1 307 Temporary
   Redirect", …)` etc.): replace the whole raw server with
   `Responder::route` mapping the same paths to
   `Response::with_status(307/308/200/404, …)` with the same Location headers.
3. `start_slow_server` (~2709, immediate headers + delayed chunked body): migrate to
   `Response::staged(ResponseHeaders, mpsc::Receiver<Vec<u8>>)` where the TEST-side
   sender task reproduces the exact existing timing: headers encode and flush
   immediately; the sender applies the SAME delay the current `start_slow_server`
   takes as its argument via `tokio::time::sleep(delay)` — moving the existing
   `// allow-test-sleep` marker verbatim onto that sleep — then sends the chunk.
   This preserves the caller's 500 ms-delay vs 100 ms read-timeout contract
   byte-for-byte (lib_tests.rs:2712, 2728-2735): same requests, same assertions,
   same timeout behavior, no NEW sleeps.
4. `spawn_multi_accept_200` (~9457-9474): replace with the harness (multi-accept is
   the harness accept loop's default).
5. `spawn_tls_multi_accept_200` (~9520): keep its own TLS acceptor; hand each
   accepted TLS stream to `serve_connection` with the same Fixed 200-ok responder
   (via `Response::ok`), preserving its accept timeout and TLS config.
6. Run `cargo xtask lint-no-raw-http-test-server`; camel-component-http must
   contribute ZERO unadjudicated findings; lower the ratchet by the total resolved
   in this task.

**Tests:**
- Full crate suite green: `cargo test -p camel-component-http --lib`
- expected: PASS before and after; names and assertions unchanged; no sleeps
  introduced (`cargo xtask lint-test-sleep` still exits 0).

**Acceptance:**
- `cargo test -p camel-component-http --lib` green.
- Lint exits 0 with ratchet lowered by the exact count resolved in this task;
  `grep -c "HTTP/1.1" crates/components/camel-http/src/lib_tests.rs` retains only
  assertion-side or exempted occurrences (verified by the lint, not the grep).
- `cargo clippy -p camel-component-http --all-targets -- -D warnings` exits 0;
  `cargo fmt --check` exits 0.

- [x] 3.4

<!-- Deviation note: headered responses (302/307/308 + Location) are delivered
via `Response::raw` byte literals because `Response` has no arbitrary-header
constructor. Raw is lint-exempt, so the ratchet is unaffected. A header-capable
`Response` constructor is a recorded follow-up so future migrations can avoid raw
literals. -->

## Phase 4: camel-test + camel-integration-test migration

### camel-test: http_test suite

#### Task 4.1: http_test.rs sites onto the harness

**Files:**
- `crates/camel-test/tests/http_test.rs` (modified — `http_producer_allows_localhost_when_configured` ~610, `http_custom_ok_status_code_range` ~659, `http_producer_bridge_endpoint_skips_auth` ~1054, `http_producer_connection_close_header` ~1120, `http_producer_skip_request_headers` ~1185)
- `crates/camel-test/Cargo.toml` (modified only if the `test-support` feature wiring for dev use needs enabling; camel-test already depends on camel-component-api with test-support — verify first, do not duplicate)

**Steps:**
1. Replace each of the five raw server bodies with harness servers preserving the
   exact per-test semantics:
   - `http_producer_allows_localhost_when_configured`: Fixed
     `Response::ok(r#"{"ok":true}"#)` with `Content-Type: application/json`.
   - `http_custom_ok_status_code_range`: Fixed `Response::with_status(204, "No
     Content", "")`.
   - `http_producer_bridge_endpoint_skips_auth`, `http_producer_skip_request_headers`:
     Fixed responder + `recorded()` where the test asserts on received headers.
   - `http_producer_connection_close_header`: Fixed responder; the test asserts the
     REQUEST carries `Connection: close` — assert via `recorded()[0]` headers.
2. Keep test fn names, requests, and assertions byte-identical in intent; only the
   server construction changes. The suite is
   `#![cfg(feature = "integration-tests")]` — run it WITH the feature.

**Tests:**
- `cargo test -p camel-test --features integration-tests --test http_test`
- expected: PASS before and after; the executed-test count is NONZERO (a
  zero-compiled run is not evidence — the cfg gate is at the top of the file).

**Acceptance:**
- Suite green with nonzero executed count (record the count).
- Lint count reduced by 5; ratchet lowered accordingly.
- `cargo clippy -p camel-test --features integration-tests --tests -- -D warnings` exits 0;
  `cargo fmt --check` exits 0.

- [x] 4.1

### camel-integration-test: partner far side

#### Task 4.2: plain_string far side onto the harness

**Files:**
- `crates/camel-integration-test/src/http_partner_test.rs` (modified — `plain_string_send_dials_literal_without_partner` ~488-540 and `head_is_complete` helper if now unused)
- `crates/camel-integration-test/Cargo.toml` (modified only if a `camel-component-api` test-support dev-dep needs adding; verify it is already present first — do not duplicate)

**Steps:**
1. Replace the raw one-shot far side with the harness started WITHOUT keep-alive
   (terminal close) using a Fixed `Response::ok("ok")` responder — the far side MUST
   stay a real listener (the test asserts literal dialing of the interpolated
   address; the harness is that real listener).
2. Replace the `head` capture with `server.recorded()[0].wire_head`; the existing
   assertion on the recorded request line keeps its exact expected bytes.
3. Remove `head_is_complete` if it becomes dead code (the lint
   `lint-unwrap`/clippy will flag dead code); keep it if still referenced.

**Tests:**
- `cargo test -p camel-integration-test --features http --lib http_partner`
- expected: PASS before and after; the `plain_string_send_dials_literal_without_partner`
  assertion outcomes unchanged.

**Acceptance:**
- Suite green.
- Lint count reduced by 1; ratchet now equals the 3 benchmark residuals ONLY.
- `cargo clippy -p camel-integration-test --features http --all-targets -- -D warnings` exits 0;
  `cargo fmt --check` exits 0.

- [x] 4.2

## Phase 5: Benchmarks + ratchet 0 + full gates

### loadgen adjudication + gate sweep

#### Task 5.1: Mark benchmark sites, ratchet to 0, full local gate pass

**Files:**
- `benchmarks/harness/loadgen/src/cli_runtime.rs` (modified — two markers above the `#[cfg(test)]` literals ~821 and ~886)
- `benchmarks/harness/loadgen/tests/cli_integration.rs` (modified — one marker above `CANNED_200` ~380)
- `scripts/xtask/ratchet-raw-http-test-server.max` (modified — to 0)

**Steps:**
1. Add `// allow-raw-http: benchmark loadgen CLI test; standalone std server; no
   product test surface; behavior preserved (mission 402)` on the line immediately
   above each of the three literals (`CANNED_200` const keeps its serving semantics —
   marker goes above the const).
2. Set the ratchet to 0. Run
   `cargo xtask lint-no-raw-http-test-server` → exit 0 with the marker report listing
   exactly the 3 benchmark locations.
3. Full local gate sweep, recording each exit code:
   `cargo fmt --check --all`;
   `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings`;
   `cargo clippy -p camel-component-kafka --all-targets -- -D warnings`;
   `cargo clippy -p camel-cli -- -D warnings`;
   `cargo clippy -p camel-cli --no-default-features --features flavor-regular,exec --all-targets -- -D warnings`;
   `cargo xtask lint-unwrap`; `cargo xtask lint-secrets`; `cargo xtask lint-single-source`;
   `cargo xtask lint-non-exhaustive`; `cargo xtask lint-log-levels`;
   `cargo xtask lint-log-redaction`; `cargo xtask lint-cancel-tokens`;
   `cargo xtask lint-test-sleep`; `cargo xtask lint-no-free-port`;
   `cargo xtask lint-unbounded-wait`; `cargo xtask lint-ignore`;
   `cargo xtask lint-publish-cycles`; `cargo xtask lint-publish-registration`;
   `cargo xtask lint-component-deps`; `cargo xtask lint-bridge-versions`;
   `cargo xtask lint-gate-forwarding`; `cargo xtask lint-context-citations`;
   `cargo xtask lint-metric-labels`; `cargo xtask schema --check`;
   `cargo audit`;
   `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-component-api -p camel-component-http -p camel-test -p camel-integration-test --no-deps` (touched-crate set; workspace-wide doc gate runs at landing);
   `cargo build --workspace`; `cargo test --workspace --lib`;
   `cargo test -p camel-core --test hexagonal_architecture_boundaries_test`.
   SKIP `lint-commits` (remote fetch; CI owns it). Integration suites needing Docker
   are CI's responsibility — note them as integration-verification-deferred-to-CI.

**Tests:**
- `benchmark_markers_are_the_only_residuals`: lint output lists exactly 3 marker
  locations, all under `benchmarks/`, marker count 3, findings 0, ratchet 0.
- command: `cargo xtask lint-no-raw-http-test-server`
- expected: exit 0.

**Acceptance:**
- Ratchet file contains 0; lint exits 0 with exactly the 3 benchmark markers.
- Every gate above recorded with its exit code; any pre-existing failure is verified
  in a SEPARATE baseline worktree created at the mission base revision
  (`git worktree add /home/shared/tmp/402-baseline 036f9b345` — NEVER build or test
  in the main checkout), filed as a bd follow-up from the repo root, and reported as
  "gate X skipped: pre-existing failure <bd-id>". Park-for-master disposition is
  unchanged: no merge, no version bump.
- `cargo test --workspace --lib` green; the touched-suite table (site → adoption →
  residual marker) is delivered in the task report for the final mission report.

- [x] 5.1
