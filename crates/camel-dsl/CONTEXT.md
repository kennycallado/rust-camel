# DSL

The route definition layer. Provides a fluent builder API and YAML/JSON configuration format for constructing Routes that the Runtime can execute.

## Language

**RouteDefinition**:
The structured representation of a Route — produced by RouteBuilder or by parsing a YAML/JSON file. CamelContext consumes RouteDefinitions to build and start Routes.
_Avoid_: route spec, route config, route descriptor

**RouteBuilder**:
Fluent Rust API for constructing a RouteDefinition programmatically using method chaining.
_Avoid_: builder, route factory

**Step**:
A single processing instruction in a RouteDefinition (e.g., `setBody`, `filter`, `to`, `choice`). Steps compile into Processors in the Runtime Pipeline.
_Avoid_: instruction, action, operation

**from**:
The source URI declaration that opens a RouteDefinition — identifies the Component and Endpoint that will produce Exchanges for the Route (e.g., `timer:tick`, `kafka:my-topic`).
_Avoid_: source, input, consumer URI (in DSL context)

**to**:
A Step that sends an Exchange to an Endpoint URI (e.g., `log:info`, `http:my-service`).
_Avoid_: sink, destination (when used as a DSL term)

## Example dialogue

> "I want to read from Kafka and send to HTTP."
> "Define a RouteDefinition using RouteBuilder: start with `from('kafka:my-topic')`, add any transformation Steps, then end with `to('http:my-service')`."
>
> "Can I define the same route in YAML?"
> "Yes — the YAML parser produces the same RouteDefinition. The Runtime doesn't know or care which form was used."
