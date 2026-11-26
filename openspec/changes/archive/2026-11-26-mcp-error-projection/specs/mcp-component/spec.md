## ADDED Requirements

### Requirement: MCP server error projection

The MCP server SHALL project every route failure to the client through the
ADR-0097 envelope contract, whether the failure arrives as a failed exchange
or as an error still carried by an `Ok` exchange: a business failure SHALL
produce an error result (`isError: true` for tools, error content for
resources) whose payload is exactly `{"error": <code>, "kind": <kind>}`; a
non-business failure SHALL produce the generic body
`{"error":"Internal Server Error"}` with the failure detail logged at the
adapter boundary. No client-visible MCP failure payload SHALL contain a URL,
a source path, or a Rust type name. The no-internals rule SHALL apply to
projected route and materialization failure bodies only — never to successful
route content, protocol metadata, or unrelated protocol errors. An exchange
whose error was handled and cleared SHALL remain a successful result. The
same projection SHALL apply to the tool path and the resource-read path. The
envelope SHALL be produced by the shared `camel-api` projection helper — no
component-local duplicate.

#### Scenario: business failure projects to the envelope

- **GIVEN** a tool route containing a `throw:` step with code `ORDER_NOT_FOUND`
  and kind `not-found`
- **WHEN** a host calls the tool
- **THEN** the result has `isError: true` and its content is exactly
  `{"error":"ORDER_NOT_FOUND","kind":"not-found"}` with no `message` or `data`
  field

#### Scenario: technical failure ships a generic body

- **GIVEN** a tool route backed by an HTTP endpoint whose request fails in a
  way whose reqwest error text contains the target URL
- **WHEN** a host calls the tool
- **THEN** the result has `isError: true` and its content is exactly
  `{"error":"Internal Server Error"}` — containing no `http://` substring, no
  `ssrf` text, and no Rust type name — and the adapter's WARN log line does
  carry the failure detail

#### Scenario: resource-read failure uses the same projection

- **GIVEN** a resource route whose read fails, once with a business error and
  once with a non-business error
- **WHEN** a host reads the resource
- **THEN** the returned content is the same envelope JSON for the business
  failure and the generic body for the non-business failure, with MIME type
  `application/json`, and no raw error string reaches the client

#### Scenario: body materialization failure ships a generic body

- **GIVEN** a tool route whose output body fails to materialize (oversize or
  unreadable stream)
- **WHEN** the bridge serializes the output
- **THEN** the tool result content is the generic body with `isError: true`
  and the failure detail appears only in the log

#### Scenario: carried exchange error projects through the same contract

- **GIVEN** a route that returns `Ok(exchange)` with `exchange.error` still
  set, once with a business error and once with a non-business error
- **WHEN** a host calls the tool (or reads the resource)
- **THEN** the result is projected exactly like the corresponding failed
  exchange, while an exchange whose error was handled and cleared remains a
  successful result

### Requirement: Argument schema violation redaction

The MCP server SHALL build schema-validation violations from the violation's
keyword (via jsonschema's `ValidationErrorKind::keyword()`) and a redacted
instance path only. The instance path SHALL NOT echo caller-supplied object
member names that are not declared by the schema — undeclared segments SHALL
render as a placeholder (`<prop>`); numeric array indices and schema-declared
property names SHALL pass through. The server SHALL cap the violation list at
25 entries and SHALL NOT include any argument text in the `ErrorData` of a
rejected `tools/call` — argument values therefore never reach rmcp's WARN
logging of handler errors.

#### Scenario: oversized argument value is redacted

- **GIVEN** a tool route whose input schema requires `id` to be a string of
  at most 10 characters
- **WHEN** a host calls the tool with a 1 MiB string in `id`
- **THEN** the rejection's `ErrorData` contains no portion of the argument
  value, and each violation names only the keyword and the redacted instance
  path

#### Scenario: caller-invented member names are not echoed

- **GIVEN** a tool route whose input schema declares `properties.id` and
  constrains additional members via `patternProperties`
- **WHEN** a host calls the tool with a payload containing a dynamically
  invented member name that violates the schema
- **THEN** the invented name appears neither in the serialized `ErrorData`
  nor in the WARN log, and the violation path shows the `<prop>` placeholder
  for that segment

#### Scenario: nested boolean subschema reports its keyword

- **GIVEN** a tool route whose input schema contains a nested `false`
  subschema (e.g. `{"properties": {"value": false}}`)
- **WHEN** a host calls the tool with a value at that position
- **THEN** the violation's keyword is the validator's kind keyword
  (`falseSchema`), not a schema-path segment, and no argument text is present

#### Scenario: violation list is capped

- **GIVEN** a tool route whose input schema can produce more than 25 violations
  for one payload
- **WHEN** a host calls the tool with such a payload
- **THEN** the rejection's violation list contains at most 25 entries
