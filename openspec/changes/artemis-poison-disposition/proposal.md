# Proposal: artemis-poison-disposition

## Why

Mission 356 Task 5.4 proved the poison contract with durable evidence: six
reservations/rollbacks, one terminal `COMMIT`, one native `ERROR` record. On
Artemis, the broker-side destination was **not observed** — the fixture exposes
no management API, so the case recorded an explicit `UNKNOWN` gap (bd
rc-5c6yv). The crash-proof claim rests on client-observed behaviour only. The
durable journal cannot close this gap: its `RCJN` frames record the bridge's own
reservations, not what the broker did with the message (a `COMMIT` frame proves
the bridge acked, not that the broker applied it — async ack, bd rc-bzqiz — and
says nothing about DLQ/expiry routing).

*Resolution (2026-11-07, mission 517): closed by this change — the fixture now
exposes the broker-side queue-stat query and the Artemis disposition is
asserted broker-side; `rc-5c6yv` is resolved on landing.*

## What Changes

- Add a stable broker-side disposition query to the dedicated Artemis fixture
  (`ArtemisBroker`): a `docker exec` into the container that runs the
  instance's own `bin/artemis queue stat --queueName <q>` and parses the
  pinned 2.36.0 table into counts (`MESSAGE COUNT`, `MESSAGES ADDED`) for the
  source queue, `DLQ`, and `ExpiryQueue`.
- Wire the query into `jms_bridge_redelivery_disposition_artemis`: the
  assertion names the broker-side outcome (acknowledged-and-removed, or
  native dead-letter/expiry evidence) instead of the current unobserved gap
  note. The six durable reservations/rollbacks and the terminal
  `ACK`/`COMMIT` assertions stay unchanged — additive evidence, not a rewrite.
- Document the mechanism, its stability argument for the pinned image, and
  what the query does **not** prove (test-support docs).
- Explicitly rejected alternatives: adding a browse/management op to the
  bridge (out of scope), Jolokia (this image deploys no console wars), and
  journal-derived inference (client-side evidence).

Affected crates: `camel-test` (test support + integration cases only). No
production code changes.

## Acceptance criteria

- The Artemis disposition test fails if the broker-side query is removed or
  returns "unknown" — the query is load-bearing, not decorative.
- The existing poison assertions (durable frames, terminal record, no payload
  leak) pass unchanged.
- Docs state which mechanism backs the query, why it is stable for
  `apache/activemq-artemis:2.36.0-alpine`, and the limits of the evidence.
- `cargo test -p camel-test --all-targets` and the mission gates pass.

## Risk budget

Test-only change; no delivery-semantics, journal-format, or poison-policy
changes (non-goals). Accepted risks: the query shells out to the docker CLI
(precedent: `docker pause` in the crash probe cases) and to CLI args for the
fixture credentials (test-only `artemis`/`artemis`). If the pinned image
layout changes, the fixture's query fails loudly, never silently claims a
disposition.
