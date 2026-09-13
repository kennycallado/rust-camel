# Design: bootfreshness

## Approach

Treat named shared-cache SQLite lifetime as a teardown contract, not a timing
assumption. The scenario boot and SQL pool drain implementation already landed
in `efd92d6f` and `41028d20`; this change does not re-implement them. It adds
coverage and hardening around the existing seam. If implementation changes are
needed, preserve the landed contract: the catalog close future is awaited and
the SQL adapter prevents in-memory pool resurrection while observing the
drain state. Any timeout-policy change must explicitly account for the
existing warning-only scenario teardown behavior.

The regression fixture uses a per-test URI name, reused by boot A and boot B.
This prevents unrelated tests in the same test process from keeping the
database alive, while preserving the exact sequential-boot assertion. The
test remains backed by the production SQL pool and real boot composition root.

The determinism argument is: boot shutdown awaits the catalog close future;
SQL close disables resurrection, awaits pool close, and observes every pool
connection drain; only after that future resolves can boot B create its pool.
No wall-clock sleep is used as a state signal. A bounded polling deadline is
only a failure bound for an observable `pool.size()` condition.

The harness-isolation ruling records the mechanism behind that argument: a
sqlx named shared-cache memory database stays alive exactly while any
connection to it is open, so boot B can only observe boot-A rows if a boot-A
connection outlived the awaited shutdown. The landed pool close closes that
window deterministically — it prevents in-memory pool resurrection, awaits
pool close, and waits for the observable drain of every pooled connection.
The fixture URI is unique to its test, so no sibling test can hold the alias
open across the boot A teardown window.

## Affected crates

- `camel-integration-test`: unique named-memory fixture and regression
  evidence/documentation.
- `camel-core`: verify the already-landed datasource catalog close boundary.
- `camel-component-sql`: verify the already-landed SQLite pool close semantics
  and change them only if regression evidence requires it.

## Architecture boundaries

This change stays at the integration-test boot seam and datasource adapter.
`camel-core` coordinates lifecycle teardown through the existing datasource
port; it does not inspect SQL internals. The SQL component owns pool-specific
drain behavior. The route data plane is unchanged, and no DSL, API contract,
or component registry surface is added. The design follows the lifecycle and
testing boundaries described by ADR-0069 and the named-memory isolation
contract established alongside ADR-0064.

## Alternatives considered

- Adding a fixed sleep after shutdown was rejected because it assumes load
  behavior and is the parent epic's prohibited sleep-as-sync pattern.
- Switching the fixture to a file-backed database was rejected because it
  hides the in-memory lifetime defect and changes the behavior under test.
- Serializing the whole test binary was rejected because it masks unrelated
  shared-state contamination instead of proving datasource teardown.

## Root-cause classification

This is a new SQLite named-memory lifetime/teardown class. It is not the JWKS
cooldown wall-clock class (`rc-7dfyq`), the camel-http consumer readiness
class, or the macOS errno class (`rc-62me6`). A unique URI is fixture
hardening against sibling-test contamination, not the established root cause.
