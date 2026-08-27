## ADDED Requirements

### Requirement: Step span kind reflects endpoint semantics

`.to(...)` process step spans SHALL carry an OTel `SpanKind` derived at
compile time from the authored endpoint URI scheme: outbound messaging
schemes (kafka, jms, activemq, artemis, mqtt) SHALL map to
`SpanKind::Producer`; outbound request/response and database schemes
(http, https, grpc, grpcs, ws, redis, opensearch, sql, surrealdb, cxf,
llm, mcp) SHALL map to `SpanKind::Client`; local execution schemes
(direct, seda, timer, and other non-endpoint schemes) and scheme-less
URIs SHALL map to `SpanKind::Internal`. All non-`.to` process steps (EIPs)
SHALL map to `SpanKind::Internal`. Scheme comparison SHALL be
case-insensitive. Route root spans and segment spans SHALL remain
`SpanKind::Internal`.

#### Scenario: HTTP outbound step opens a Client span

- **GIVEN** a traced route with a `.to("http://api.example/x")` step
- **WHEN** the pipeline processes an exchange
- **THEN** that step's span has `SpanKind::Client`

#### Scenario: Kafka outbound step opens a Producer span

- **GIVEN** a traced route with a `.to("kafka:orders")` step
- **WHEN** the pipeline processes an exchange
- **THEN** that step's span has `SpanKind::Producer`

#### Scenario: In-memory step stays Internal

- **GIVEN** a traced route with a `.to("direct:tree-sub")` step and an EIP
  (filter) step
- **WHEN** the pipeline processes an exchange
- **THEN** both step spans have `SpanKind::Internal`

#### Scenario: Root and segment spans stay Internal

- **GIVEN** a traced route whose entry is a remote consumer and whose body
  contains a splitter segment
- **WHEN** the pipeline processes an exchange
- **THEN** the route root span and the segment span both have
  `SpanKind::Internal`
