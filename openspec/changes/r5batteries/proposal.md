# Proposal: r5batteries

## Why

R5 (r5routesrv, bd rc-zs7au) landed the long-running route-server battery
with REST/HTTP as the only proven transport: the bless scope-bound
decision (e_gpt finding 4) held that the cli-compile requirement is
transport-agnostic and the drain/signal seams are shared
(`drive_lifecycle`), so no contract fork was possible — but nothing pins
a gRPC or WebSocket listener serving from a sealed artifact. bd rc-z332y
tracks the unpinned surface. Investigation found one real gap, not just
missing tests: `protoFile` on `grpc://` endpoints is absent from the
R2 compile-time asset matrix, so a gRPC consumer in a sealed
(source-free) artifact cannot resolve its proto at boot. The battery
work therefore carries one small compile-pipeline delta plus the two
batteries.

## What Changes

- `camel-cli` compile policy: the `protoFile` URI parameter on `grpc://`
  endpoints joins the R2 asset matrix as a non-secret file class
  ("proto file"), embedded into the store and substituted at boot
  through the existing Uri-context materialization seam — the same
  machinery the gRPC TLS `*Path` family already uses.
- `camel-component-grpc` URI parse: the protoFile path-safety rule
  (relative, no `..`) gains the materialization carve-out — an absolute
  path is accepted if and only if its canonicalized form stays inside
  the OS temp directory (the root under which `materialize.rs` creates
  the per-boot materialization directory). Absolute paths elsewhere
  and `..` traversal stay rejected. Without this the substituted
  absolute path trips the existing `proto path must be relative` guard
  and the sealed artifact can never boot a gRPC consumer.
- Two listener batteries in `compiled_artifact_test.rs`, mirroring
  `route_server_serves_listener_until_sigterm` and
  `route_server_drains_inflight_request`:
  - `route_server_serves_grpc_listener_until_sigterm` — a `grpc://`
    consumer (`transport=plaintext` explicit — the parse fails closed
    on omission, ADR-0033) serving from a sealed artifact, probed at
    the wire level with a raw HTTP/2 preface + SETTINGS handshake (no
    new dev-deps);
  - `route_server_drains_inflight_ws_exchange` — a `ws://` consumer
    whose in-flight delayed exchange completes after SIGTERM inside the
    drain budget (raw HTTP/1.1 upgrade + masked WebSocket frames).
- cli-compile spec deltas: two MODIFIED requirements (asset matrix gains
  `protoFile`; route-server requirement gains the two transport-pinning
  scenarios).

Battery count: 73 → 75.

Excluded: bytes-seam proto resolution inside `camel-component-grpc`
(materialize.rs documents that as a future flip for all legacy path
readers); manifest listener-listing changes for `from:`-URI consumers
(the manifest lists `rest:`/`mcp:` sections as listeners and consumer
schemes under `components` — unchanged); any `camel run` change.

## Acceptance criteria

- A battery mirroring `route_server_serves_listener_until_sigterm`
  exists for a `grpc://` consumer serving from a sealed artifact and
  runs green.
- The equivalent drains-in-flight battery exists for a `ws://` consumer
  and runs green.
- `protoFile` embeds at compile (typed asset entry + substitution-table
  entry, Uri context) and the sealed artifact boots the gRPC consumer
  without any source-side file.
- Absolute-path and `${env:}` `protoFile` values fail closed at compile
  with the standard asset diagnostics; at runtime the grpc parse keeps
  rejecting absolute paths outside the materialization root and `..`
  traversal everywhere.
- No new dev-dependencies; probes are raw `std::net::TcpStream`.

## Risk budget

Containment risk dominates: the batteries spawn real compiled binaries
under the existing harness-child discipline (`child_guard`,
`KillOnDrop`, fixture roots, bind-race retry). Accepted: moderate test
wall-time (one extra compile per flow, mirroring the REST battery
shape). Out of bounds: touching the grpc component's proto resolution,
the substitution machinery beyond classification, or the drain/signal
seams (R5 contract, already blessed).
