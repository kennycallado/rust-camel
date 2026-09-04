# Proposal: itest-send-method

## Why

The scenario `send` action has no `method` field. The HTTP adapter
infers the client method as `POST` when a body is present and `GET`
when it is not (adapters/http.rs:466-469). A scenario therefore cannot
send `PUT`, `DELETE`, `PATCH`, or `HEAD` at all, cannot send a
bodyless `POST` (it silently becomes `GET`), and cannot send a `GET`
with a body (it becomes `POST`). Any route whose inbound behavior
branches on method (a REST consumer where `GET` reads and `PUT`
updates) cannot have those branches tested. The outbound direction is
unaffected: route configuration sets the producer method and the
partner matcher discriminates by method. bd rc-5mii, found in the
post-merge review of ADR-0069 v1.

## What Changes

- `send` gains an optional `method` field, normalized to uppercase and
  validated as an RFC 7230 token at document load.
- When `method` is absent, the current inference is preserved exactly
  (body implies `POST`, no body implies `GET`). Existing documents keep
  running unchanged.
- When `method` is present, it wins: a bodyless `POST`, a `DELETE`
  with a body, and `PUT` without a body all become expressible.
- An invalid token (a space, a slash) fails document validation with
  the action index in the error, exit `doc-validation` 2, without the
  `http` feature compiled: validation is a pure token check in the
  grammar layer.
- The crate README documents the field in the scenario vocabulary and
  example.

Excluded: `receive`-side changes, changes to partner scripting
behavior, changes to the `validate` action, and any transport beyond
plain `http`.

## Acceptance criteria

- A scenario with `method: PUT` and no body sends a request whose
  method is `PUT`, proven end to end: the harness scripted response matches
  only `PUT` and the received body validates.
- A document with `method: "P UT"` fails validation with exit 2 and
  the action index in the error, with the `http` feature off.
- Existing scenario documents keep their current observable behavior:
  the inference is unchanged, proven by focused tests asserting
  inferred `POST` with a body and inferred `GET` without one, and the
  corpus and e2e suites stay green without edits.

## Risk budget

Grammar addition with a defaulted field on one action. The failure
mode is a mis-parsed document, which the strict loader already rejects
with `deny_unknown_fields`. Worst case: an inference regression on
existing documents, caught by the unchanged e2e suite. Out of bounds:
touching any other action, the exit taxonomy, or the partner side.

Bd: rc-5mii
