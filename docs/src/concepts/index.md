# Core concepts

rust-camel builds every route from three abstractions. An Exchange carries the
data. A Route chains the processors that transform it. A Component owns a URI
scheme and connects the route to the outside world. This section defines that
model. Every pattern in the guide assumes it.

Read the pages in this order:

1. [Exchange and Message](exchange-message.md) - the envelope that carries the
   body, headers, properties, and error state. Every pattern mutates it.
2. [Routes and pipelines](routes-pipelines.md) - how a source and an ordered
   step list form a Tower service chain. This is where you compose patterns.
3. [Components and endpoints](components-endpoints.md) - how a Component turns
   a URI into the consumers and producers that do real I/O.
4. [Error handling](error-handling.md) - how faults propagate through a
   route, and how handlers control recovery.
5. [Data plane vs control plane](planes.md) - why message flow and route
   lifecycle run on separate trait hierarchies.
6. [Glossary](glossary.md) - the canonical name for every term. Open it
   when a word needs a precise definition.

When the model is clear, the [EIP patterns](../eip/index.md) show how to
compose these concepts into routing, transformation, and messaging solutions.
