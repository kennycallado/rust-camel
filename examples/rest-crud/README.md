# REST CRUD Example

Runnable demo of the `rest:` YAML DSL. One YAML block defines the API
surface; `camel_dsl::yaml::parse_yaml` lowers each operation into an `http:`
route with `unmarshal(json)`, `to(direct:...)`, `marshal(json)`, and the
right `CamelHttpResponseCode` default. Handler routes consume from the
`direct:` endpoints and perform CRUD against an in-memory DashMap. A static
mount co-hosts on the same port (ADR-0009).

## Running

From the workspace root:

```bash
cargo run -p rest-crud
```

The server listens on `0.0.0.0:9090`. Press Ctrl+C to stop.

## Endpoints

| Method   | Path                  | Status          | Notes                            |
|----------|-----------------------|-----------------|----------------------------------|
| GET      | `/api/users`          | 200             | list all users (JSON array)      |
| POST     | `/api/users`          | 201             | create user (`{name, email}`)    |
| PUT      | `/api/users/{id}`     | 200 / 404       | update user (full replace)       |
| DELETE   | `/api/users/{id}`     | 204 / 404       | delete user                      |
| GET      | `/static/index.html`  | 200             | static mount co-hosting          |

Two users (`Alice`, `Bob`) are seeded on startup.

## Smoke test

```bash
# list
curl -s http://localhost:9090/api/users
# create
curl -s -X POST http://localhost:9090/api/users \
  -H 'Content-Type: application/json' \
  -d '{"name":"Charlie","email":"charlie@example.com"}'
# update
curl -s -X PUT http://localhost:9090/api/users/1 \
  -H 'Content-Type: application/json' \
  -d '{"name":"AliceUpdated","email":"alice@new.com"}'
# delete
curl -s -o /dev/null -w '%{http_code}\n' -X DELETE http://localhost:9090/api/users/2
# static
curl -s http://localhost:9090/static/index.html
```

## How it works

1. A `rest:` YAML block defines the API (path, operations, verbs, status codes).
2. `camel_dsl::yaml::parse_yaml` parses it. The DSL expansion (Phase 1) lowers
   each `operations.*` entry into an `http:` consumer route with:
   - `set_header(CamelHttpResponseCode, default)` — POST 201, DELETE 204, GET/PUT 200
   - `unmarshal(json)` for verbs with a body (POST, PUT)
   - `to(direct:...)` — the operation's `to` target
   - `marshal(json)` — serialise the body back to JSON wire form
   - `set_header("Content-Type", "application/json")` — the explicit content
     type so the reply finaliser ships JSON, not text/plain
3. The example registers four handler routes with `RouteBuilder::from("direct:...")`
   that consume from those endpoints. Each closure performs CRUD against the
   shared `UserStore` (DashMap) and sets the response body.
4. A second `http-static:` route mounts `examples/rest-crud/static/` on the
   same port. The HTTP co-hosting contract (ADR-0009) dispatches API exact
   path first, then static prefix, then fallback — so the JSON API and the
   static HTML coexist on one listener.

## Architecture

```
rest: YAML ──parse_yaml──▶ http: routes (with to: direct:...)
                                  │
                                  ▼
                       direct: handler routes (with .process() closures)
                                  │
                                  ▼
                          DashMap storage (CRUD)

         http-static:/static  ──▶ static file server (same port)
```

## Code map

| File                                 | Responsibility                                |
|--------------------------------------|-----------------------------------------------|
| `examples/rest-crud/Cargo.toml`      | workspace deps (camel-dsl, components, …)    |
| `examples/rest-crud/src/main.rs`     | entry point, REST YAML, route handlers        |
| `examples/rest-crud/src/storage.rs`  | DashMap-backed `UserStore`                    |
| `examples/rest-crud/static/`         | static mount directory                        |

## Error handling

- **Malformed JSON body → 400 Bad Request.** The `unmarshal(json)` step
  raises `CamelError::TypeConversionFailed`; the HTTP finaliser maps this
  to `400 Bad Request` with a structured JSON error body
  (`{"error":"bad_request","message":"..."}`).
- **Schema validation failure → 400 Bad Request.** When `request_schema`
  is declared on an operation, the body is validated against the schema
  after unmarshaling. Failures return `ValidationError → 400` with
  details of which schema constraints were violated.
- **Other pipeline errors → 500 Internal Server Error.**
