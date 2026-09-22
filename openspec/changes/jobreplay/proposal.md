# Proposal: jobreplay

## Why

bd rc-ucemm (P1, retro520 finding). The `camel job` send loop classifies the
SEDA no-active-consumers/subscribers gate as a retryable startup failure
(`is_retryable_startup_failure` in `crates/camel-cli/src/commands/job/mod.rs`).
The gate fires pre-enqueue but INSIDE the caller's pipeline — so when the job's
`send.to` enters a route whose earlier steps have side effects (SQL, file
writes, counters) before a `to(seda:...)`, every 20 ms retry re-executes the
whole pipeline and duplicates those side effects for the full 3 s window.

This violates the settled contract from rc-zjrx / rc-tgaxf (e_gpt ruling,
2026-09-10): the SEDA gate must FAIL FAST at senders that cannot re-run their
pipeline. Six sibling stimulus-delivery sites were fixed in rc-tgaxf; the
camel-cli job classifier was missed — worse, it explicitly includes the gate
as retryable on a false premise ("the job send is the first and only send, so
no side effects can have run" — false whenever `send.to` is a route entry such
as `direct:`).

## What Changes

**Included** (affected crates: `camel-cli`, `camel-component-seda`):

- `is_retryable_startup_failure` excludes the SEDA gate: an
  `EndpointCreationFailed` that IS the gate is non-retryable (fail fast);
  every non-gate `EndpointCreationFailed` stays retryable — the direct
  registration race plus, as a documented residual retained by bd scope,
  SEDA queue-full and bounded enqueue/fanout timeout errors. The classifier
  delegates to the seda crate's owned predicate (`is_direct_startup_race`),
  the same discriminator rc-tgaxf applied at the six fixed sites.
- camel-component-seda producer gate wording fix (discovered during
  implementation, adjudicated scope-A by r_glm): the pre-enqueue gate fired
  the single-mode "has no active consumers" wording BEFORE mode dispatch,
  making the fanout "has no active subscribers" wording unreachable outside
  a deregistration race window — contradicting the crate's own documented
  contract (rc-zjrx/rc-tgaxf two-wording premise) and the spec's fanout
  scenario. The wording now tracks the endpoint mode; classification and
  both predicates are unchanged. Component-local producer-level wording
  pins added (single + fanout).
- Correct the doc comments on the classifier and `send_with_startup_retry`
  that document the false "pre-enqueue-safe" premise.
- Invert the two characterization tests that pin the gate as retryable
  (`seda_single_mode_gate_is_retryable`, `seda_fanout_gate_is_retryable`) to
  pin it as NON-retryable; all other classification outcomes unchanged.
- Adversarial tests proving exactly-once side effects:
  - in-process: real `CamelContext`, route `direct:jobs → mock:counted →
    seda:worker` (no seda consumer), drive `send_with_startup_retry`, assert
    the mock endpoint received exactly 1 exchange and the gate error returns
    without spinning the 3 s window;
  - end-to-end subprocess: `camel job` fixture with `direct:jobs → file
    (append) side effect → seda:worker` (no consumer), assert exit 1, report
    `Failed` with the gate error, and the side-effect file's exact bytes
    equal one body write (`"tick"` — two executions would read
    `"ticktick"`, which a line-count assertion cannot distinguish).

**Excluded**: readiness probing / park-for-later-dispatch (e_gpt ruling chose
fail-fast only); moving the transport-path retry boundary (outer apparatus
errors are pre-pipeline — already side-effect-free); the mid-pipeline direct
registration-race residual (bd explicitly retains direct-race retries;
registration completes during `ctx.start()`, narrowing that window); sibling
stimulus sites (already fixed in rc-tgaxf); any change to the seda gate's
classification predicates or error variants (the wording fix touches
diagnostic text only).

## Acceptance criteria

- A SEDA no-active-consumers (single) or no-active-subscribers (fanout) gate
  error reaching the job send loop returns `SendError::Pipeline` on the FIRST
  attempt — no sleep, no replay.
- Direct `EndpointCreationFailed` registration-race errors remain retryable
  for the full window.
- Adversarial: a side effect placed before a seda send in the job's entry
  route executes EXACTLY ONCE (in-process mock count == 1; subprocess
  side-effect file's exact bytes equal one body write, `"tick"` — a second
  execution would read `"ticktick"`).
- End-to-end job run against a never-activating seda consumer: exit 1,
  outcome `Failed`, error names the gate, side-effect file exact bytes
  prove exactly one execution (timing proof is the in-process test's
  elapsed-around-send bound; subprocess `duration_ms` is boot-dominated
  and unasserted).
- Existing classification outcomes for all non-gate errors unchanged.

## Risk budget

Acceptable: a job sending directly to a never-activating `seda:` endpoint now
fails fast instead of waiting up to 3 s — intended per the fail-fast ruling;
operators see the same diagnosis for single-mode endpoints, and the fanout
wording is now the mode-correct text the crate always documented. Out of
bounds: any change to the seda gate's classification predicates or error
variants; any wording change beyond the mode-correct fanout diagnostic; any
behavior change beyond the job send-loop classification; new retry loops.
