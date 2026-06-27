# `script:` Is Synchronous; Async JS Belongs in `function:`

**Status:** Accepted
**Absorbs:** DEC-1 (JS engine choice: Boa)

`script:` uses **Boa** for synchronous expression evaluation only — predicates, header/body transformations, simple logic. Async JS (`await`, `fetch`, npm imports) is explicitly out of scope for `script:` and belongs in `function:`.

Allowing async in `script:` would let user code evade circuit breakers, retry logic, and pipeline-level metrics, since those operate at the Tower layer above the script call. It would also introduce a JS event loop inside the Tokio runtime with no clean integration boundary. The `function:` step already handles the async use case with proper isolation and flow auditability.

## Engine choice: Boa (absorbed from DEC-1)

In an integration framework, scripting languages serve as **expression evaluators**, not orchestration engines. This matches Apache Camel, where all scripting languages (Groovy, JS/Nashorn, Simple, OGNL) are strictly synchronous. Async I/O from a script would also obscure error-handling visibility and lose flow auditability (the YAML would no longer describe the full route). `camel-language-js` therefore stays on **Boa**; the original "Boa vs rquickjs" comparison is closed (rquickjs async support is no longer a motivating factor).

| Step        | Engine                                 | Scope                                                                                                        |
| ----------- | -------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `script:`   | Boa (in-process, sync)                 | Predicates (`camel.headers.get('type') === 'urgent'`), body/header transformations, simple synchronous logic |
| `function:` | Deno container (out-of-process, async) | Async I/O (`fetch`, `camel.send`, ProducerTemplate), imports/npm, complex logic needing a full event loop    |
