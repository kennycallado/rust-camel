# Proposal: txnreject

## Why

`transactionMode=Managed` in `camel-sql` parses, is documented, and then silently
degrades to Auto: `producer.rs` and `consumer.rs` log
`transactionManager not yet implemented; using Auto mode` and continue. A route
that asks for atomicity and gets autocommit is a flag that parses and is dropped,
the exact class of defect ADR-0090 bans. e_opus ruling 2026-11-15 (bd `rc-qv2iz`
Q4) splits the fix out of `rc-qv2iz`: reject `Managed` at endpoint creation with
a loud, actionable error until the transacted block lands.

bd: `rc-ufjsx` (p1).

## What Changes

- `SqlComponent::create_endpoint` rejects `TransactionMode::Managed` with
  `CamelError::Config` naming the value `Managed`, the endpoint URI, bd
  `rc-qv2iz`, and the replacement: `transactionMode=Auto` plus one
  multi-statement `sql:` step (on statement failure the component rolls the
  connection back and reuses it; it discards the connection only if the rollback
  fails non-benignly or cleanup is cancelled). The endpoint URI is rendered
  secret-redacted (`camel_api::redact::redact_url`): SQL URIs carry credentials
  in `db_url` and the error may reach logs.
- The rule lives in one function, `SqlEndpointConfig::validate_transaction_mode`,
  called from `create_endpoint`.
- Parsing is unchanged: `TransactionMode::from_str` still accepts `"Managed"`;
  the failure is the deliberate reject, not a parse error.
- `SqlEndpoint::new*` constructors are unchanged (they take a config by value and
  return `Self`); they get a short comment pointing at the validation.
- The producer/consumer `Managed` warn branches become unreachable through the
  URI surface. The branches stay byte-identical as safety nets for programmatic
  construction; only the comment above each is replaced by one explaining the
  path is unreachable through the URI surface and why.
- Docs: `docs/src/components/database.md` says the component REJECTS
  `transactionMode=Managed` at load with an error naming `rc-qv2iz`, keeping the
  single-statement workaround. The `TransactionMode` enum doc comment in
  `config.rs` (which today promises a warn + Auto fallback) is corrected to state
  the endpoint-creation rejection and that direct constructors bypass validation.
- Spec: no delta. The mission permits a scenario only where a requirement already
  states Managed behaviour; the conductor checked every capability (`sql-pool`
  has none; `transactionMode` appears only in JMS-scoped specs) and no qualifying
  requirement exists. No capability is created and no existing requirement is
  broadened.

Explicitly excluded: implementing the `transacted` block or any part of
`rc-qv2iz`; the bind family (`rc-0eiqm`, `rc-7e681`); any change to
`bind_kinds.rs`, `producer.rs` query paths or `consumer.rs` bind paths (mission
590 is live).

## Acceptance criteria

- A route declaring `transactionMode=Managed` fails endpoint creation with an
  error naming `rc-qv2iz`.
- The value is still parsed (no unknown-parameter error).
- Existing Auto-mode behaviour is unchanged.
- Test for the reject: the error is `CamelError::Config`, not a parse error.

## Risk budget

Behaviour change is deliberate: routes that previously loaded with a silent
degradation now fail at load. Pre-1.0 compatibility policy (ADR-0090) allows the
break and demands the loud failure. `Auto` and all other parameters are
untouched. Tests need no database: the reject happens at endpoint creation,
before any pool or connection is created.
