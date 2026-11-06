# Proposal: jms-invalid-property-policy

## Why

The JMS bridge silently drops business headers when the broker refuses a property name.
`bridges/jms/src/main/java/org/rustcamel/jms/JmsProducer.java:50-57` wraps every
`setStringProperty` in the outbound header loop in `catch (Exception e)`, logs at `FINE`,
and sends anyway (`producer.send(msg)` at line 59). On ActiveMQ Artemis a hyphenated,
allowlisted name such as `x-crash-proof` is an illegal JMS property identifier, so the
send reports success while the business header never reaches the broker. Silent data loss
is worse than a rejected send. Discovered in mission 356 task 5.3 and tracked as bd
`rc-ooczk`.

## What Changes

- Replace the swallow-and-continue catch with an accumulate-then-reject policy: a failed
  property write is collected, and the send fails with a typed error naming the offending
  property names (never their values, never host or connection details).
- Legal-name forwarding stays byte-identical: same loop, same normalization, same
  denylist/allowlist gates; only the error path changes.
- `JmsBridgeService.send` maps the typed error to `INVALID_ARGUMENT` instead of the
  generic `INTERNAL`, so senders can distinguish "your header name is invalid" from
  "the broker is broken".
- The consumer-side inbound copy loop (`JmsConsumer.java:968-975`, `catch (Exception
  ignored)`) is recorded in design.md as the same silent-catch pattern, but is NOT
  changed here: it copies broker-accepted names, not user input.
- Docs: `bridges/jms/README.md` header-forwarding section states the rule; the Rust
  component README allowlist row points at it; CHANGELOG entry under Unreleased; bridge
  version bump in `bridges/versions.toml`.

Affected crates/components: `bridges/jms` (Java), `crates/components/camel-jms` (docs
only), `crates/camel-test` (CI-owned Artemis regression row).

## Acceptance criteria

- An allowlisted legal name still forwards and round-trips byte-identically (unit row on
  the embedded Classic broker; existing native rows unchanged).
- An illegal name fails the send; the error text names the property and carries no value
  or host.
- The same case is pinned on the Artemis path via the existing testcontainers fixture
  (CI-owned, `integration-tests` feature, per ADR-0054). Any suite that could not run
  locally is named in the change report rather than claimed.
- `./gradlew --offline cleanTest test` and `spotlessCheck` pass from `bridges/jms`.

## Risk budget

- In scope: producer send-path error handling, docs, tests, version bump.
- Out of bounds (mission non-goals): sanitising or renaming property names, silent
  downgrade to warnings, changes to the durable-subscriber contract or journal framing,
  Artemis-only special casing (one rule on every broker; only the legality set differs,
  and that difference is documented).
- Behaviour risk is bounded: senders who never send illegal names see no change; senders
  who do currently lose data silently and now get a typed rejection.
