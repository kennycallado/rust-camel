# Design: jobreplay

## Approach

Replace the job send loop's retry classification with the seda crate's owned
discriminator, exactly as the rc-tgaxf sweep did for the six sibling
stimulus-delivery sites:

```rust
fn is_retryable_startup_failure(e: &CamelError) -> bool {
    camel_component_seda::is_direct_startup_race(e)
}
```

`is_direct_startup_race` (camel-component-seda, landed rc-zjrx) is
`!is_no_active_consumers_gate(e) && matches!(e, EndpointCreationFailed(_))`:
every `EndpointCreationFailed` is retryable EXCEPT the SEDA gate wordings.
The gate is structurally indistinguishable by position — it fires inside the
producer's `call()` future, so whether it rejected the job's own entry send or
a `to(seda:...)` deep inside the entry route's pipeline, it arrives as the
same inner pipeline `CamelError`. Only exclusion is safe; readiness probing
was explicitly ruled out (e_gpt, rc-tgaxf: fail-fast only).

Why not move the retry boundary instead: the transport path (outer `Err` of
`attempt_send` — producer/endpoint apparatus creation) already replays only
apparatus creation, never the pipeline; the pipeline path is the only replay
hazard, and its classification is what this change fixes. No boundary move
needed.

Comment corrections: `is_retryable_startup_failure`'s doc and
`send_with_startup_retry`'s doc both state the false premise ("a gate
rejection is pre-enqueue and the job send is the first and only send, so no
side effects can have run"). They are rewritten to the fail-fast contract
with rc-ucemm as the reference.

Test strategy (three layers):

1. **Characterization inversion** (`startup_retry_classification_tests.rs`):
   `seda_single_mode_gate_is_retryable` and `seda_fanout_gate_is_retryable`
   flip to `..._is_not_retryable`; direct-race, generic
   `EndpointCreationFailed`, and every non-retryable case stay unchanged.
2. **In-process adversarial** (new `#[cfg(test)]` module in the job command,
   driving `send_with_startup_retry` directly): real `CamelContext` with
   `direct` + `mock` + `seda` components, route `from direct:jobs → to
   mock:counted → to seda:worker`, NO seda consumer route, ctx started.
   Assert: returns `SendError::Pipeline` whose error satisfies
   `is_no_active_consumers_gate`; the mock endpoint recorded EXACTLY one
   exchange; elapsed well under the 3 s window (fail-fast, no 20 ms spin).
3. **Subprocess e2e** (camel-cli integration test, real `camel` binary):
   fixture route `from direct:jobs → to file:{dir}?fileName=count.txt&
   fileExist=append → to seda:worker` with body "tick"; no seda consumer.
   Assert: exit 1, report outcome `Failed`, error names the gate, and
   `count.txt`'s EXACT bytes equal `"tick"` (one execution; two would
   read `"ticktick"`, which a line-count cannot distinguish). The
   fail-fast timing proof lives in layer 2, where elapsed wraps
   `send_with_startup_retry` directly; the subprocess report's
   `duration_ms` is boot-dominated and deliberately NOT asserted.
   File append is the subprocess-observable
   side-effect counter (`mock:` is in-memory and unreadable across the
   process boundary — same observation shape as
   `batch_typed_arg_coerces_and_drains`).

## Affected crates

- `camel-cli`: `src/commands/job/mod.rs` (classifier + two doc comments),
  `src/commands/job/startup_retry_classification_tests.rs` (two inversions +
  the `seda_queue_full_is_retryable` residual pin + helper doc updates +
  header comment), new in-process adversarial test module, new/extended
  integration test.
- `camel-component-seda` (scope amendment, adjudicated mid-implementation):
  the pre-enqueue gate's rejection wording now tracks the endpoint mode —
  pre-fix it fired the single-mode text before mode dispatch, making the
  documented fanout wording unreachable (latent contract violation, root
  cause of the fanout-scenario gap). One guard in `SedaProducer::call`
  (lib.rs ~1091); predicates, classification, and error variants unchanged;
  two component-local producer-level wording pins added.

## Architecture boundaries

- **Components**: camel-component-seda's classification contract is
  UNTOUCHED — both predicates (`is_no_active_consumers_gate`,
  `is_direct_startup_race`) and the error variant are exactly as rc-zjrx
  landed them; the single amendment is the mode-aware DIAGNOSTIC WORDING at
  the pre-enqueue gate, which brings the emitted text into compliance with
  the crate's own documented two-wording contract.
- **Runtime/DSL**: untouched. No route semantics, DSL, or context changes.
- **CLI**: the change is confined to the job command's send-phase error
  classification — a boundary the `cli-jobs` spec domain already governs.
- **Error taxonomy**: no new variants; the mode-correct fanout diagnostic
  is the sole wording change (predicates and variant classification per
  rc-fr20u doctrine: variant carries the class, seda owns its message
  text).

## Residual risk (documented, accepted)

The variant-based classification retains as retryable ALL non-gate
`EndpointCreationFailed` errors — not only the direct registration race:
SEDA queue-full ("SEDA queue '…' is full") and bounded enqueue/fanout
timeout errors share the variant and also fire mid-pipeline (post
side effects), so retrying them replays the pipeline too. bd rc-ucemm's
scope excludes only the no-active-consumer/subscriber gates ("retaining
direct registration-race retries"); the sibling residuals stay documented
here for a follow-up if the owner wants the exclusion widened. The direct
race window itself is startup-only (consumers register during
`ctx.start()`, before the send loop begins), narrower than the SEDA gate
(which can fire for the entire 3 s window — no consumer exists).
A job sending DIRECTLY to a never-activating `seda:` endpoint now fails fast
instead of waiting 3 s — intended per the fail-fast ruling.

Position-indistinguishability (as observed by the error-only classifier):
the classifier sees only the returned `CamelError`, never where it fired,
so a gate rejection at the job's own entry send and one from a
`to(seda:...)` deep inside the entry route's pipeline are
indistinguishable — only exclusion is safe at this boundary.
