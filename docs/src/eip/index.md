# EIP patterns

rust-camel implements the Enterprise Integration Patterns catalog (Hohpe and Woolf) as Tower middleware. Every pattern is a `Service<Exchange>`. You add it to a route builder next to a source and a sink.

The patterns group into four families:

- [Routing](routing.md): decide where an exchange goes next
- [Transformation](transformation.md): change the content, format, or type of the exchange body
- [Messaging](messaging.md): split, aggregate, reorder, and sample exchanges
- [Resilience and control](resilience.md): protect a route from failure, limit throughput, and scope error handling

The vocabulary is shared with Apache Camel for familiarity, not compatibility ([ADR-0046](../adr/0046-apache-camel-inspiration-not-conformance.md)).

For route steps that are not EIPs (Stream Cache, Bean, Stop), see [Processing steps](../steps/index.md). For the route structure that hosts these patterns, see [Routes and pipelines](../concepts/routes-pipelines.md).

Most patterns take a predicate or expression. For the available languages, see [Expression languages](../languages/index.md).
