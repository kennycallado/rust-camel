# Top-level blocks
A route file also accepts these top-level keys alongside `routes`.

## REST DSL
The `rest` key defines REST API blocks.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `host` | string | no | `0.0.0.0` | Listen host |
| `port` | integer | no | `8080` | Listen port |
| `path` | string | no | `""` | Base path |
| `security_policy` | object | no | — | Block authorization copied to every lowered route |
| `operations` | list | no | `[]` | HTTP operations |

REST operation:

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `method` | string | yes | — | HTTP method (GET, POST, ...) |
| `path` | string | no | `/` | Sub-path |
| `operation_id` | string | no | — | Unique operation ID |
| `to` | string | no | — | Target endpoint URI |
| `steps` | list | no | `[]` | Child steps |
| `consumes` | string | no | `application/json` | Request content type |
| `produces` | string | no | `application/json` | Response content type |
| `binding` | string | no | `json` | Binding mode: `json`, `raw`, `form`, or `xml` |
| `success_status` | integer | no | — | Success HTTP status |
| `request_schema` | object | no | — | Request body schema |
| `response` | object | no | — | Response definition |
| `description` | string | no | — | Operation description |
| `parameters` | map | no | `{}` | Additional parameters |

Operations bind in one of four modes. The default `json` mode accepts JSON-essence media types for `consumes` and `produces`: bare `application/json`, parameterized forms such as `application/json; charset=utf-8`, and `+json` suffixes such as `application/problem+json`. It unmarshals requests, marshals responses, and validates declared schemas automatically. Any other media type in `json` mode fails route load. The `raw` mode accepts any RFC 9110 type/subtype media type, leaves the request as `Body::Stream` with no automatic unmarshal or marshal, and sends the trimmed `produces` value as the response Content-Type. `request_schema` and `response.schema` are rejected in `raw` mode. The `form` mode, declared as `binding: form`, binds `application/x-www-form-urlencoded`. Its `consumes` and `produces` values must use exactly that media type, optionally followed by a single `; charset=utf-8` parameter, case-insensitively. Any other base, parameter, or charset value fails route load with a `RouteError` that names the operation, the field, the offending value, and the expected media. `request_schema` and `response.schema` are allowed: the form body is parsed into a JSON object first, and schema validation runs on that JSON, so the order is form parse, then JSON, then schema validate. The form pipeline is otherwise identical to `json`: for body-bearing verbs it injects `unmarshal: form` (carrying the request schema when declared), then the user steps, then the internal response-marshal step for `form`, the response `Content-Type`, and the default success status. Media negotiation is unchanged. The `xml` mode, declared as `binding: xml`, binds structured XML. Its `consumes` and `produces` values must be `application/xml`, `text/xml`, or any `type/*+xml` subtype, case-insensitively, optionally followed by a single `; charset=utf-8` parameter. Any other base, parameter, or charset value fails route load with a `RouteError` that names the operation, the field, the offending value, and the expected media. `request_schema` and `response.schema` are allowed: for POST, PUT, and PATCH the pipeline injects `unmarshal: xml` (carrying the request schema when declared), so the wire XML becomes `Body::Json`, then JSON Schema validation runs, then the user steps follow. The order is XML to JSON, then schema validation, then the user steps. GET and DELETE receive no unmarshal step but still marshal their response. The response pipeline adds the internal response-marshal step for `xml` after the user steps, and the declared response schema is OpenAPI metadata only, so the order is user JSON, then schema metadata, then XML text. The JSON projection mirrors leaf text as strings, so schemas describe that projection, not the wire XML. XML binding is not multipart. The response-marshal step is internal, not an authored `marshal:` step; authored `marshal:` steps in `steps:` still run as written, and a failure of the internal response-marshal step is reported as HTTP 500. The default success status is injected in every mode; a `raw` POST returns `201` with the declared `produces` type.

Declared `format` constraints in `request_schema` are enforced: a body that violates `format: email` is rejected. The schema's `$schema` key selects draft semantics; a schema without `$schema` is treated as draft 2020-12. Format enforcement for `request_schema` is always on; there is no per-operation opt-out. Payloads that passed validation before format enforcement may now be rejected; the `[components.validator] validate-formats = false` opt-out restores the previous accept behavior for validator endpoints.

### Media negotiation gate
Lowering injects a media negotiation gate as the first step of every lowered REST route. The gate enforces the declared `consumes` and `produces`:

- A request `Content-Type` the operation does not declare fails with `415`. Body-less verbs skip the request check.
- The `Accept` header is matched by media-range precedence; a mismatch fails with `406`. Equal-specificity ties take the lowest q value.
- An absent header or an undeclared side is permissive.
- A malformed `Accept` header degrades to `*/*`. A malformed `Content-Type` fails closed.

```yaml
- method: post
  path: /ingest
  binding: raw
  consumes: application/octet-stream
  produces: text/plain
  to: direct:ingest
```

```yaml
- method: post
  path: /orders
  binding: xml
  consumes: application/xml
  produces: application/xml
  request_schema:
    type: object
    properties:
      order:
        type: object
  to: direct:orders
```

### Raw binding streaming contract
`raw` operations own the stream semantics of their request and reply bodies.
The contract:

