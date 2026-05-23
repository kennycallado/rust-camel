# `script:` Is Synchronous; Async JS Belongs in `function:`

`script:` uses Boa for synchronous expression evaluation only — predicates, header/body transformations, simple logic. Async JS (`await`, `fetch`, npm imports) is explicitly out of scope for `script:` and belongs in `function:`.

Allowing async in `script:` would let user code evade circuit breakers, retry logic, and pipeline-level metrics, since those operate at the Tower layer above the script call. It would also introduce a JS event loop inside the Tokio runtime with no clean integration boundary. The `function:` step already handles the async use case with proper isolation and flow auditability.
