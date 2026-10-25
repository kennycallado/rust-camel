# Proposal: 409-jms-unknown-params

## Why

`JmsEndpointConfig::from_uri` drops unknown query parameters silently
(`crates/components/camel-jms/src/config.rs:507`, `_ => {}`). A typo such as
`?durableSubscribtion=news` is accepted, the endpoint boots, and the parameter
keeps its default — the worst failure mode for the durable-subscription work
(rc-c4fbo): the route looks healthy and is not durable. Hard-erroring on
unknown parameters would be breaking, so the accepted minimum is a
non-breaking diagnostic that names the parameter, suggests the closest known
key when one is near, and states that the parameter was ignored.

## What Changes

- `from_uri` collects unknown query-parameter **keys** (never values) in
  source order into a new `unknown_params: Vec<String>` field on
  `JmsEndpointConfig`. Bare `=`-less query segments keep today's skip
  behavior; duplicate occurrences are preserved.
- Known keys (camelCase and snake_case aliases) are matched by a gate-first
  check against one authoritative key table adjacent to the match arms;
  every existing parameter behaves exactly as today, including error cases
  and last-write-wins duplicates.
- `create_endpoint` (component.rs) emits one `tracing::warn!` per endpoint
  that has unknowns: the unknown keys, a closest-known-key suggestion when
  the normalized edit distance is small ("no close match" otherwise), and an
  "ignored" hint. Keys and suggestions only — never values, never the URI.
- One sentence added to the JMS component docs about the warning.

Excluded: strictness knob, error-on-unknown, durable-subscription parameters
(rc-c4fbo M2 owns them), Java/bridge changes, version bump.

## Acceptance criteria

- A typo (`durableSubscribtion`) is reported once with the "ignored" hint and
  the correct close-match verdict (for this key: "no close match" — no
  durable parameters exist yet); a true near-miss (e.g. `concurrentConsumerss`)
  receives the canonical-key suggestion.
- A known alias (`concurrent_consumers`) reports nothing.
- Several unknowns are reported in source order, in one warning.
- A secret-shaped value on an unknown key is never echoed: only the key is
  stored or logged.
- `cargo test -p camel-component-jms --lib` green; broker-free tiers of
  `cargo test -p camel-component-jms` green (exact tiers recorded in the park
  report).
- `cargo fmt --check`, `cargo clippy -p camel-component-jms --all-targets --
  -D warnings`, `cargo xtask lint-log-levels`, `cargo xtask lint-log-redaction`
  pass.

## Risk budget

- In bounds: additive struct field, one warning call site, new tests.
- Out of bounds: changing any known parameter's semantics/default, failing
  boot on unknowns, echoing values, pre-empting rc-c4fbo parameter design.
- The `unknown_params` field is additive; a workspace-wide audit must confirm
  no struct-literal construction of `JmsEndpointConfig` exists outside
  `from_uri` (additive fields break exhaustive literals).

Bd: rc-d8svj
