# Proposal: bridge-otel-observability

## Why

The Java bridges (jms, xml, cxf) run blind from a tracing standpoint: the gRPC
IPC between Rust components and the bridges carries no trace context, so a
trace that crosses into a bridge simply vanishes — bridge logs cannot be
correlated with the Rust-side trace. Bridge logs are also plain free-form
text on stdout, which makes fleet log ingestion parse-hostile. Both gaps were
identified during the 0.6.0 pre-release (bd rc-qtn1, escalones 1 and 2).

Escalon 3 of rc-qtn1 (quarkus-opentelemetry with its own OTLP export) stays
out of scope: it opens a new egress surface with open decisions (collector
trust, native-image reflection config) and belongs after the
bridge-lockstep-hardening Quarkus bump. It will be tracked as a new bd issue
(P3, discovered-from rc-qtn1) filed by this change.

## What Changes

- Escalon 1 — traceparent IPC propagation: every RPC the Rust side sends to
  a Java bridge (Send/Subscribe/Health via `crates/components/camel-jms`)
  injects the current camel-otel context as gRPC metadata `traceparent`,
  reusing `crates/services/camel-otel/src/propagation.rs`
  (`inject_from_exchange`/`inject_context`, `TRACE_PARENT_HEADER`). No new
  propagation machinery. The JMS Java bridge surfaces the received
  traceparent in its logs via a gRPC server interceptor.
- Escalon 2 — structured JSON logging: the three Java bridges emit JSON log
  lines through the existing java.util.logging-to-stdout pipeline (Quarkus
  `quarkus-logging-json` console formatter; no log framework swap).
- Escalon 3 — split out: file the new bd issue verbatim, close rc-qtn1 with
  the split note.

Included: camel-jms client-side injection (feature-gated `otel`, following
the camel-http/kafka/ws pattern), bridges/jms interceptor + tests, JSON
logging wiring for jms/xml/cxf, docs updates.

Excluded: OTLP export from bridges, quarkus-opentelemetry, native-image
re-verification (next lockstep window), any Rust-side log changes.

## Acceptance criteria

- The in-proc gRPC harness in
  `crates/components/camel-jms/src/bridge_client_test.rs` captures
  `Request<>` metadata and asserts a well-formed `traceparent`
  (`00-<32hex>-<16hex>-<2hex>`) on Send, Subscribe, and Health when a
  context is active; `cargo test -p camel-jms --features otel` green.
- No active context means no `traceparent` metadata and no Java log line;
  existing suites unchanged (`cargo test -p camel-jms`, `./gradlew test`).
- A Java unit test asserts the traceparent value arrives intact through the
  bridge service (interceptor -> gRPC Context -> service log).
- All three bridges log JSON lines to stdout; existing gradle suites green.
- Escalon-3 bd filed with the verbatim escalon text; rc-qtn1 closes with
  the split note.

## Risk budget

Acceptable: additive gRPC metadata (bridges ignore unknown keys today),
one new optional dependency per bridge (`quarkus-logging-json`) and one
optional Rust dep for camel-jms behind the existing `otel` feature pattern.
Out of bounds: log framework swaps, new egress (OTLP), proto changes,
breaking the PortAnnouncer stdout protocol line, regressions in the
just-unblocked clippy gate for camel-jms/camel-bridge.
