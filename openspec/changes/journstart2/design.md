# Design: journstart2

## Approach

Three coordinated changes in `camel-core/src/lifecycle`:

1. **Boot-unique command IDs (the fix).** `next_context_command_id`
   (`context_lifecycle.rs`) becomes `next_context_command_id(boot_nonce, op,
   route_id)` emitting `context:{op}:{route_id}:{nonce}:{seq}` (five
   segments). The three call sites (`:116` start, `:169` stop, `:239`
   abort-stop) already hold `&RuntimeBus`. The per-process
   `CONTEXT_COMMAND_SEQ` stays: it only needs uniqueness within a boot once
   the nonce scopes the boot.

   **Nonce derivation (journal-derived, deterministic):** at
   `RuntimeBus::ensure_journal_recovered` (`runtime_bus.rs:162-174`), after
   `uow.recover_from_journal()`, compute

   `nonce = 1 + max(P)` where `P` = for every recorded command ID, the
   penultimate colon-separated segment, taken only when that ID's final two
   segments both parse as `u64` (`0` when no ID qualifies)

    and store it by changing `journal_recovered_once: OnceCell<()>` to
    `OnceCell<u64>`. The IDs are already loaded by
    `InMemoryRuntimeStore::recover_from_journal` (`in_memory.rs:481`), so the
    store computes the nonce in one place and exposes it through a defaulted
    `RuntimeUnitOfWorkPort` method `recovered_boot_nonce() ->
    Result<u64, DomainError>` (`runtime_ports.rs:105-128`; default `Ok(0)`).
    The error is reserved for the exhausted-space boundary below and its
    message names the offending recorded ID.

    **Boundary policy (exhausted value space):** if `max(P) == u64::MAX` —
    only reachable by a caller deliberately recording an adversarial ID such
    as `context:start:r:18446744073709551615:0`, never organically — the
    deterministic rule cannot produce a strictly greater nonce. The boot
    FAILS CLOSED: `recovered_boot_nonce` returns an explicit startup error
    that names the offending recorded ID and tells the operator to clean or
    rotate the journal. `ensure_journal_recovered` propagates the error at
    the first `register_route`, and no context command ID is issued for
    that boot. There is no fallback nonce and no wall clock anywhere. Every
    standing guarantee stays unconditional: determinism, no wall clock, and
    no issued ID ever equals a recorded ID — the boot refuses to issue
    rather than risk suppression, because failing loudly beats silent
    suppression.

   **Why tail-scan max+1:** a candidate ID
   `context:{op}:{route_id}:{nonce}:{seq}` can equal a recorded ID only if
   that ID's final two segments are exactly `{nonce}:{seq}` — which makes
   `nonce` a collected (forbidden) penultimate value, and the chosen nonce
   is strictly greater than all of them. The tail scan needs no prefix or
   segment-count parsing, and it covers every hazard: new-format IDs (their
   penultimate segment is their nonce), legacy four-segment IDs for routes
   whose IDs contain colons (`context:start:foo:0:0` for route `foo:0`
   yields forbidden nonce `0`), and future mixed histories. The raw count
   alternative regresses when `forget_seen` (`in_memory.rs:405-412`) removes
   a failed command's ID — a later boot can re-derive an earlier nonce while
   that boot's ID is still recorded. Tail-scan max+1 is airtight against
   every recorded ID regardless of which boot wrote it. Replay ordering is
   safe: recovery runs at the first `register_route`, before
   `start_context` issues any command.

   **No-journal case:** the context builder installs a unit-of-work
   regardless (`context_builder.rs:213-223`); without a journal, recovery
   loads no durable IDs, the nonce stays `0`, and lifecycle outcomes are
   unchanged (the ID string does gain the `0` nonce segment, which is safe
   because nothing parses command IDs — see item 3 below).

