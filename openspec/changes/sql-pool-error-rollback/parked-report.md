# Parked Report: sql-pool-error-rollback

All tasks in this change are complete. The change parks with one recorded
limitation and one informational finding outside the blessed spec scope.

## Limitation: no deterministic fault injection for a live rollback failure

The spec's discard branch (an open non-benign rollback failure) is not
deterministically forceable against a healthy SQLite connection with the
existing test harness. The harness has no fault-injection seam, so a live
rollback failure cannot be produced on demand.

The discard path is covered indirectly by unit tests:

- `classify_db_message_is_fail_closed` — classifier returns `Keep` only for
  the exact benign string, `Discard` for everything else (fail-closed).
- `rollback_error_classifier_rejects_non_database_errors` — classifier rejects
  non-database errors as `Discard`.
- `cleanup_guard_discards_when_armed_keeps_when_disarmed` — the armed guard
  drops the connection (discard), the disarmed guard returns it (keep). This
  is exactly the branch cancellation and rollback-failure paths use.

## RED/GREEN evidence

RED (task 1.1, pre-wiring):

```
test producer::tests::failing_manual_begin_does_not_poison_the_pool ... FAILED
assertion `left == right` failed
  left: Some(Number(3))
 right: Some(Number(2))
```

RED (task 1.2, pre-wiring):

```
test producer::tests::stream_list_error_releases_connection_and_pool_stays_usable ... FAILED
assertion `left == right` failed
  left: Some(Number(3))
 right: Some(Number(2))
```

Both RED results show the same failure mode: a leaked open transaction is
visible through same-connection pool reuse (max_connections=1).

GREEN (after wiring): `cargo test -p camel-component-sql --lib` →
294 passed; 0 failed. The poison and stream tests are green with ROW_COUNT 2.

Preservation/retention tests (`failed_statement_without_transaction_keeps_error_and_pool_usable`,
`failing_select_keeps_error_and_pool_usable`) passed already on unwired code.
They guard message preservation and keep-not-discard behavior, not the rollback
wiring. Recorded here honestly: they were not RED.

## Gate sweep

`scripts/gates-local.sh` ran from the worktree under the systemd scope
`fleet-sqltxpool`: exit 0.

- passed: 29
- failed: 0
- skipped: 4 (`lint-commits`, `lint-bridge-releases`, `doc-guide-test`,
  `cargo-audit` — network/docs-toolchain gates owned by CI)
- ran: 29 of 33

## Erratum

The blessed `tasks.md` acceptance commands said `-p camel-sql`. The actual
package is `camel-component-sql` (`crates/components/camel-sql/Cargo.toml`).
Task commands were executed with the real package name. The text erratum is
corrected in `tasks.md` in this commit and recorded in `.bless.json` errata.

## Informational: out-of-scope cancellation gaps

These findings are informational and outside the blessed spec scope. The
blessed spec deliberately scopes cancellation safety to cleanup in flight.
Two other cancellation windows can still re-pool a connection with an open
transaction:

- Cancellation between connection acquire and cleanup start.
- Mid-stream consumer abandonment (dropping `StreamBody` mid-iteration).

Filed as bd rc-2ljv5 (bug, P2, discovered-from: rc-en1ud).
