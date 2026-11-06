# Design: jms-invalid-property-policy

## Diagnosis (STAGE 1 evidence)

**The hole.** `bridges/jms/src/main/java/org/rustcamel/jms/JmsProducer.java:45-58` iterates
the forwarded header map; line 49 skips denied names; line 51 calls
`msg.setStringProperty(key, entry.getValue())` inside `try { } catch (Exception e)` at
lines 50-57, logging at `Level.FINE` (lines 53-56). `producer.send(msg)` at line 59 then
proceeds and `send` returns `msg.getJMSMessageID()` at line 64 — a success with the
header silently missing. The `ContentType` write at line 43 is NOT wrapped, but its name
is fixed and legal on every broker, so it shares none of the hole.

**Which names are legal, and why.** The JMS spec requires property names to be valid
message-selector identifiers: Java identifier characters (letters, digits, `_`, `$`),
first character a letter (see the in-repo analysis at
`crates/camel-test/tests/support/jms_crash_probe_cases.rs:333-342`). `x-crash-proof`
contains `-` and is therefore illegal. ActiveMQ Classic tolerates such names; the Artemis
JMS client rejects them in `setStringProperty` (empirically proven in the mission 356
Artemis proof, bd rc-ooczk). The legality set is a broker/client property, not a bridge
policy: the bridge must not duplicate it — the provider's own rejection is the source of
truth, and the bridge's job is to stop swallowing it.

**Other property-write paths surveyed.** `grep -rn "setStringProperty|setProperty"`
across `bridges/jms/src/main/java` finds exactly two sites, both in `JmsProducer`
(lines 43, 51). The consumer never writes properties. The inbound copy loop
`JmsConsumer.java:968-975` enumerates broker-accepted names and calls
`b.putHeaders(name, ...)` inside `catch (Exception ignored)` — the same silent-catch
pattern, but on broker-accepted names rather than user input; recorded here as a
follow-up observation, not changed (mission non-goal: journal framing untouched).

**Error-path composition.** `JmsBridgeService.send` (`JmsBridgeService.java:77-96`)
catches any exception from `producer.send` and maps it to a gRPC error with
`e.getMessage()`. The Rust producer surfaces it via the `BRIDGE_TRANSPORT_ERROR_PREFIX`
diagnostic (`crates/components/camel-jms/src/producer.rs:199-206`). A typed exception
thrown from the header loop therefore rides an existing, proven path; the service only
needs to classify it as `INVALID_ARGUMENT`. The `durable-subscribers-v1` capability gate
(`JmsBridgeService.java:39`) is consumer-side only; the new error path neither duplicates
nor touches it. Handler-contract boundary (ADR-0012, CONTEXT-MAP.md): the producer
failure is a `Bridged error`-adjacent producer-path error — the send fails, the route's
error handler owns the operational signal.

## Approach

1. **Typed error.** New `InvalidPropertyException extends JMSException` in the bridge.
2. **Accumulate, then reject.** Keep the loop, order, normalization, and deny/allow gates
   byte-identical. Wrap only the `setStringProperty` call: on failure, collect the key.
   After the loop, if any collection is non-empty, throw
   `InvalidPropertyException(<comma-separated names>)`. Accumulate-all (vs fail-fast) so
   legal names after an illegal one are still attempted — forwarding of legal names is
   position-independent, and the sender sees every offender in one round trip. The
   message carries names only, never values (the exception text is built from the loop
   key, and the caught provider message is not appended: provider text may quote values).
3. **Service mapping.** `JmsBridgeService.send` catches `InvalidPropertyException` first
   and maps to `Status.INVALID_ARGUMENT`; the existing generic `INTERNAL` catch stays for
   everything else. Health/restart semantics unchanged: a rejection is the sender's
   error, not a broker failure — no `BrokerFailureClass`, no probe interaction.
4. **Tests.** Java unit rows (gradle, offline, run here): Mockito-faked `Message` proves
   accumulate-then-reject and the name-only error text; embedded Classic `vm://` broker
   (already a test dep) proves legal-name forwarding stays byte-identical end-to-end.
   CI-owned Artemis row (`crates/camel-test/tests`, `integration-tests` feature per
   ADR-0054, dead-letter-channel assertion idiom): an allowlisted hyphenated header on
   Artemis dead-letters with the property name in the error text and no value; the legal
   control name round-trips. The Artemis row needs Docker plus a built bridge binary and
   is not part of the local gate; the change report names it as CI-owned.
5. **Docs + version.** `bridges/jms/README.md` "Header forwarding" section replaces the
   "This release does not fix that gap" sentence with the rule (legal names, rejection on
   violation, no sanitising). `crates/components/camel-jms/README.md` allowlist row
   points at the rule. CHANGELOG under Unreleased. `bridges/versions.toml` jms → 0.9.1.

## Affected crates

- `bridges/jms` (Java): JmsProducer, new InvalidPropertyException, JmsBridgeService
  mapping, tests, README.
- `crates/components/camel-jms`: README doc row only.
- `crates/camel-test`: new CI-owned Artemis integration row.
- `bridges/versions.toml`, `CHANGELOG.md`.

## Architecture boundaries

Components boundary (CONTEXT-MAP.md): the bridge owns broker contact and its typed
diagnostics (`BrokerFailureClass` precedent); the Rust component owns route-side error
ownership. The DSL/Runtime are untouched. No new Rust API surface: the error arrives as
the existing transport-error diagnostic text.

## Alternatives considered

- **Sanitise/rename with collision fallback** — REJECTED (mission non-goal): a renamed
  property breaks selector-based consumers and the cross-language allowlist contract
  (`HeaderPolicy.normalize` folds `-`/`_`/`.` in the denylist, so `x-a` and `xa` are the
  same allowed name; a sanitised write would also silently merge distinct user headers).
- **Silent downgrade to a warning** — REJECTED: exactly today's behaviour; it is the bug.
- **Java-side pre-validation of name legality** — REJECTED: duplicates the broker's
  legality set (Classic vs Artemis differ); the provider rejection is authoritative and
  the accumulate step already names the offender.
- **Fail-fast on first failure** — REJECTED in favour of accumulate-all: fail-fast makes
  legal forwarding position-dependent and reports one offender per round trip.

Single-phase change; no `## Phases` section.