- **No pipeline caching.** Lowering injects no `unmarshal`/`marshal`, so no
  `StreamCacheService`-wrapped processor compiles ahead of the user steps.
  The request `Body::Stream` reaches the first user step unpollied; the
  injected `Content-Type` and default-status steps never read it.
- **Single consumption.** The request stream is consumed at most once. A
  second consumption attempt fails with `AlreadyConsumed` and propagates as
  a route error — never a panic. A reply whose stream was already consumed
  returns HTTP 500 with an empty body.
- **Metadata preservation.** The HTTP consumer records the request
  `Content-Type` and `Content-Length` in the stream metadata before the
  exchange enters the route, and the pipeline never alters them.
- **Original or new reply stream.** A route may reply with the original
  request stream (echo) or a newly generated `Body::Stream`; both are
  streamed to the wire under the route-supplied `Content-Type`.
- **Request limits fail closed.** A request with `Content-Length` over
  `max_request_body` is rejected 413 before the stream opens. A chunked
  request over the cap fails with the limit error when consumed.
- **Response limits cover materialized bytes only.** `max_response_body`
  caps materialized reply bodies — an over-cap materialized reply is
  replaced with HTTP 500 (`Response body exceeds configured limit`). A
  streamed reply is not byte-capped: capping a stream mid-flight would
  truncate an already-committed response, so routes that need response
  caps must materialize the body first.
- **Client disconnects do not fail the consumer.** If a client drops the
  connection during a streamed reply, the server keeps serving subsequent
  requests.

## MCP catalog
The `mcp` key declares an MCP server catalog (ADR-0060). Each tool lowers to an `mcp:<server>/tool/<name>` consumer route; each resource lowers to an `mcp:<server>/resource/<name>` consumer route.

| Field | Type | Required | Description |
|---|---|---|---|
| `server` | object | no | Server declaration: `name`, `bind`, `security_policy` |
| `tools` | list | no | Tool declarations: `name`, `input_schema` (the `input_schema` must be a JSON object — lowering rejects other JSON shapes at parse time, naming the tool and the offending kind, bd rc-ap58) |
| `resources` | list | no | Resource declarations: `name`, `uri` |

```yaml
mcp:
  - server:
      name: crm
      bind: 127.0.0.1:9100
      security_policy: { roles: [mcp-client] }
    tools:
      - name: lookup
        input_schema:
          type: object
          properties:
            id: { type: string }
          required: [id]
    resources:
      - name: customers
        uri: crm://customers
```

The input schema and resource URI travel percent-encoded on the lowered route's query string.
The block's server `name` must match a `Camel.toml` key under `mcp.servers.<name>` or consumer start fails.
The DSL block's `bind`, `tls`, `max_tools`, and `max_resources` values configure the runtime listener.
If TOML and the DSL declare different values for the same key, startup fails and names both sources.
Keys declared on only one side use that side's values. The server `security_policy` propagates to every lowered route.
See [MCP component](../components/mcp.md) and the
[MCP crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-mcp/CONTEXT.md#config-keys).

## Template declaration
| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `id` | string | yes | — | Template identifier |
| `parameters` | list | no | `[]` | Template parameters |
| `routes` | list | no | `[]` | Route definitions with `{{param}}` placeholders |

### Placeholders and language expressions
Route-template `{{param}}` placeholders substitute in template strings, both embedded (`log:relay-{{body}}`) and as a whole node. A whole-node `number` or `boolean` parameter yields a JSON number or boolean; a `string` parameter yields a string.

A `language: minijinja` expression whose `source` is a string is not a substitution target. Its `source` passes through byte-identically, so minijinja syntax such as `{{ headers.name }}` reaches the compiler unchanged and a template parameter named like a minijinja variable never collides. Only `source` is exempt; other fields on the same route substitute normally. Other expression languages (`rhai`, `simple`, `jsonpath`, `xpath`) keep ordinary substitution.

To emit literal `{{...}}` from a normal template string, escape the opening braces: `\{{name}}` renders `{{name}}` with no substitution and no whole-node typing. Write `\{{` in a YAML plain or single-quoted scalar; a double-quoted YAML scalar treats the backslash as an escape and does not carry it literally. In a JSON route body write `\\{{`. An unescaped `{{name}}` naming an undeclared parameter still fails with `UnknownParameter`. `${...}` environment interpolation is unaffected.

```yaml
templates:
  - id: relay
    parameters:
      - name: body
        type: string
    routes:
      - id: "relay-{{body}}"
        from: "direct:start"
        steps:
          - script:
              language: minijinja
              source: "{{ body }} passthrough"
          - to: "log:relay-{{body}}"
  - id: escaped
    parameters: []
    routes:
      - id: 'esc-\{{x}}-route'
        from: "direct:start"
        steps:
          - to: "log:info"
```

## Templated route instantiation
| Field | Type | Required | Description |
|---|---|---|---|
| `route_template_ref` | string | yes | Template ID to instantiate |
| `route_id` | string | no | Override route ID |
| `parameters` | map | no | Concrete parameter values |

**Reference**: [DSL crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-dsl/CONTEXT.md)
