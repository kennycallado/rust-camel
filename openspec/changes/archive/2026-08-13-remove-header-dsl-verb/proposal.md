# Proposal: remove-header-dsl-verb

## Why

The DSL provides `set_header` and `set_header_if_absent` to write exchange
headers, but offers no declarative way to DELETE a header. Today the only
escape hatch is a rhai script (`exchange.input.headers.remove("X")`), which
is heavyweight for a one-key deletion and forces users out of the declarative
route model.

Apache Camel ships `<removeHeader>` as a first-class EIP verb. rust-camel's
vocabulary parity is incomplete without it. The demo use case (rc-dey8) is
stripping `CamelHttpPath` before a bridged HTTP call — a single-key removal
that should be one YAML line, not a script.

## What Changes

- Add a `remove_header: { key: <name> }` declarative DSL verb mirroring
  Apache Camel `<removeHeader>`.
- Thread it through the full declarative pipeline: `RouteDslStep` →
  `DeclarativeStep` → `BuilderStep` → step compiler → processor.
- Add a `RemoveHeader` processor in camel-processor (mirrors `SetHeader`:
  input-only header mutation, consistent with the existing architecture).
- Include the verb in the public JSON route schema.
- Explicitly EXCLUDED: `remove_headers` (glob/pattern variant) — deferred to a
  follow-up. Output-message header removal — excluded because no existing
  processor mutates `output.headers` (SetHeader is input-only); introducing
  that pattern here is out of scope and YAGNI for the single-key use case.

## Acceptance criteria

- A route YAML containing `- remove_header: { key: CamelHttpPath }` parses,
  compiles, and removes that header from `exchange.input.headers` at runtime.
- Removing a non-existent key is a no-op (no error), matching
  `HashMap::remove` semantics.
- The JSON route schema (`route-schema.json`) exposes `remove_header` as a
  valid step, so `camel-lint` accepts routes using it.
- Empty key (`remove_header: { key: "" }`) is rejected at compile time with a
  clear error, mirroring the `set_header` empty-key validation.
- Unit tests cover: removal of an existing input header, no-op on missing
  key, and preservation of other headers.

## Risk budget

Low risk. The change threads a new variant through 4 enums and adds one small
processor that calls `HashMap::remove`. No new architectural patterns, no
control-plane surface, no async/runtime changes. The only accepted risk is
schema churn (route-schema.json gains one entry). Out of bounds: output-header
mutation, glob patterns, and any change to `set_header` behavior.