2. **No-silence guard (gh#52 requirement).** In `start_context` after the
   StartRoute loop (`context_lifecycle.rs:111-120`):
   - `RuntimeCommandResult::Duplicate` for a StartRoute → `warn!` naming
     route and command ID (the exact bug signature).
   - Belt-and-braces sweep: for each `auto_startup_route_ids()` entry whose
     `GetRouteStatus` is not `Started` after the loop → `warn!`.
   `auto_startup = false` routes are excluded by construction (never in
   `auto_startup_route_ids()`); genuine start failures already return `Err`
   and fail the boot loudly. Severity is WARN, not an error: the nonce fix is
   primary prevention, the guard is the backstop that makes any future
   suppression visible.

3. **Format consumers.** Verified by repo-wide sweep: no code parses the ID
   (dedup keys are opaque strings; all `split(':')` hits are URI parsing);
   no live doc or spec names the format; `journal inspect` displays events
   only. Only the generator and its three call sites change, plus its doc
   comment.

## Tests

- Regression (acceptance): in `crates/camel-core/tests/runtime_journal_test.rs`
  (drop-before-reopen pattern of `accepted_command_id_survives_restart`):
  boot 1 on a tempdir journal → assert `Started` and journal sequence
  `RouteRegistered → RouteStartRequested → RouteStarted`; drop context; boot
  2 and boot 3 on the same journal → same assertions every boot, plus each
  boot's shutdown `StopRoute` is accepted and appends `RouteStopped`
  (stop IDs were suppressed on boots 2+ too). Red before the fix, green
  after.
- Nonce unit tests: two recoveries over identical journal state derive the
  same nonce; a journal with recorded tail-numeric IDs derives a nonce
  strictly greater than every recorded penultimate value; a journal seeded
  with legacy four-segment IDs — including a route ID containing colons —
  yields a nonce that avoids all of them and boots without suppression.
- Boundary test (fail-closed): a journal seeded with a recorded ID whose
  penultimate segment is `u64::MAX` (for example
  `context:start:r:18446744073709551615:0`) fails the boot with an explicit
  error naming that ID; no context command ID is issued.
- Guard tests: duplicate-suppressed StartRoute produces a WARN naming the
  route; registered-but-not-started after the loop produces a WARN;
  `auto_startup = false` stays silent.

## Affected crates

- `camel-core` only (`lifecycle/application/context_lifecycle.rs`,
  `lifecycle/application/runtime_bus.rs`,
  `lifecycle/application/ports/runtime_ports.rs`,
  `lifecycle/adapters/in_memory.rs`, tests). No API, DSL, component, or
  journal-schema changes.

## Architecture boundaries

Change stays inside the lifecycle application layer: the nonce crosses the
`RuntimeUnitOfWorkPort` boundary as a defaulted method (hexagonal boundary
test guards the direction); the ID format is internal to the lifecycle
command path; the guard lives in `start_context` beside the existing warn
patterns. The durable dedup semantics (`first_seen` / `forget_seen`) are
untouched — commands remain idempotent within a boot and across restarts
once IDs are boot-scoped.

## Phases

Single-phase change (three tasks: nonce fix + guard + tests are one coherent
delivery; no subsystem split is warranted).

## Alternatives considered

- **Wall-clock boot nonce (PR #53 baseline):** correct but
  non-deterministic — same journal state yields different IDs across runs,
  so journal diffs are not reproducible. Rejected; the fail-closed boundary
  removes the need for any fallback.
- **Raw count of recorded command IDs as nonce:** rejected — validated
  counterexample under `forget_seen` (count regresses, later boot re-derives
  an earlier nonce while that boot's ID is still recorded → collision).
- **New `RuntimeEvent::BootMarker`:** strictly monotonic and journal-native,
  but touches serde, replay, compaction, and `journal inspect` display —
  too much surface for this fix; the max-alive+1 nonce achieves the same
  guarantee without a schema change.
- **Boot failure instead of WARN in the no-silence guard:** rejected for
  now — changes exit behavior for deployments already hitting the bug; the
  issue asks for a signal, and WARN provides it without turning dark routes
  into failed boots. (Distinct from the nonce boundary policy above, which
  does fail the boot: exhausted nonce space has no correct ID left to
  issue, while a suppressed-but-usable route still warrants a warning.)
