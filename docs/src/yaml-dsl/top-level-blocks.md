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
| `binding` | string | no | `json` | Binding mode: `json` or `raw` |
| `success_status` | integer | no | — | Success HTTP status |
| `request_schema` | object | no | — | Request body schema |
| `response` | object | no | — | Response definition |
| `description` | string | no | — | Operation description |
| `parameters` | map | no | `{}` | Additional parameters |

Operations bind in one of two modes. The default `json` mode accepts JSON-essence media types for `consumes` and `produces`: bare `application/json`, parameterized forms such as `application/json; charset=utf-8`, and `+json` suffixes such as `application/problem+json`. It unmarshals requests, marshals responses, and validates declared schemas automatically. Any other media type in `json` mode fails route load. The `raw` mode accepts any RFC 9110 type/subtype media type, leaves the request as `Body::Stream` with no automatic unmarshal or marshal, and sends the trimmed `produces` value as the response Content-Type. `request_schema` and `response.schema` are rejected in `raw` mode. The default success status is injected in both modes; a `raw` POST returns `201` with the declared `produces` type.

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
  server:
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

The input schema and resource URI travel percent-encoded on the lowered route's query string. Server runtime config (bind, TLS, caps) is owned by `Camel.toml` under `mcp.servers.<name>`; the block's server `name` must match a TOML key or the consumer start fails. The server `security_policy` propagates to every lowered route. See [MCP component](../components/mcp.md).

## Template declaration
| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `id` | string | yes | — | Template identifier |
| `parameters` | list | no | `[]` | Template parameters |
| `routes` | list | no | `[]` | Route definitions with `{{param}}` placeholders |

## Templated route instantiation
| Field | Type | Required | Description |
|---|---|---|---|
| `route_template_ref` | string | yes | Template ID to instantiate |
| `route_id` | string | no | Override route ID |
| `parameters` | map | no | Concrete parameter values |

**Reference**: [DSL crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-dsl/CONTEXT.md)
