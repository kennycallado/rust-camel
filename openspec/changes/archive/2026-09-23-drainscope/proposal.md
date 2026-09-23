# Proposal: drainscope

## Why

Mission 238 reviewer (bd rc-j27pc, discovered from rc-1e7sb) found a bare
`while let Some(envelope) = route_rx.recv().await` drain inside the
`start_tls_consumer` helper (crates/components/camel-component-grpc/tests/
integration.rs:1293, inside a `tokio::spawn` pipeline simulator).
`lint-unbounded-wait` walks only `#[test]` / `#[tokio::test]` function
bodies, so plain helper fns under `tests/` are invisible to the ratchet.
The site escapes the safety intent of the normative scenario "Receive
inside a spawned background task is per-iteration bounded" (openspec
`unbounded-wait-bounding`): the helper is outside the requirement's
lexical scope, but a route producer that never sends parks the pipeline
task until the job-level timeout backstop burns the runner — exactly the
failure class that scenario prescribes a conversion shape for.

Note: the crate moved to `crates/components/camel-component-grpc/` after
mission data was captured; the helper and drain are otherwise unchanged
(grpclik 5bb3dc8f is in the base).

## What Changes

- Convert the single drain site at integration.rs:1293 to the per-iteration
  deadline D-recipe (2 s, matching the file-wide deadline convention):
  `loop { match timeout(.., route_rx.recv()).await { Ok(Some(env)) => ..,
  Ok(None) => break, Err(_) => break } }` (mpsc `recv()` yields `Option`).
- File a follow-up bd for the deferred lint-scope widening (helper fns
  under `tests/`), seeded with the candidate inventory from design.md
  Appendix A and bound by the follow-up-bd contract in design.md.

Excluded: widening `lint-unbounded-wait` scope, any ratchet-ceiling
movement, any spec delta, any production-code change. Path (a) is
rejected on budget evidence — see design.md "Alternatives considered".

## Acceptance criteria

- The drain at integration.rs:1293 is per-iteration deadline-bounded;
  channel-close (`Ok(None)`) still ends the drain; a stall (`Err(_)`)
  ends the loop, dropping the receiver so the next RPC fails observably
  with `Status::internal("pipeline channel closed")`
  (consumer.rs:990-992).
- TLS test trio that exercises `start_tls_consumer`
  (`inbound_tls_handshake_real`, `inbound_mtls_handshake_real`,
  `inbound_mtls_rejects_certless_client`) passes in the grpc integration
  suite (stability gate for the 2 s initial-idle exposure).
- `cargo xtask lint-unbounded-wait` still exits 0 at ceiling 393 (site is
  lint-invisible either way; count unchanged).
- Follow-up bd created (scope widening, discovered-from rc-j27pc) whose
  acceptance criteria cover the design.md follow-up-bd contract: AST-
  derived full inventory, per-site adjudication, spawned-closure policy
  decision, lint scope tests, monotone-ceiling handling.
- Affected crates: camel-component-grpc (tests only).

## Risk budget

Test-support code only; zero production surface. Accepted: a pathological
>2 s producer gap now ends the drain early; the receiver drops and the
affected RPC fails with `Status::internal("pipeline channel closed")`
instead of parking — the intended fail-fast trade. Out of bounds:
touching consumer/server production code, ratchet ceiling changes, spec
text.
