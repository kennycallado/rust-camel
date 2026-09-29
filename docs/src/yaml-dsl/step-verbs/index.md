# Step verbs reference

Every YAML step verb and field, derived from the authoritative source
`crates/camel-dsl/src/route_ast.rs`. Each verb maps to a struct documented in
[Route structure](../route-structure.md).

Where a verb takes a predicate or value expression, the standard language fields
apply: `simple`, `rhai`, `jsonpath`, `xpath`, or `language` paired with
`source`. Each family page lists the fields of every verb in full.

## Families

- [Basic steps](basic.md): Send to endpoints, log, set message fields, and call beans.
- [Routing verbs](routing.md): Filter, branch, and copy the exchange to one or more targets.
- [Transformation and enrichment](transformation.md): Convert, marshal, script, and enrich the message body.
- [Messaging verbs](messaging.md): Split, aggregate, reorder, sample, and offload message state.
- [Cache verbs](cache.md): Cache message bodies and manage cache entries.
- [Resilience and control](resilience.md): Handle errors, pace traffic, repeat steps, and validate content.
