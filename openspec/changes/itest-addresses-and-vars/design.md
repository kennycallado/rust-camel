# Design: itest-addresses-and-vars

## Approach

Four pieces, one theme: the scenario tier speaks in real addresses
and data flows between actions.

1. Unified variables. `bindVar` fills the scenario variable map at
   boot, before action zero. `extract` overwrites on receive. Last
   writer wins, no load-time collision policing: refreshing a value is
   legitimate. `ScenarioVars::set` already has these semantics.
2. Interpolation. One resolver over `send` (`to.endpoint` whole
   string, body string leaves recursive, header values) and
   `receive.from.endpoint`. Raw substitution, no percent-encoding:
   `ParsedTarget::parse` rejects malformed results loudly. `$${`
   escapes a literal. Substitution is string-only.
   Unset variable at send time fails `scenario-var-unresolved`
   (existing runtime class, exit 1) naming the variable.
3. Address resolution. Harness-provisioned endpoint references
   resolve to the partner bound address in the driver, before any
   dial. The declared URI never reaches `TcpStream::connect`.
4. Partner scripting. A top-level `partners:` section, a map from the
   exact declared endpoint string (the `:0` URI as written) to a
   sequence of script entries, deserialized before boot. Each entry
   carries optional `method` and `path` matchers plus a `response`
   (status, headers, body). Full parity with the Rust
   `ScriptedResponse`. Present-and-unmatched serves 500 empty. An
   absent section keeps `permissive(200)`.

Receive-lane poisoning fix, adapter level: tag `in_flight` entries
with a generation counter. A send that fails before the wire (parse,
connect) removes its own entry, only when the generation still
matches, instead of delivering the error through the channel. A
post-connect failure parks as today. Client-role-first dispatch is
preserved. The replace-then-fail race needs a test: send A fails
after send B replaced the slot, the generation mismatch must leave
B's entry intact.

## Affected crates

- camel-integration-test: `document.rs` gains the `partners:` section
  and endpoint-ref validation; `runner.rs` gains the interpolation
  resolver and the harness map fill; `adapters/http.rs` gains the
  generation guard.
- camel-cli: the test command binds document-declared partners
  instead of permissive-only.
- Docs: crate README grammar sections, book testing chapter pointer,
  a runnable partner-direct example with a CRUD chain.

## Architecture boundaries

All work stays inside the itest crate plus the CLI test command. No
product code changes: the camel-component-http listener API is
untouched (bd rc-5yon owns it). The grammar stays strict
(`deny_unknown_fields`, hand-rolled walks naming offending keys). The
`${env:}` boot layer is untouched and deliberately does not resolve
in scenario strings: wire bytes must not depend on ambient
configuration.

## Alternatives considered

- One change with rc-5yon as a second phase: rejected. rc-5yon
  crosses into product code under the full gate set and deserves its
  own design pass. Coupling a p2 bugfix merge to a p3 product API
  redesign inflates both. The grammar here is its landing zone.
- Scripting on endpoint references: rejected. The reference walk
  enforces string-valued keys and `deny_unknown_fields`, structured
  response lists would break that invariant. The same partner is
  referenced from many actions, so per-ref scripts would need
  deduplication with conflict rules. What a partner serves is
  orthogonal to where an action sends.
- Prefer-arrival-over-parked for the poisoning fix: rejected. It
  inverts intended role semantics and forces polling two sources.
- Percent-encoding at substitution: rejected. It would double-encode
  pre-encoded tokens. The parse guard is loud enough.
- `${env:}` in scenario sends: rejected. Wire bytes would depend on
  ambient configuration, breaking replayability.
