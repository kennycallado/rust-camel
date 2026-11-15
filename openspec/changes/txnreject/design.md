# Design: txnreject

## Approach

Add `SqlEndpointConfig::validate_transaction_mode(&self, uri: &str) -> Result<(), CamelError>`
in `crates/components/camel-sql/src/config.rs`. On `TransactionMode::Managed` it
returns `CamelError::Config` with a message that names the value `Managed`, the
endpoint URI rendered secret-redacted via `camel_api::redact::redact_url` (SQL
URIs carry credentials in `db_url`; endpoint-creation errors can reach logs),
`rc-qv2iz`, and the replacement pattern: `transactionMode=Auto`
plus one multi-statement `sql:` step (on statement failure the component rolls
the connection back and reuses it; it discards the connection only if the
rollback fails non-benignly or cleanup is cancelled).

`SqlComponent::create_endpoint` (`lib.rs`) calls it right after
`SqlEndpointConfig::from_uri(uri)?`, before catalog resolution and pool
creation, so a Managed route fails at load with no side effects. `create_endpoint`
already returns `CamelError::Config` for other misconfiguration, so the error
family is consistent.

`TransactionMode::from_str` keeps accepting `"Managed"`: the failure must read as
the deliberate reject with guidance, not as "unknown transaction mode".

The `SqlEndpoint::new*` constructors take `SqlEndpointConfig` by value and return
`Self`, so they cannot reject. They get a comment pointing at
`validate_transaction_mode`. The `Managed` warn branches in `producer.rs` and
`consumer.rs` stay byte-identical as dead safety nets for programmatic
construction; only the comment above each branch is replaced by one stating the
path is unreachable through the URI surface and why (endpoint creation rejects
first).

Docs: `docs/src/components/database.md` paragraph on manual multi-statement
transactions changes "is still pending (bd rc-qv2iz) and is not available" into
the reject statement with the same workaround. The `TransactionMode` enum doc
comment in `config.rs` (which promises a warn + Auto fallback) is corrected to
state the endpoint-creation rejection and that direct constructors bypass
validation. The `#[uri_param]` metadata kind `enum:Auto,Managed` is left as
parsed-surface truth.

Spec: no delta. The mission permits a scenario only where a requirement already
states Managed behaviour; no such requirement exists (`sql-pool` covers pool
cleanup, not Managed; `transactionMode` appears only in JMS-scoped specs). No
capability is created and no existing requirement (including dead-config-policy's
serde-compatibility exception) is broadened. This is recorded here as the
spec-level finding.

## Affected crates

- `camel-sql`: `config.rs` (new validation function + corrected `TransactionMode`
  doc comment), `lib.rs` (call the validation in `create_endpoint`),
  `endpoint.rs` (comment only), `producer.rs` / `consumer.rs` (comment only on
  the warn branches).
- `docs`: `src/components/database.md` (Managed paragraph).

## Architecture boundaries

Components layer only. No Runtime, DSL, Services, Languages or Functions
changes. No data/control plane movement: the reject happens during component
wiring, before any data-plane object exists. Respects the data/control plane
split (validation is control-plane configuration handling).

## Alternatives considered

- Reject in `TransactionMode::from_str`: rejected — the error would read as a
  parse failure ("unknown transaction mode") instead of the deliberate,
  documented reject with the `rc-qv2iz` pointer and the workaround.
- Reject in `SqlEndpoint::new*`: rejected — the constructors return `Self` and
  cannot fail; changing their signature is a wider API break than the behaviour
  fix needs.
- Fail at first query/execute: rejected — too late; ADR-0090 requires loud
  failure at load, and a route that loads must be runnable.
- Keep the silent Auto fallback: rejected — that is the defect being fixed.
