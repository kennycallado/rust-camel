# Introduction

rust-camel is a Rust-native integration framework. It moves messages between
timers, HTTP APIs, files, message brokers, databases, and LLMs. You compose
each route from Enterprise Integration Patterns.

The pattern vocabulary comes from Apache Camel. The implementation does not.
Every processor and producer is a Tower `Service<Exchange>`. A route is a
middleware chain, so backpressure, timeouts, and cancellation are built in.
The compiler checks the route before it runs. See [ADR-0001](adr/0001-tower-data-plane-split-from-control-plane.md) for the data-plane
design.

Two interfaces build the same `RouteDefinition`. A fluent Rust API gives
developers compile-time safety. A YAML DSL gives operators declarative
authoring. Neither is a wrapper around the other.

You can build:

- HTTP APIs that fan out to Kafka and a database.
- File pipelines that watch a directory, transform, and publish.
- Message routers that split, filter, and resequence broker streams.
- Scheduled jobs that call an LLM and write the reply to a sink.

rust-camel is not a Camel port. It promises no Camel compatibility. It
promises Camel familiarity. A Camel user recognizes Filter, Content-Based
Router, and Splitter on first read. See [ADR-0046](adr/0046-apache-camel-inspiration-not-conformance.md) for the consultation
protocol that governs when Camel behavior informs this design.

Start with the [Getting started guide](getting-started/index.md).
