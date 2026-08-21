# Proposal: mock-declarative-testkit

## Why

Route authors work YAML-first (`camel run`), but verifying route behavior today
requires writing Rust tests against `camel-test`. The demo workflow pain (CLI +
YAML, no assertion path) is real demand: the 2026-08-18 architect ruling (Q3)
accepted option 3c — a `camel test` test-kit command — precisely because it is
the only inspection channel that respects the architecture (in-process harness,
`MockComponent::get_endpoint` Arc, no IPC, no query-bus surgery). The ruling
deferred this change until #1 landed and demand materialized; both conditions
now hold: change #1 (58e37e53) shipped the assertion API
(`expect_count`, `expect_minimum_count`, `try_assert_satisfied`,
`MockAssertionError`) and the demo needs YAML-driven asserts.

## What Changes

- New `camel test` subcommand in `camel-cli` (commands/test.rs, following the
  lint.rs `Outcome` pattern).
- A declarative **test document** format (`*.test.yaml`): references route
  files (or embeds inline routes), optional `inputs:` (send body/headers to a
  from-endpoint via producer), and mandatory `expects:` blocks per mock
  endpoint mapped onto change #1's expectation API.
- Test-runner semantics: exit 0 = all pass, 1 = assertion failure, 2 = CLI
  misuse / document or route parse errors (matches `camel lint` convention);
  per-expectation result lines + summary.
- Docs: README section for `camel test` + one example test document.

## Acceptance Criteria

- `camel test demo.test.yaml` boots the routes in-process, drives declared
  inputs, evaluates expects via `try_assert_satisfied`, and exits with the
  contract above.
- Route files stay pure: `camel run` ignores test documents; no DSL schema
  change to route YAML.
- The ruling falsifier is served: asserts are mandatory in YAML and the demo
  path needs no Rust code.
- Frozen invariant held: no IPC, no RuntimeBus/QueryBus changes; assertions go
  through `get_endpoint` Arcs.

## Risk Budget

- Low blast radius: additive subcommand; camel-cli only; no runtime/component
  changes.
- Main risks: settle-time flakiness (mitigated by quiet-period polling, not
  fixed sleeps) and boot-path duplication with `camel run` (mitigated by a
  minimal test boot that registers only test-relevant components).
