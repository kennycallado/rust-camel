## ADDED Requirements

### Requirement: Route-view traversal is schema-context-scoped and free-form maps are opaque

The route-view CST walk SHALL carry a schema context (the candidate
subschemas applicable to the current node, resolved from the embedded
`route-schema.json`) and SHALL interpret mapping keys against that
context, not against name sets derived globally from the whole schema.
A key declared by the current node's context SHALL dispatch on its own
subschema: URI-bearing string/sequence leaves reach endpoint emission;
structured containers (objects with declared properties, or arrays of
object items) are traversed recursively; and a FREE-FORM subschema — an
object with no declared properties, keyed by arbitrary user data (REST
response `headers`, `security_policy.config`, `parameters` maps) — SHALL
be treated as an opaque leaf: the walk SHALL NOT interpret, emit
endpoints from, or recurse into its entries. An UNDECLARED key that the
active schema PERMITS (`additionalProperties` absent, `true`, or a typed
schema) SHALL be treated as legitimate user data — dispatched on the
`additionalProperties` schema when typed, opaque otherwise — and SHALL
NOT fall back to global name interpretation. Only an undeclared key that
every active candidate REJECTS (`additionalProperties: false`) SHALL
retain the legacy global-name fallback for schema-invalid shape
tolerance, with free-form maps excluded from that fallback's container
name set. Schema-context composition expansion SHALL use a branch-local
active-reference guard (per-lookup, popped on exit), so a definition
reachable through two different branch paths expands both times. The
walk SHALL hold no per-document mutable global state: linting one
document SHALL NOT influence another document's diagnostics.

#### Scenario: REST response header named uri is opaque

- **GIVEN** a rest document whose operation carries `response.headers.uri` set to an ordinary header value `timer:foo?frequency=1s`, and a catalog that knows `timer` with a `period` option
- **WHEN** R-URI-known runs over the document
- **THEN** no diagnostic references the header value (`frequency` is not flagged as an unknown option, and the scheme is not flagged)

#### Scenario: REST response headers named to and endpoints are opaque

- **GIVEN** a rest document whose `response.headers` contains `to: log:out` and `endpoints: [direct:a, direct:b]` entries
- **WHEN** the engine builds the route view
- **THEN** none of `log:out`, `direct:a`, `direct:b` appears as a captured endpoint URI

#### Scenario: REST operation to fields remain validated

- **GIVEN** the same rest document also carries an operation-level `to: timer:foo?frequency=1s` and a nested `steps: [{to: timer:bar?bogus=1}]`
- **WHEN** R-URI-known runs over the document
- **THEN** the operation-level and nested `to:` URIs are validated exactly as before the change (one UnknownOption on `frequency`, one on `bogus`)

#### Scenario: security_policy config free-form map is opaque

- **GIVEN** a route whose `security_policy.config` contains a `to: log:leak` entry
- **WHEN** the engine builds the route view
- **THEN** `log:leak` does not appear as a captured endpoint URI

#### Scenario: schema-invalid shapes keep legacy tolerance

- **GIVEN** a document using a schema-invalid but historically tolerated shape for endpoint nesting (e.g. `multicast:` with a direct sequence of steps, or an object-form `from`)
- **WHEN** the engine builds the route view
- **THEN** endpoint URIs in those shapes are still captured, byte-identical to before the change

#### Scenario: permissive-context stray keys are opaque

- **GIVEN** a document whose root (envelope, which permits undeclared keys) carries a stray `response:` key containing `to: log:stray`
- **WHEN** the engine builds the route view
- **THEN** `log:stray` does not appear as a captured endpoint URI

#### Scenario: nested endpoint capture across all root forms

- **GIVEN** three documents — envelope (`routes: [...]`), bare-route, and legacy array form — each containing nested `steps[].to: log:nested`
- **WHEN** the engine builds each route view
- **THEN** `log:nested` is captured with its own byte-exact span in all three forms

#### Scenario: recursive composition expands per branch

- **GIVEN** a route nesting `do_try` inside `do_try` (each level carrying `steps: [{to: log:deep}]`) — the schema path revisits `RouteDslStep` through `DoTryData.steps` twice
- **WHEN** the engine builds the route view
- **THEN** the innermost `to: log:deep` endpoint is captured

#### Scenario: cross-document isolation

- **GIVEN** a poisoned document (response headers containing URI-shaped header values) linted immediately before a clean document
- **WHEN** the clean document is linted
- **THEN** its diagnostics are identical to linting the clean document alone
