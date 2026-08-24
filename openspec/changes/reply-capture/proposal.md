# Proposal: reply-capture

## Why

`camel test` cannot assert what a synchronous direct request-reply route replies. The delivery loop in
`crates/camel-cli/src/commands/test/runner.rs` (`deliver_input`, :152-195)
calls `producer.oneshot(exchange)` and discards the `Ok(Exchange)` reply at
:183 (`Ok(_) => return Ok(())`). The reply is already in hand — the direct
producer awaits it (`DirectProducer::call`, `crates/components/camel-direct/
src/lib.rs:436-496`) — so every test of such a route today silently
loses the response it exists to check. This is the second gate on Stage C's
lint escalation (e_opus advisory A1 on rc-car5, mirroring the beans gap closed
by bean-test-registry) and ADR-0064 lists it as an open problem (:210-212).

## What Changes

Capture the synchronous direct reply per input and assert it declaratively.

- `TestInput` gains an optional `expectReply: {body?, headers?}` field
  (`deny_unknown_fields`, camelCase) — natural 1:1 pairing with delivery;
  inputs without `expectReply` keep today's behavior.
- Reply assertion is exact-match v1: `body` (string-or-JSON, `InputBody`
  shape) and/or `headers` (exact `HashMap<String, serde_json::Value>` submap,
  matching `Message.headers`), asserted against the reply message —
  `output` when the route set one, else the final `input` message (canonical
  Exchange semantics; nothing in the lean set sets `output` today).
- Replies map to inputs by order — delivery is strictly sequential
  (runner.rs:360-368), so pairing is deterministic.
- Evaluation happens in the existing evaluate phase alongside endpoint
  results: each reply produces a PASS/FAIL line (e.g. `reply[0] direct:in`)
  counted into the summary; failed reply expectations are assertion failures
  (exit 1), NOT document errors. Delivery-time `Err` stays a doc error
  (exit 2), unchanged.
- An input with `expectReply` whose reply body/headers mismatch → FAIL line +
  exit 1 (same class as a mock expectation failure).
- A document may be reply-only: non-empty `expects` is no longer required
  when at least one input declares `expectReply` (a document with neither is
  still a document error).
- `direct:` producers always resolve or err (synchronous direct reply);
  timeouts/errs remain exit-2 delivery failures.

Explicitly excluded:
- Matcher-based assertions (regex/predicates) — rc-3kwt territory, composes
  later (`expect_header_regex` already exists unwired in camel-mock).
- Non-`direct:` input schemes (already rejected by validation).
- Output-only assertions (asserting the presence of an `output` message as
  such) — the reply surface already selects `output` first; properties and
  error-field assertions are out of scope.
- Concurrent delivery; pattern switching.
- Any camel-core, camel-direct, or camel-mock change.

## Acceptance criteria

- A route `from: direct:in` with a `set_body` step (or bean `setBody` stub)
  and an input `expectReply: {body: "enriched"}` passes with a PASS reply
  line; a wrong expected body fails with exit 1 and a FAIL reply line.
- Header assertions work the same way (exact map).
- Inputs without `expectReply`: byte-identical behavior to today (guard
  test).
- Reply evaluation composes with mock `expects` in one document; exit-code
  precedence 2 > 1 > 0 unchanged.
- Docs (`docs/src/testing/index.md`) document `expectReply` + the
  reply-message contract (`output` first, else final input message);
  example verified green.

## Risk budget

Additive camel-cli surface (document.rs + runner.rs + report plumbing in
test.rs driver); no other crate. Accepted risks: none beyond the exact-match
v1 surface being deliberately minimal. Out of bounds: any camel-core change,
any matcher engine, any non-direct scheme.
