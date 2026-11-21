# Partition: sqltrans family → slices

Source: `openspec/changes/sqltrans/specs/sql-transactions/spec.md` @ `448e91421`
(frozen). Slices: S1 = `sqltrans-portmodel` (this change), S2 = `sqltrans-runtime`
(rc-qv2iz.6), S3 = `sqltrans-modes` (rc-qv2iz.7).

**verbatim: no requirement reworded.** The four S1 requirement blocks in this
change's delta spec are copied byte-for-byte from the frozen source; slice
ownership notes are prose appended after each requirement's last scenario,
outside the SHALL statement and scenario text itself.

| # | Requirement | Slice | Verified |
|---|---|---|---|
| 1 | Transacted route block surface | S1 | ok |
| 2 | Lazy begin and block-owned commit/rollback | S2 | ok |
| 3 | Cancellation cutoff | S2 | ok |
| 4 | Handled joined DB errors are rollback-only | S2 | ok |
| 5 | Join semantics for sql steps on the block datasource | S2 | ok |
| 6 | Load-time validation is fail-closed and typed | S1 | ok |
| 7 | Transaction mode semantics | S3 | ok |
| 8 | Busy block session | S2 | ok |
| 9 | Block timeout | S2 | ok |
| 10 | Uncertain finalization detaches the connection | S2 | ok |
| 11 | Pool exhaustion is bounded and named | S2 | ok |
| 12 | Bind path invariance inside joined steps | S2 | ok |
| 13 | TransactionManager port and registry resolution | S1 | ok |
| 14 | idempotent consumer composition | S2 | ok |
| 15 | ADR and operational documentation | S1 + S2 (see note) | corrected |

Every requirement is owned exactly once, with req 15 deliberately split:
its ADR/docs-skeleton and unit-tier load-rejection coverage land in S1, its
operational note and docker-suite runtime scenarios land in S2 (see note 1).
No requirement is dropped, added, or reworded.

## Verification notes

1. **Req 15 split (the one correction).** The owner map assigns req 15 wholly
   to S1 with the parenthetical "(ADR + docs skeleton; S2 carries the
   operational note)". Taken literally, req 15 also demands the docker-suite
   runtime scenarios (`camel-test/tests/sql_test.rs`) and those are
   unreachable in S1: mission boundaries forbid docker tests in this slice,
   and the runtime scenarios exercise behavior that does not exist until S2.
   Correction recorded: S1 delivers ADR 0101 + docs skeleton + the unit-tier
   load-rejection coverage; S2 delivers the operational note, the docker-suite
   scenarios, and req 14's disposition tests. The delta spec states this
   split in its ownership note under req 15. Requirement text itself is
   untouched.
2. **Req 7 → S3 is load-bearing.** Making `Managed` join-required before a
   join exists (S2) would open a silent auto-commit hole (e_opus). S1 keeps
   today's endpoint-creation rejection (`txnreject` tests green, unmodified)
   and the Auto-fallback warnings.
3. **Req 1 scenario 1 vs darkness.** "YAML transacted block with two sql steps
   loads" completes end-to-end only when a manager exists (S2). S1 proves the
   model equivalence it asserts (YAML/builder/canonical → identical step
   model) at the parse/compile layer; the dark test proves the unregistered
   path fails with `ManagerMissing`. Recorded as a deferral, not a rewrite.
4. **Req 13's "implemented by camel-sql over sqlx"** is satisfied by the port
   surface being implementation-ready; the camel-sql implementation itself is
   S2 deliverable (stated in the delta ownership note).
