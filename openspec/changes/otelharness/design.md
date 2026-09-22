# Design: otelharness

## Approach

Two mechanical steps, ordered so each lands green:

1. **Sibling move.** Delete the inline `#[cfg(test)] mod tests { ... }`
   (service.rs lines 560–1280) and place its body verbatim in a new
   `src/service_tests.rs`, wired at the old sampler-wiring site as:

   ```rust
   #[cfg(test)]
   #[path = "service_tests.rs"]
   mod tests;
   ```

   The module tree is unchanged (`service::tests`), so `use super::*;`
   resolves identically and test paths stay `camel_otel::service::tests::*`.
   Precedent: `51a7d49f` (camel-api `metrics_tests.rs`) and the existing
   `#[cfg(test)] #[path = "sampler_tests.rs"] mod sampler_tests;` at
   service.rs:42–44 in this crate.

2. **Harness extraction.** Inside `service_tests.rs`, add a private helper:

   ```rust
   fn bounded_repro<F, Fut>(tag: &str, regression: &str, body: F)
   where
       F: FnOnce() -> Fut,
       Fut: std::future::Future<Output = ()>,
   ```

   It owns the channel/thread/`catch_unwind` scaffold: spawns a thread named
   `{tag}-repro`, builds a dedicated current-thread runtime, `block_on`s
   `body()`, reports panics through the channel with the existing
   `&str`/`String` downcast, and asserts completion via
   `recv_timeout(Duration::from_secs(60))` with the existing panic texts
   ("repro thread failed (not a hang): …" / "repro thread did not finish:
   stop() hung ({regression} regression)"). Both stall tests shrink to their
   arrange/act/assert bodies inside the closure; the metric repro calls
   `bounded_repro("q74u", "rc-q74u", …)`, the span repro
   `bounded_repro("q6ju71", "rc-6ju71", …)` — thread names and diagnostics
   byte-identical to today.

Behavior-preservation oracle: the baseline `cargo test -p camel-otel -- --list`
(91 tests, captured at 6b110708) must diff empty after each step; the suite
(including both stall repros) must stay green. A future logs-path repro
reuses `bounded_repro` instead of copying the scaffold a third time.

## Affected crates

- `crates/services/camel-otel`: `src/service.rs` (delete inline test mod, add
  3 wiring lines, ~1280 → ~565 lines), `src/service_tests.rs` (new, tests +
  shared harness), `CONTEXT.md` (layout note; the ADR-0012 line anchors are
  stale today — the annotated `error!` sits at 377 and the Drop-impl `warn!`
  at 534, both shifting +3 — Task 1.3 re-measures rather than trusts
  arithmetic).

No other crate changes — `#[cfg(test)]` items are invisible to dependents.

## Architecture boundaries

Services-layer only; no Runtime/DSL/Components/Languages/Functions surface
changes. The data/control plane boundary is untouched (no production code
path edited). Relevant decisions: ADR-0007 (graceful provider shutdown — the
invariant these stall repros guard), ADR-0012 (log-policy annotations whose
CONTEXT.md anchors shift), ADR-0049 (non-exhaustive posture — unaffected,
no public enum changes).

## Alternatives considered

- **`tests/` integration dir:** rejected — the tests exercise
  `service::tests`-private items (`STATUS_STARTED`, private fields) and must
  stay a unit-test child of the `service` module.
- **Harness in a shared `test_support` module:** rejected for now — only one
  file hosts repros today; promote to a crate-visible test-support module
  when a second file needs it (YAGNI).
- **Keep inline, extract harness only:** rejected — leaves `service.rs` at
  ~1.2k lines; bd item 2 explicitly asks for the sibling move.

## Phases

Single-phase — omitted (no `## Phase N` headings in tasks.md).

### Self-grill record

**Questions generated:**
1. [glossary] Does "bounded_repro" / "bounded-stop stall repro" conflict with
   existing camel-otel CONTEXT.md terminology (ADR-0007 graceful shutdown)?
2. [sharpen] The design names the tag as `q6ju71` (not `6ju71`) while the bd
   id is `rc-6ju71` — does `{tag}-repro` produce the verbatim thread name?
3. [scenario] After `#[path]` move, does `use super::*;` still resolve the
   `service`-private items the tests touch (`STATUS_STARTED`, private fields)?
4. [cross-ref] Do the two scaffold copies and the ADR-0012 anchors match the
   claimed source lines, and is the public-API-invariant real?

**Answers (with citations):**
1. [glossary] No conflict. CONTEXT.md ADR-0012 table and ADR-0007 note carry
   no `bounded_repro`/harness term; `bounded-stop` is descriptive, not a
   redefined glossary concept (`CONTEXT.md:64-74`). Confirm.
2. [sharpen] Verbatim-preserving. Source thread names are `q74u-repro`
   (`service.rs:1047`) and `q6ju71-repro` (`service.rs:1219`). Design passes
   `bounded_repro("q6ju71", "rc-6ju71", …)` → `{tag}-repro` = `q6ju71-repro`
   and `{regression}` = `rc-6ju71`. The tag intentionally drops the `rc-`
   prefix; correct as written (`design.md:38-42`).
3. [scenario] Resolves identically. `#[path]` changes only the file backing
   the module, not the module tree; `mod tests` stays `service::tests`, so
   `use super::*;` (`service.rs:562`) still reaches `service`. Proven by the
   existing sibling `sampler_tests.rs` using `use super::OtelService;`
   (`sampler_tests.rs:1`). Confirm.
4. [cross-ref] Verified: `mod tests` = lines 561–1280, `#[cfg(test)]` at 560
   (`service.rs:560-561`); scaffold patterns
   `mpsc::channel::<Result<(), String>>` and
   `recv_timeout(Duration::from_secs(60))` each occur exactly 2× today
   (grep count = 2/2); no `service::tests` reference exists outside the file
   (repo-wide grep = none); ADR-0012 anchors at 288/~430 (`CONTEXT.md:73-74`).
   Public-API-invariant holds (all items `#[cfg(test)]`-gated). Confirm.

**Outcome:** confirm — all four techniques resolved against cited source; no
drift, no scope creep, oracle is sound.
**Self-grill mode:** self-grill-proposals skill
