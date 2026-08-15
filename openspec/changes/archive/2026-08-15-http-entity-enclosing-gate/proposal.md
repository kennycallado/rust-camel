# Proposal: http-entity-enclosing-gate

## Why

The HTTP producer sends a request body even when the resolved method is GET
or HEAD (bd rc-q1sw). The body attach path in
`crates/components/camel-http/src/lib.rs` (materialization at ~L2135, attach
at ~L2150-2185) never checks the method. Three paths attach a body: direct
send, stream send, and the SSRF-safe redirect loop. The redirect loop
re-sends the body on every 307/308 hop.

RFC 9110 section 9.3.1 states that a client SHOULD NOT generate content in
a GET request. Section 9.3.2 extends the same rule to HEAD. Servers,
proxies, and caches reject, truncate, or drop such requests. The violation
grows on each redirect hop.

Apache Camel (Java) is the reference for this project. `HttpMethods.java`
marks GET, HEAD, DELETE, OPTIONS, and TRACE as not entity-enclosing.
`HttpProducer.createMethod` builds a request entity only when the method is
entity-enclosing. The Java client enforces this by type: `HttpGet` and
`HttpHead` do not accept an entity. rust-camel kept the method-resolution
half of that design (bd rc-f2cj) but dropped the body-gating half. This
change restores the missing half.

## What Changes

- Add an `is_entity_enclosing` predicate for HTTP methods in the producer.
  Entity-enclosing: POST, PUT, PATCH. Not entity-enclosing: GET, HEAD,
  DELETE, OPTIONS, TRACE.
- When the resolved method is not entity-enclosing, the producer SHALL NOT
  attach the exchange body to the outbound request, in all three send paths.
- When a body is dropped this way, the producer emits exactly one `warn!`
  log entry with the resolved method and correlation id. A stream body
  always warns, because its emptiness cannot be known without consuming it.
- The exchange body stays consumed (`std::mem::take` semantics unchanged).
  The producer does not restore it.
- No configuration knob. camel-http has no `allowGetBody` escape hatch, so
  this change adds none.
- Spec delta: new requirement `Producer method/body coupling` in
  `http-emission-correctness`.

## Acceptance Criteria

1. GET or HEAD with a non-empty exchange body sends no body and no
   Content-Length.
2. DELETE, OPTIONS, TRACE with a body behave the same.
3. POST, PUT, PATCH with a body still send the body (regression guard).
4. The drop emits exactly one `warn!` that names the resolved method.
5. A GET with a body through a 307/308 redirect chain sends no body on any
   hop.
6. A stream body under GET is not attached, and the stream does not leak an
   `AlreadyConsumed` error.
7. An empty body under GET sends no body and emits no `warn!`.

## Risk Budget

Behavior change is observable but narrow: it fires only when a caller sets
an explicit non-enclosing method on an exchange that carries a body. The
method-resolution order (rc-f2cj) does not change. Default routes that rely
on body-based POST inference see no difference: a non-empty body still
infers POST, which stays entity-enclosing.

Known compat cost, accepted as intentional Apache Camel parity: DELETE with
a body is legitimate in some APIs today (for example Elasticsearch DELETE
by query), and body-carrying extension methods become unsupported the same
way. Apache Camel's `HttpMethods` marks DELETE as not entity-enclosing, so
this project follows the reference behaviour deliberately. A caller that
needs DELETE-with-body must transform the exchange (for example move the
body into a query param or header) before the producer step.
