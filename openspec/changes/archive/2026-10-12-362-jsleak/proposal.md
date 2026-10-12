# Proposal: 362-jsleak

## Why

The `js` language treats any new global property as integrity drift and
rebuilds the whole Boa realm. Scripts that create a top-level name (`var y =
1`, `function f(){}`, `async function f(){}`, `z = 1`) therefore rebuild the
realm on every evaluation, and Boa 0.22 never frees the previous realm:
~310KB/eval (6.2GB over 20k evals; the reported 3MB/s at ~10 evals/s). A
second, smaller retention: promise jobs enqueued by `.then` are never drained
(~7.5KB/eval). The existing steady-state test covers only `camel.body`, so
neither leak is pinned.

Bd: rc-jo85f. Evidence: KB `js-leak-research-2026-10-11` (empirical). The same
probes confirmed every added key (`var`/`function`/undeclared/`async
function`) is CONFIGURABLE, that the next eval's scrub yields `typeof
undefined`, that 20k evals stay flat at 17–31MB RSS, and that 20k promise jobs
are dropped. 359 (ddc6ce7d) is exonerated; the realm rebuild predates it.

## What Changes

- Integrity drift ignores CONFIGURABLE global additions — the pre-eval scrub
  already deletes them. Non-configurable additions, vanished baseline keys,
  and an unreadable added-global descriptor still recycle.
- The worker's job executor discards enqueued promise jobs instead of
  retaining them. `script:` stays synchronous (ADR-0006); callbacks still do
  not run.
- Debug diagnostics on integrity drift and realm recycle, without logging
  script source, exchange data, or user-controlled property names/values.
- Regression tests: repeated `var`/`function`/undeclared scripts keep the
  recycle counter at zero; repeated `.then` scripts drop exactly N jobs with
  zero recycles, re-checked after a forced recycle.

Excluded: wrapper-shape changes, Boa version bumps, running microtasks, and
metrics infrastructure.

## Acceptance criteria

- Repeated evals of `var y = 1; y`, `function f(){}; 0`, `async function
  f(){}; 0`, and `z = 1; z` do not increment the recycle counter, and a
  following `typeof y`/`typeof z` is `'undefined'`.
- `Object.defineProperty(globalThis,'pwn',{configurable:false})` and
  `Object.freeze(Object.prototype)` still recycle.
- N repeated `Promise.resolve(1).then(() => {}); 0` evals make the discarding
  executor's dropped-job count — read via `downcast_job_executor` — equal N,
  with zero recycles; the sequence repeats with zero extra recycles after a
  forced (non-configurable) recycle. Queue depth alone is not evidence.
- Memory flatness is confirmed by the empirical probe (20k evals, 17–31MB),
  not by unit assertions.
- `camel.body` steady-state remains flat; the realm changes only on a real
  recycle. Drift and recycle emit debug diagnostics with no script source, no
  exchange data, and no user-controlled key/value.

## Risk budget

Accept: configurable globals persist until the next eval's scrub (already
true); promise callbacks do not run (already true); and a script that now
scrubs-and-reuses instead of recycling keeps its untracked state within the
realm generation, exactly as existing `camel.body`-style scripts already do —
the declared non-guarantee, with no widened isolation promise. Reject: weaker
non-configurable enforcement (including fail-open on an unreadable
descriptor), running microtasks, wrapper widening, or logging script/value
data. Boa stays 0.22.0.
