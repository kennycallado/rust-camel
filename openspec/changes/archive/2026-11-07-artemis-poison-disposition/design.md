# Design: artemis-poison-disposition

## Approach

**Chosen mechanism: the broker's own `artemis queue stat` CLI, run inside the
container via `docker exec`.**

The fixture calls
`docker exec <container> /var/lib/artemis-instance/bin/artemis queue stat
--url tcp://localhost:61616 --user artemis --password artemis --queueName <q>`
and parses the pinned 2.36.0 table into `QueueStat { message_count,
messages_added }`. The parser is fail-closed two ways:
`run_with_deadline(command) -> Result<Output, String>` bounds each exec (child
polled with `try_wait` until the deadline, killed and reaped on expiry —
regression-tested with a sleeping child, no docker needed), and
`parse_queue_stat(stdout, queue) -> Result<Option<QueueStat>, String>`
distinguishes a well-formed filtered table without the row (`Ok(None)`,
documented as unreachable for this scenario) from malformed headers or
non-numeric counts (`Err`). The observation layer treats `Ok(None)` for the
scenario's expected queues (source, `DLQ`, `ExpiryQueue`) as a hard failure,
never as zero counts — absence must not establish acknowledged-removal. The
overall observation carries its own bounded wait (async-ack drain), so no
read can hang the case.

| source count | DLQ added | Expiry added | disposition |
|---|---|---|---|
| 0 | 0 | 0 | acknowledged and removed (bridge terminal policy) |
| 0 | >0 | 0 | broker-native dead-letter (`DLQ`) |
| 0 | 0 | >0 | expired (`ExpiryQueue`) |
| >0 | — | — | still queued / inconclusive |

Stability argument (pinned `apache/activemq-artemis:2.36.0-alpine`):
- The CLI ships **inside the container** — tool and broker are version-matched
  by construction. Verified empirically **on this exact image pin**:
  `queue stat --queueName` filters to the row, the table shape is
  deterministic, and `DLQ`/`ExpiryQueue` anycast queues exist by default with
  `dead-letter-address=DLQ` under `match="#"`. The claim is scoped to this
  verified pin, not extrapolated to other Artemis 2.x versions.
- No web console dependency: this image's instance deploys **no** console wars
  (bootstrap.xml declares them; the `web/` directory is absent), so Jolokia —
  the Classic fixture's mechanism — is unavailable here. No extra port is
  exposed.
- `docker exec` from the test harness has precedent (`docker pause`/`unpause`
  in `jms_crash_probe_cases.rs`).

**Wiring.** `ArtemisBroker` gains `queue_stat(queue) -> Result<QueueStat, String>`
(sync `Command`, mirroring the existing `docker()` helper, under the exec
deadline) plus a pure
`parse_queue_stat(stdout, queue) -> Result<Option<QueueStat>, String>`
(`Ok(None)` only for a well-formed filtered table without the row —
documented as unreachable for this scenario because all three queues exist;
`Err` for malformed headers or non-numeric counts). The observation layer
fails hard on `Ok(None)` or `Err` for the scenario's expected queues.
`observe_destination`'s Artemis branch polls with a bounded wait (async ack
drain, bd rc-bzqiz) and returns `Some(ArtemisDestination { .. })` with a
`describe()` naming the outcome. `classify_precedence` gains the Artemis
numeric branch (native-first only with DLQ evidence; never from a missing
`COMMIT`). `record_and_report_disposition`'s BridgeCapFirst arm asserts
`acknowledged_and_removed()` for Artemis and replaces the `GAP 5.4` note with
real evidence. The test `expect`s the `Some(destination)` — removal or failure
of the query fails the test, making it load-bearing. Existing Classic
assertions and `assert_threshold_contract` are untouched.

**Docs.** `artemis.rs` module/method docs carry the mechanism + stability
argument; a camel-test test-support README section records what the query
does **not** prove: point-in-time management counts under the isolated
dedicated-container scenario only (not journal layout, not byte persistence,
not a universal proof against message loss), no crash-proofness claim,
`MESSAGES ADDED` accumulates over the broker lifetime (exact here because
each test owns a fresh dedicated container). The two terminal kinds are kept
distinct: the bridge's terminal discard is an *acknowledgement* (source
drains, zero `DLQ` adds), while a broker-native dead-letter *moves* the
message to `DLQ` (non-zero `MESSAGES ADDED` there); with this image's default
addressing (`dead-letter-address=DLQ` under `match="#"`) a silently dropped
message has no route — the counts always distinguish acknowledgement from
dead-letter routing. The test credentials travel as CLI args (visible in
`ps` inside the container) — test-only `artemis`/`artemis`, accepted.

## Affected crates

- `camel-test`: `tests/support/artemis.rs` (query + parser + docs),
  `tests/support/jms_disposition_cases.rs` (observation struct, Artemis
  branch, wiring, focused synthetic unit cases for the parser and
  classifier), test-support docs section.

## Architecture boundaries

Test-support only. The bridge, its delivery semantics, the journal format,
and the poison policy are untouched (mission non-goals). No production crate
changes; no new public API outside the camel-test support module.

## Alternatives considered

- **Journal-derived inference** (mission's first check): rejected — RCJN
  frames are the bridge's own reservation ledger; client-side evidence cannot
  establish broker-side destination state.
- **Jolokia over the web console** (Classic parity): rejected — the pinned
  alpine image deploys no console wars; there is no Jolokia agent to talk to.
- **Bridge browse/management op** (queue browser or core management
  messages): rejected — extends the bridge surface; a non-goal.
- **`docker exec` + `artemis data print`**: rejected — reads the journal
  store, a much larger and less stable surface than one queue-stat table.
