# camel-sql

SQL component for rust-camel: query execution, streaming (StreamList), and batch processing against SQL databases via `sqlx`.

## Language

**StreamList + streaming split**:
SQL `outputType=StreamList` combined with the streaming split EIP enables at-least-once row-at-a-time processing:

```yaml
- from:
    uri: "sql:SELECT * FROM users"
    parameters:
      outputType: StreamList
    steps:
      - split:
          streaming: true
          stream:
            format: ndjson
          steps:
            - set_property:
                name: userId
                simple: "${body.id}"
            - to: "sql:UPDATE users SET processed = true WHERE id = :#userId"
```

- **At-least-once semantics**: The consumer polls lazily — rows are fetched from the DB as the stream is consumed. The consumer does NOT commit/ack the batch until the entire split pipeline completes. If the pipeline crashes mid-stream, the batch re-delivers on next poll.
- **Deadlock warning**: The sub-pipeline's SQL producer creates its own pool connection. If the pool has only 1 connection and it's held by the consumer's StreamList fetch, the producer deadlocks waiting for a connection. Configure `max_connections >= 2` in the pool when a streaming split sub-pipeline uses `sql:` as a producer.
- **Field access**: NDJSON rows expose fields via the `:#` named parameter prefix in SQL statements (e.g., `:#userId`, `:#body.id`). These reference properties and body fields from the current fragment Exchange.

## Log-level policy

Per ADR-0012.

**Bridging:** this component supports `bridgeErrorHandler=true` (URI param). When enabled, poll-loop errors are wrapped as Exchanges and routed through the owning route's error handler — the consumer logs at `warn!` on the bridged path to avoid duplicate `error!` (Apache Camel ErrorHandler pattern).

**Labels wired in Phase B (commits cfb7c74c + 0a801cec):**

All 4 sites are category (b′) outside-contract: a normal-data `send_and_wait` or post-processing call returned Err, meaning the route handler did NOT absorb the failure — the consumer's `error!` is the only ERROR signal. Each site calls `runtime.metrics().increment_errors(route_id, label)` via a shared `record_post_process_failure` helper (`consumer.rs:41-51`), then logs at `error!` with `// log-policy: outside-contract`:
- `b-prime:sql:on-consume` (`consumer.rs:145`) — post-process single row failure.
- `b-prime:sql:on-consume-batch` (`consumer.rs:187`) — post-process batch row failure.
- `b-prime:sql:stream-list` (`consumer.rs:257`) — StreamList downstream send failure.
- `b-prime:sql:poll-failed` (`consumer.rs:356`) — unbridged poll failure.

**Category (g) labels wired in Phase B (commit 78ef8430):**
- `g:sql:producer-pool-init` (`producer.rs:188`) — producer lazy pool init failure. Calls `runtime.health().force_unhealthy_for_route(route_id, label, reason)` + `// log-policy: outside-contract`.
- `g:sql:consumer-pool-init` (`consumer.rs:442`) — consumer pool init giving up after retry budget exhausted. Calls `runtime.health().force_unhealthy_for_route(route_id, label, reason)` + `// log-policy: outside-contract`.

**Migration status (Phase B close):**
- All handler-owned sites (categories a, b-bridged) → `warn!`.
- All system-broken sites (category c) → `// log-policy: system-broken` + `error!`.
- All outside-contract sites (categories b′, e, g) → WIRED in Phase B with real `increment_errors` / `force_unhealthy_for_route` calls + `// log-policy: outside-contract` annotations. See ADR-0012 Phase B closure notes.
- Duplicate logs at `producer.rs:171` and `consumer.rs:160` removed (Phase 2).
- `consumer.rs:429` duplicate-`error!` bug fixed (Phase 2; split into bridged warn + unbridged error with increment_errors).

## Contract Surface

Per rc-o6o Phase 2 §3.4. The authoritative list of what the SQL placeholder
parser accepts, rejects, and silently does NOT do. Aligned with ADR-0016
(strict rejection of ambiguous syntax).

### Accepted

- **Positional `#`** — placeholder char (default `#`, configurable per
  Endpoint). Body MUST be a JSON array; bindings come from index.
