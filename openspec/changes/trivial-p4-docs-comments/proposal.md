# trivial-p4-docs-comments

Docs/comment accuracy fixes from the p4 backlog (rc-mgki, rc-4u9w, rc-bdof).

## Why

Trivial change — no spec breakdown needed.

## What changes

- `crates/components/camel-http/CONTEXT.md`: note PEM re-read cadence — TLS material is read at client build, pinned clients cached for `PINNED_CLIENT_TTL` (60 s), so edited PEMs take effect at most once per TTL window; producer cert-rotation unsupported (ADR-0004 is consumer-side).
- `crates/services/camel-redis-repo/CONTEXT.md`: note concurrent refreshes collapse to a single rebuild via the executor's single-flight connect gate.
- `crates/camel-core/src/lifecycle/adapters/consumer_management.rs`: correct two watcher comments — pre-first-poll abort drops the sender unfired (reachable, not "cannot happen"); the TerminationGuard covers every termination mode after the first poll.
- `openspec/changes/archive/2026-08-26-explicit-task-watcher/design.md`: normal-path stop() Err is silently discarded, not debug-logged (matches `let _` at the call site).
