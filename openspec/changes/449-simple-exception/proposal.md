# Proposal: 449-simple-exception

## Why

`do_try` catch bodies need the exception value itself, but Simple only exposed
`${exception.message}`, so a catch body could not read the error kind without
string surgery. `camel-astro`, an external consumer, needs `${exception}` to
route on the variant. The `do-try` docs also lacked a pointer to the canonical
error-kind table, so readers had no single place to learn which kinds exist.

## What Changes

- Simple gains the `${exception}` lookup. It renders `<variant_name>: <Display>`
  (for example
  `HttpOperationFailed: HTTP GET http://svc/api failed: 500 Internal Server Error`)
  and evaluates to `Value::Null` when the exchange carries no error.
  `${exception.message}` keeps its existing contract and output.
- The `do-try`, `resilience`, and `languages/simple` docs gain cross-links to
  the canonical error-kind table, and `languages/simple.md` states the
  `${exception}` contract.
- Delta on `openspec/specs/language-value-boundary`: MODIFIED requirement(s)
  that enumerate the Simple atom and slot grammars.

Non-goals: no `doTry` catch-semantics change, no error-taxonomy change, no
exception serialization.

## Acceptance criteria

- `${exception}` renders kind-prefixed Display; response-body payloads and Debug
  formatting never appear in the rendered value.
- `${exception}` is `Value::Null` when the exchange has no error.
- `${exception.message}` output is unchanged.
- `openspec validate 449-simple-exception --type change --json` and
  `openspec validate --all --strict --json` pass.

## Risk budget

Acceptable: the rendering uses only the static variant name and the existing
`Display` contract, so no new data reaches the payload.
Out of bounds (non-goals): catch-body control flow, error taxonomy, exception
serialization.