- **Named `:#<token>`** — `<token>` is read until a SQL delimiter
  (whitespace, `?`, `(`, `)`, `,`, `;`, `=`, `<`, `>`, `!`, `'`, `"`, `` ` ``,
  `/`, `:`). Names may contain alphanumerics, `_`, `-`, `.`, `@`, and other
  characters valid in map keys. Resolution order via `ExchangeLookupPath`
  (see `crates/camel-api/src/exchange_lookup.rs`):
  - `body.x.y[.z]` walks JSON tree (`["x"]["y"]`).
  - `body.items.0` indexes arrays (numeric segments without leading zeros).
  - `header.some.name` — flat key `"some.name"` (headers are flat maps).
  - `property.x` / `exchangeProperty.x` — flat key.
  - anything else — unscoped fallback: body JSON flat key → header → property.
- **IN-clause `:#in:<token>`** — same terminator/name rules as Named; value
  MUST resolve to a JSON array. Empty array emits literal `NULL` (produces
  `IN (NULL)` which matches nothing). Non-empty array emits `$N, $N+1, ...`
  using the configured separator (default `, `).
- **Expression `:#${<expr>}`** — escape hatch. `<expr>` follows the same
  `ExchangeLookupPath` grammar as Named (so `body.user.name` walks JSON).
  Distinct from Named because the surrounding `${...}` is the explicit
  "use the path language" form; SQL shorthand `:#name` and the explicit
  `:#${name}` are equivalent for plain paths but `${...}` documents intent.
- **PostgreSQL `::cast`** — `:#id::text` resolves the placeholder as `id`
  and leaves `::text` as the SQL cast. Single `:` is a terminator; `::` does
  NOT extend the name.

### Rejected

- **Bare reserved scope names** (`:#body`, `:#header`, `:#property`, `:#exchangeProperty`) — SQL binds scalars, not whole scopes.
  - `:#body` → `CamelError::ProcessorError("SQL placeholder ':#body' requires a path (e.g. ':#body.field') — bare scope not supported")`.
  - `:#header`, `:#property`, `:#exchangeProperty` → `CamelError::ProcessorError` wrapping `LookupPathError::EmptyScopedKey { scope, input }` (raised at parse time).
  - Rationale (e_gpt oracle blessing #2): the scope name alone is ambiguous for SQL. `${body}` in Simple language DOES mean whole body (different caller, different semantics); but a SQL placeholder needs a concrete value to bind.
- **Empty placeholder name** (`:#`, `:#()`) — returns
  `CamelError::ProcessorError("Empty named parameter name at position N")`.
- **Unclosed expression** (`:#${body`) — returns
  `CamelError::ProcessorError("Unclosed expression at position N")`.
- **Unresolved parameter** (`:#missing` with no matching body / header /
  property) — returns `CamelError::ProcessorError("Named parameter 'missing'
  not found in body, headers, or properties")`.
- **Malformed path** (`:#body..x`, `:#body.`) — returns
  `CamelError::ProcessorError("Invalid placeholder name ...")` wrapping the
  `LookupPathError` from `ExchangeLookupPath::parse`.
- **IN-clause non-array value** — returns
  `CamelError::ProcessorError("IN clause parameter 'X' must be an array,
  got type Y")`.
- **Positional parameter with non-array body** — returns
  `CamelError::ProcessorError("Positional parameter requires body to be a
  JSON array")`.

### Silent behavior forbidden

- **Literal body keys starting with a reserved scope prefix are NOT
  addressable via SQL shorthand NOR via `:#${...}`.** A body JSON key
  literally named `body.user.name` (i.e. `{"body.user.name": "alice"}`) is
  NOT accessible via `:#body.user.name` (walks `body["user"]["name"]`) AND
  NOT accessible via `:#${body.user.name}` (also path semantics — both
  syntaxes share `ExchangeLookupPath`). To address such a key, move the
  value to a header or property, OR wrap the JSON so the key does not
  collide with the scope grammar. This is the cost of unambiguous grammar;
  documented here as the audit trail. See rc-o6o Phase 2 §3.2 + §6 risk and
  ADR-0016.
- **String literals are NOT scanned for placeholders.** Single-quoted SQL
  text (`'foo:#bar'`) is opaque; `#`/`:#` inside it is literal SQL. Escaped
  `''` inside a literal stays escaped.
- **No implicit type coercion.** A header value of `42i64` resolves as JSON
  number `42`; the binding layer (`sqlx`) handles DB driver conversion. The
  placeholder layer does NOT cast.
- **`::` Postgres casts are preserved literally.** The parser does NOT eat
  `::` as part of a name; it does NOT strip `::` either. `:#id::text`
  produces SQL fragment `::text` after `$N` substitution.
