# Proposal: jms-pool-config-dedup

## Why

`crates/camel-test/tests/support/jms.rs` carries four ~18-line `JmsPoolConfig`
literals: `shared_jms_activemq`, `shared_jms_artemis`, `shared_jms_artemis_auth`,
and `jms_component_for_broker` (mission 535). All four are the same shape — one
broker keyed `"default"`, `max_bridges: 1`, `tls: None`,
`bridge_start_timeout_ms: 90_000`, rest default — and differ only in URL,
broker type, and credentials. A fifth copy is one test away.
(bd: rc-fpn7p, flagged by the mission 535 holistic review.)

## Builder signature

```rust
fn pool_config(
    broker_url: String,
    broker_type: BrokerType,
    username: Option<&str>,
    password: Option<&str>,
    bridge_start_timeout_ms: u64,
) -> JmsPoolConfig
```

Private to the support module. Deviations from the order's example signature
(`user/pass/reconnect_ms`), both in the direction the order demands
("parameter names that read at the call site"):

- `broker_type` is added: the fixtures span `ActiveMq`/`Artemis`, so the type
  must be a parameter (the bd description's sketch agrees).
- `reconnect_ms` is renamed `bridge_start_timeout_ms`: the field it feeds is
  the bridge **startup** timeout (`90_000` at every site); "reconnect" would
  misname it at every call site.

No positional booleans. Five named parameters stay below the options-struct
threshold.

## Byte-identical guarantee

Every field the literals set today is set to the same value per fixture:

| fixture | url | type | user | pass |
|---|---|---|---|---|
| `shared_jms_activemq` | `shared_activemq()` url | `ActiveMq` | `"admin"` | `"admin"` |
| `shared_jms_artemis` | `shared_artemis()` url | `Artemis` | `None` | `None` |
| `shared_jms_artemis_auth` | `shared_artemis_auth()` url | `Artemis` | `"artemis"` | `"artemis"` |
| `jms_component_for_broker` | `broker.broker_url` | `ActiveMq` | `ACTIVEMQ_USER` | `ACTIVEMQ_PASSWORD` |

All four keep `max_bridges: 1`, `tls: None`, `bridge_start_timeout_ms: 90_000`,
`..JmsPoolConfig::default()`. Test names, assertions, and broker URLs are
untouched.

## `single_broker` verdict (deliverable 3)

`JmsPoolConfig::single_broker` (`crates/components/camel-jms/src/config.rs:1349`)
cannot cover any fixture without behaviour change: it hardcodes
`username/password: None` **and** leaves `bridge_start_timeout_ms` at the
`Default` value `30_000` (`default_bridge_start_timeout_ms`, config.rs:126),
while all four fixtures pin `90_000`. Even the credential-free Artemis fixture
would lose its 90 s startup budget. Left alone.

## Scope

- `crates/camel-test/tests/support/jms.rs` only (no signature crosses a caller:
  all four functions keep `pub async fn ... -> JmsComponent`).
- `jms_credentials_cases.rs` holds a fifth identical literal; it is a separate
  module outside this order's scope and keeps its own copy (bd follow-up).
- Non-goals per order: no new fixtures, no `JmsPoolConfig` change, no
  Artemis/JMS production code.

## Verification

- `git diff` review: four short call sites plus the builder; fixture→field
  table in the park.
- `cargo test -p camel-test --features integration-tests --test jms_test -- --list`
  (compile-only; execution is CI-owned — needs Docker + bridge).
- `cargo fmt --check --all`,
  `cargo clippy -p camel-test --all-targets --features integration-tests -- -D warnings`.
