# Design: r5batteries

## Approach

Two slices plus one enabling carve-out, all inside `camel-cli` and
`camel-component-grpc`:

1. **protoFile joins the R2 asset matrix.** One match arm in
   `tls_uri_param_class` (`crates/camel-cli/src/compile/policy.rs`):
   `"protoFile" if grpc => Some("proto file")`. `collect_uri` already
   walks every query pair of every endpoint URI and pushes a `checked`
   `AssetRef` with `SubstitutionContext::Uri`; `checked` already
   rejects `${...}` and absolute paths; `sources` already confines and
   embeds by declared path; `materialize` already writes exactly the
   substitution-targeted bytes into the per-boot 0700 TMPDIR directory
   and `runtime` already rewrites the recorded byte spans before the
   document boots. The class is non-secret — no `--embed-secrets`
   opt-in.

2. **Materialization carve-out in the grpc URI parse.**
   `parse_grpc_uri` (`camel-component-grpc/src/config.rs:673-680`)
   rejects any `protoFile` that `starts_with('/')` or contains `..` —
   the substituted value is an absolute materialized path, so without a
   change the sealed artifact can never boot. The rule becomes: the
   `..` rejection stays unconditional (the materializer never emits
   `..`, so nothing legitimate is lost); an absolute path without `..`
   is accepted if and only if its canonicalized form stays inside the
   canonicalized OS temp directory — the root under which
   `materialize.rs` creates the per-boot directory. `/etc/passwd`
   stays rejected; `/tmp/<per-boot>/protos/x.proto` passes;
   `/tmp/../etc/passwd` fails the unconditional `..` guard. Component
   tests pin all three. TLS `*Path` parameters undergo no such parse
   validation (which is why substitution already works for them); the
   carve-out brings `protoFile` to the same effective posture while
   keeping the traversal guard.

3. **Two batteries in `compiled_artifact_test.rs`** (73 → 75 tests),
   reusing the R5 harness seams verbatim: `child_guard`,
   `compile_and_deploy_listener` (generalized to take a doc file name),
   `spawn_child`/`spawn_drained`, `wait_for_marker`,
   `wait_for_marker_tight`, `send_signal`, `wait_exit_code`,
   `with_bind_race_retry`, `free_port`, `--report`/`--manifest`
   assertions.

   - **gRPC serve** (`route_server_serves_grpc_listener_until_sigterm`):
     source dir carries `doc.yaml` + `protos/helloworld.proto` (the
     14-line helloworld.proto from `examples/grpc-example`); the route
     is `from: grpc://127.0.0.1:{port}/helloworld.Greeter/SayHello?protoFile=protos/helloworld.proto&transport=plaintext`
     — `transport` is explicit because the parse fails closed on
     omission (ADR-0033). After `context started` and the post-bind
     consumer marker (`grpc consumer started, waiting for requests` —
     logged only after `GrpcServerRegistry::get_or_spawn` binds), a
     raw-TCP HTTP/2 probe sends the client preface + empty SETTINGS
     frame and asserts a server SETTINGS frame (type `0x04`) comes
     back — wire-level proof of a serving h2 listener with zero new
     dev-deps. Then liveness (`try_wait` None), SIGTERM, exit 0
     (30 s bound), exact report
     `{"kind":"route","status":"completed","error":null}`, and a
     `--manifest` run showing `artifact_kind` `server` with `grpc`
     among the components.
   - **WS drain** (`route_server_drains_inflight_ws_exchange`): route
     `from: ws://127.0.0.1:{port}/echo` with steps `log: "slow-enter"`,
     `delay: 3000`, `set_body: "slow-pong"`, then `to:
     ws://127.0.0.1:{port}/echo` — the producer side runs in
     server-send mode and echoes back on the sender's connection key.
     A `std::net::TcpStream` helper performs the HTTP/1.1 upgrade
     (101), sends a masked text frame, and reads server frames. After
     the `slow-enter` marker proves the exchange is inside the delay
     step, SIGTERM lands ≈0 s into the 3 s delay (10 s default drain
     budget); the echoed `slow-pong` frame must still arrive, then exit
     0, then the exact completed report.

## Affected crates

- `camel-cli`: `src/compile/policy.rs` (one match arm + doc comment),
  `tests/compiled_artifact_test.rs` (two batteries + two probe helpers
  + doc-name generalization), `tests/compile_asset_test.rs` (protoFile
  collect/reject coverage, mirroring `compile_collects_grpc_tls_uri_params`).
- `camel-component-grpc`: `src/config.rs` (protoFile parse carve-out:
  absolute paths accepted iff canonically confined under the OS temp
  directory) + unit tests for the accepted/rejected shapes.
- `camel-ws`: `src/lib.rs` (discovered during Task 2.2 — the WS drain
  battery exposed that `WsConsumer::stop` broadcast `Close(1001)` and
  tore down connections in milliseconds, stranding any in-flight
  exchange; HTTP drains only because its `stop` is a no-op. Minimal
  conformance patch: a bounded 5 s settle-wait on the rc-nftni
  in-flight gauge at the top of `stop`, failing open to the prior
  teardown on expiry; quiet contexts proceed instantly. The blessed
  scenario "WebSocket consumer drains an in-flight exchange" already
  mandates this behavior — the product was non-conformant, the battery
  enforced the spec. The original "camel-ws: none" forecast below was
  an impact-estimation error, corrected here; the normative delta
  spec text is unchanged.)

## Architecture boundaries

Data/control plane untouched. The compile-side slice stays inside the
pipeline's existing classification seam (`policy.rs`) — no new embed
machinery, no runtime change. The one component edit is the
`camel-component-grpc/src/config.rs` parse carve-out (slice 2), which
widens acceptance only for canonically temp-confined absolute paths.
The batteries pin the already-blessed R5 serving/drain contract
(first-signal graceful, second force) over two more transports; they do
not fork it. Consumer routes declared via `from:` URIs are document
data; the manifest keeps listing them under `components` (listener
declarations remain the `rest:`/`mcp:` sections' contract).

## Alternatives considered

- **Ship the proto beside the artifact in the deployment dir** —
  rejected: violates the sealed-artifact posture (source-free
  deployment; the battery deploys into a fresh dir containing only the
  artifact).
- **Drop the parse guard entirely (accept any absolute protoFile)** —
  rejected: weakens the component's traversal posture for no need; the
  canonicalized-under-temp-dir rule accepts only temp-confined
  absolute paths — a strict superset of what the compiler's
  materialization produces, and no wider.
- **Bytes seam inside the grpc component** (read proto bytes from the
  virtual store, no materialization) — rejected: materialize.rs
  explicitly documents that flip as future work for the whole legacy
  path-reader class; doing it for one class would fork the discipline.
- **Real gRPC/WebSocket client dev-deps (tonic/tokio-tungstenite)** —
  rejected: raw `TcpStream` probes (h2 preface + SETTINGS; HTTP/1.1
  upgrade + masked frames) prove the wire contract without growing the
  dev-dependency graph.
- **Test-only change (no compile delta), grpc battery skipped** —
  rejected: bd rc-z332y's acceptance criteria ask for the gRPC battery
  unconditionally; it cannot pass without the embed + carve-out.
