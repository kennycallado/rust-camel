# unbounded-wait-bounding Specification

## Purpose
Deadline bounding for test waits. The `lint-unbounded-wait` detector requires
awaits on channels, locks, and global test mutexes in test bodies to be
deadline-bounded, and analyzes inline closures and helper functions in test
trees.
## Requirements
### Requirement: Deadline-bounded channel receives in test bodies

Every `.recv().await` on an asynchronous channel inside a `#[test]` / `#[tokio::test]`
function body (including receives inside `async move {}` blocks passed to `tokio::spawn`,
which are in test-body AST scope) SHALL be deadline-bounded at the call site — via
`tokio::time::timeout` / `timeout_at` future-argument containment, a per-iteration deadline
inside a loop, or the shared `camel_component_api::test_support::timeout_wait` helper — or
carry an `// allow-test-wait:` marker with a site-specific justification. Once the
mission-394 inventory is fully adjudicated, the ratchet ceiling SHALL equal the
unadjudicated finding count per the ratchet zero end-state requirement; the historical
arithmetic of prior partial burns (548 − 117 = 431) is superseded.

<!-- openspec-scenario-ops
renamed: Ratchet counts only unadjudicated receives -> Ratchet counts only unadjudicated receives at the zero end state
-->

#### Scenario: Single expected receive is timeout-wrapped

- **GIVEN** a test that awaits `rx.recv().await` once and asserts on the value
- **WHEN** the site is converted
- **THEN** the receive is bounded (as the future argument of
  `tokio::time::timeout(deadline, rx.recv())` or via `timeout_wait`), the `Elapsed`
  case fails the test with a message naming the wait and the deadline, and the
  channel-closed case is preserved as a distinct failure

#### Scenario: Drain loop in the test body uses a per-iteration deadline

- **GIVEN** a `while let Some(v) = rx.recv().await` drain loop directly
  in a test function body
- **WHEN** the site is converted
- **THEN** the loop exits on channel-close, and a receive that stalls
  past the deadline fails the test (a silent early exit on stall is not
  accepted)

#### Scenario: Receive inside a spawned background task is per-iteration bounded

- **GIVEN** a `tokio::spawn(async move { .. rx.recv().await .. })`
  background drainer or pipeline simulator in a test body
- **WHEN** the site is converted
- **THEN** each receive iteration is individually deadline-bounded (an
  overall timeout on the whole task is not used, because the task
  legitimately spans the whole test), a stall ends the task's loop, and
  any pre-existing `expect` chain inside the task that carries
  test-alive semantics is preserved with the timeout wrapper so a stall
  still fails observably

#### Scenario: Marker use requires justification

- **GIVEN** a receive site where a deadline is semantically wrong
- **WHEN** the site is adjudicated instead of bounded
- **THEN** the source line carries `// allow-test-wait:` followed by a
  non-empty reason naming why a deadline is wrong for that wait, and
  the conversion log lists the site with that justification

#### Scenario: Ratchet counts only unadjudicated receives at the zero end state

- **GIVEN** `lint-unbounded-wait` reports the workspace unadjudicated
  finding count and every mission-394 inventory site is bounded or
  marker-adjudicated
- **WHEN** the ceiling is lowered to that count
- **THEN** `cargo xtask lint-unbounded-wait` exits 0 at the lowered
  ceiling, and the ceiling is never raised afterward

### Requirement: Loop-wait enclosure by wrapping timeouts only

`lint-unbounded-wait` SHALL suppress a loop finding only when every
wait-class await site in the loop body is individually bounded by a
timeout that wraps that site: (a) the site's span lies inside the
future-argument region of a bounding timeout call
(`tokio::time::timeout` / `timeout_at`, resolved through the
alias/glob candidate machinery), (b) the site's base expression is
itself such a bounding call (per-iteration deadline), or (c) the
site's base is a local whose only binding in the loop subtree is
`let x = <bounding call>`. An "await site" is a classified wait: an
awaited call in the wait-method class (recv, lock, acquire, wait,
join_next, connect), an awaited free-fn call resolving to the
spawn/connect call targets, an awaited spawned-handle binding, an
awaited directly-invoked closure with a wait-class tail, or an
opaque (unresolvable) await base. Await sites inside inline closure
bodies (directly invoked and awaited, or synchronous and directly
invoked) count as loop-subtree sites; closures merely passed as
arguments or stored contribute no sites. Out-of-class awaits are
not sites: awaited calls resolving to the finite-call targets
(`tokio::time::sleep`, `tokio::task::yield_now`) are dropped,
awaited directly-invoked closures whose tail is not a wait-class
call are dropped, and out-of-class method awaits (`send`,
`notified`, `on_next`, `accept`, `read`, `write`, `flush`) stay
deferred to a later detector revision — a loop whose only awaits
are out-of-class is not reported. A bounding call that does not
wrap a site — a sibling statement, a disjoint branch, a
closure-internal call, or a dropped never-awaited binding — SHALL
NOT bound that site; if any site in the loop is unbounded, the
loop SHALL be reported (line of the `loop` keyword) and contained
wait findings stay subsumed.

#### Scenario: Dropped sibling timeout does not bound loop waits

- **GIVEN** a test-body loop containing
  `let _ = tokio::time::timeout(d, async { .. }); rx.recv().await;`
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** the loop is reported at the `loop` line, because the
  timeout's future region encloses only the async block and the
  binding is never awaited

#### Scenario: Timeout in a disjoint branch does not bound other branches

- **GIVEN** a test-body loop where one branch awaits
  `tokio::time::timeout(d, a()).await` and a sibling branch awaits
  `rx.recv().await`
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** the loop is reported, because the branch-local timeout does
  not wrap the other branch's receive

#### Scenario: Per-iteration await-on-timeout stays bounded

- **GIVEN** a test-body loop of the shape
  `loop { match tokio::time::timeout(d, rx.recv()).await { .. } }`
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** no finding is emitted, because each site's base expression
  is the bounding timeout call itself

#### Scenario: Awaited local timeout binding stays bounded

- **GIVEN** a test-body loop containing
  `let f = tokio::time::timeout(d, rx.recv()); f.await;` with `f`
  bound exactly once in the loop subtree
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** no finding is emitted for the loop, because the awaited
  local's sole binding is a bounding call

#### Scenario: Rebound local timeout does not bound the await

- **GIVEN** a test-body loop containing
  `let f = tokio::time::timeout(d, rx.recv()); let f = other(); f.await;`
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** the loop is reported, because `f` is bound more than once
  in the loop subtree, so the sole-binding provenance rule does not
  apply

#### Scenario: Timeout inside a spawned closure does not bound the loop

- **GIVEN** a test-body loop whose await site is a plain
  `rx.recv().await` while some `tokio::spawn(..)` inside the loop
  awaits a timeout in its own closure body
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** the loop is reported, because the closure-internal timeout
  runs in another task and does not wrap the loop's site

#### Scenario: Await in an unenclosed async block is reported

- **GIVEN** a test-body loop whose await site sits inside a bare
  `async { rx.recv().await }` block that is NOT inside any timeout
  future region, while a sibling statement carries a timeout call
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** the loop is reported, because an async block is not itself
  a deadline; only region enclosure, await-on-timeout, or sole-binding
  provenance bound a site

#### Scenario: Awaited inline closure in a loop reports the loop

- **GIVEN** a test-body loop whose only await is
  `(|| async { rx.recv().await })().await;`
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** the loop is reported once at the `loop` line, because
  the unrolled closure body contributes an unbounded site and the
  wrapper await is not itself a wait site

#### Scenario: Finite-tail inline closure in a loop is not a site

- **GIVEN** a test-body loop whose only await is
  `(|| tokio::time::sleep(d))().await;`
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** no finding is reported, because the awaited closure's
  tail is a finite call and the loop has no wait-class site

#### Scenario: Ratchet verdict recorded on count movement

- **GIVEN** the detector change may surface loop findings previously
  hidden by unrelated timeouts in member trees
- **WHEN** `cargo xtask lint-unbounded-wait` runs after the fix
- **THEN** the count is exact-green at the current ceiling (393), or
  the ceiling is raised only together with an inventoried list of the
  new findings adjudicated as true positives, or the ceiling is
  lowered together with the recorded inventory of every finding the
  change removed (each removal classified as fixed, bounded in-tree,
  or reclassified out-of-class)

### Requirement: Inline closure bodies in the unbounded-wait scan

`lint-unbounded-wait` SHALL analyze a closure body as part of the
enclosing test-function body when the closure executes in the test
function: (a) the closure is the callee of a directly-awaited call
(any parenthesized closure — sync closure returning an async block,
`async ||` closure, or sync closure returning a bare future), or
(b) the closure is synchronous (not `async ||`, body not an async or
try block) and directly invoked, so its statements run at call time.
All other closures — passed as arguments, stored in bindings,
returned, or executed inside spawned work — SHALL stay pruned from
the scan, and waits inside them SHALL NOT be reported. Bounding,
marker suppression, and loop subsumption SHALL apply to unrolled
closure bodies unchanged, because all of them operate on source
spans.

#### Scenario: Directly invoked and awaited closure is unrolled

- **GIVEN** a test body containing
  `let v = (|| async { rx.recv().await })().await;`
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** the inner receive await is reported at its line, because
  the await drives the closure body inline in the test body

#### Scenario: Direct call without await stays pruned

- **GIVEN** a test body containing
  `let f = (|| async { rx.recv().await })();` and no await of `f`
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** no finding is reported, because the future is never
  polled and the body never executes

#### Scenario: Closure passed as an argument stays pruned

- **GIVEN** a test body that passes a closure with an interior
  await to a helper call, as `helper(|r| async move { r.recv()
  .await });`
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** no finding is reported, because the closure body runs
  where the helper drives it, not in the test body

#### Scenario: Nested directly-invoked closure reports once

- **GIVEN** a test body containing
  `(|| async { (|| async { rx.recv().await })().await })().await`
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** exactly one finding is reported for the inner receive
  await, because both closure bodies unroll and the outer awaits
  are wrapper calls, not wait sites

#### Scenario: Sync directly-invoked closure reports blocking waits

- **GIVEN** a sync test body containing `(|| { rx.blocking_recv()
})();`
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** the blocking receive is reported, because the closure
  body executes at the call site

#### Scenario: Inline closure in a timeout future argument stays pruned

- **GIVEN** a test body containing
  `let _ = tokio::time::timeout(d, (|| async { rx.recv().await })
()).await;`
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** no finding is reported, because the closure call is an
  argument to `timeout` and never directly awaited, so the closure
  stays pruned and contributes no site

#### Scenario: Unrolled inline closure site inside a timeout region stays bounded

- **GIVEN** a test body containing
  `let _ = tokio::time::timeout(d, async { (|| async { rx.recv()
.await })().await }).await;`
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** no finding is reported, because the directly-awaited
  inline closure unrolls inside the timeout future region and the
  receive site lies inside that region

#### Scenario: Timeout inside an awaited inline closure bounds it

- **GIVEN** a test body containing
  `(|| async { let _ = tokio::time::timeout(d, rx.recv()).await;
})().await;`
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** no finding is reported, because the closure unrolls and
  its only await is on a bounding timeout call that drives the
  receive with a deadline

#### Scenario: Inline closure inside spawned work stays pruned

- **GIVEN** a test body containing `tokio::spawn(|| { (|| async {
rx.recv().await })().await; });`
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** no finding is reported, because the inline closure is
  only reachable through the spawned closure, which stays pruned

#### Scenario: Awaited inline closure in a loop reports the loop

- **GIVEN** a test-body loop containing
  `(|| async { rx.recv().await })().await;`
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** the loop is reported once at the `loop` line with the
  inner site subsumed, because the unrolled site is unbounded

### Requirement: Awaited inline-closure tail drives a wait site

`lint-unbounded-wait` SHALL report an awaited directly-invoked
closure whose tail expression (the stripped closure body, or the
final expression statement of a block body) is itself a wait-class
call — a method in the wait-method class (`recv`, `lock`,
`acquire`, `wait`, `join_next`, `connect`, `spawn`) or a free-fn
call resolving to the wait or spawn call targets — because the
await drives that future inline while the body contains no inner
await to unroll. Awaited directly-invoked closures with an
async-block body or a non-wait tail SHALL NOT be reported at the
outer await; their bodies are analyzed through the unroll rule
above, and loop-site classification SHALL agree with this rule.

#### Scenario: Bare-future tail awaited inline reports the await

- **GIVEN** a test body containing `let v = (|| rx.recv())()
.await;`
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** the await is reported, because the closure body builds
  the receive future and the outer await drives it inline

#### Scenario: Finite tail awaited inline is not a wait site

- **GIVEN** a test body whose only await is
  `(|| tokio::time::sleep(d))().await;`
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** no finding is reported, because the awaited closure's
  tail is a finite call and the await cannot park the test

### Requirement: Deadline-bounded global test-lock acquisition

Every acquisition of a process-global test-serialization mutex
(`REGISTRY_TEST_LOCK`, `SERVER_MUTEX`, `TEST_MUTEX`,
`SENTINEL_TOPOLOGY_LOCK`, `TOPOLOGY_LOCK`, `ACK_TEST_LOCK` — empty
`tokio::sync::Mutex<()>` statics whose guards are held for the
duration of a test) SHALL acquire through the shared
`camel_component_api::test_support::acquire_deadline` helper. All
inventoried sites convert; this requirement admits no
`allow-test-wait` adjudication for these six locks.

#### Scenario: Mechanical conversion preserves guard shape

- **GIVEN** a test body containing `let _guard = LOCK.lock().await;`
- **WHEN** the site is converted
- **THEN** the site reads `let _guard = acquire_deadline(&LOCK, "<LOCK> <family>", TEST_LOCK_DEADLINE).await;` (helper imported from `camel_component_api::test_support`), and the guard's drop semantics are unchanged

#### Scenario: Stalled holder fails the waiting test, not the binary

- **GIVEN** a peer test holds the lock and stalls (deadlock or
  crashed peer under the guard)
- **WHEN** a queued test's acquisition exceeds the deadline
- **THEN** the waiting test panics with a message naming the lock,
  the acquisition site, and the deadline, instead of parking
  indefinitely

#### Scenario: Deadline exceeds worst healthy queue, not one holder

- **GIVEN** lock families whose tests serialize on one mutex inside
  a single test binary (44/23/18/6/3/3 holders), with worst-case
  healthy holder chains up to 602 s under load
- **WHEN** the deadline is chosen
- **THEN** a single uniform `TEST_LOCK_DEADLINE` of 900 s covers
  every family's worst-case healthy queue with ~1.5× margin, so a
  timeout indicates a stalled holder, not queue depth

#### Scenario: One helper, no per-crate reimplementations

- **WHEN** any crate needs bounded test-lock acquisition
- **THEN** it uses `camel_component_api::test_support::acquire_deadline`; no parallel helper is introduced in any crate

#### Scenario: Lint sources and ratchet stay untouched

- **WHEN** the 97 inventoried sites are converted
- **THEN** `scripts/xtask/src/lint_unbounded_wait.rs` and
  `scripts/xtask/ratchet-unbounded-wait.max` are not modified
  (the drainscope mission owns them), and
  `cargo xtask lint-unbounded-wait` findings only decrease

#### Scenario: Helper timeout path is observable and sleep-free

- **GIVEN** the helper's unit tests with a spawned holder that
  acquires the lock and awaits a pending future
- **WHEN** `acquire_deadline` with a short deadline is called
- **THEN** it panics naming the lock, the deadline, and the
  acquisition call site (verified by capturing the panic payload),
  and the tests use no `tokio::time::sleep` (lint-test-sleep clean)

### Requirement: Helper-fn bodies in the unbounded-wait scan

`lint-unbounded-wait` SHALL scan non-test function bodies for
unbounded waits when the function is lexically inside a file under a
`tests/` directory (a path component named exactly `tests`) or
inside an inline module annotated `#[cfg(test)]` (including nested
inline submodules thereof). Non-test helpers in out-of-line
`#[cfg(test)] mod X;` module files are a declared false-negative
class (each file parses standalone). Findings for `#[test]` /
`#[tokio::test]` function bodies SHALL be identical before and after
the widening.
Closures that do not execute in the scanned body (spawned work,
callbacks) SHALL stay pruned inside helper fns, mirroring the
test-body rule. The ratchet ceiling SHALL stay monotone across the
widening: it may not increase without a review-justified decision,
and every newly-visible site SHALL be adjudicated (bounded in-tree,
`// allow-test-wait:` marker with site-specific justification, or a
named ceiling entry) — no silent sites.

#### Scenario: Helper fn under a tests/ directory is scanned

- **GIVEN** a non-test `async fn helper` containing an unenclosed
  `rx.recv().await` in a file whose path contains a `tests` directory
  component
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** the receive site is reported as an unbounded-wait finding

#### Scenario: Non-test fn outside tests/ and outside cfg(test) stays invisible

- **GIVEN** a non-test `fn helper` containing an unenclosed wait in a
  `src/` file with no `#[cfg(test)]` ancestor module
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** the helper body produces no finding (production code is
  out of scope; only test-attributed fns report there)

#### Scenario: Non-test fn inside an inline cfg(test) module is scanned

- **GIVEN** a `#[cfg(test)] mod tests` (inline body) in a `src/` file
  containing a non-test `async fn helper` with an unenclosed wait
- **WHEN** `lint-unbounded-wait` scans the file
- **THEN** the helper's wait site is reported, and nested inline
  submodules of the `#[cfg(test)]` module are scanned the same way

#### Scenario: Test-fn findings unchanged by the widening

- **GIVEN** the workspace finding list for test-attributed fn bodies
  before the widening (296 sites at main d9ac1ca7)
- **WHEN** the widened scanner runs over the same tree
- **THEN** every pre-existing test-fn finding is still reported at
  the same file:line, and the widened scope only adds findings from
  non-test fn bodies

#### Scenario: Spawned-closure bodies inside helper fns stay pruned

- **GIVEN** a helper fn under `tests/` that passes a closure
  containing `rx.recv().await` to `tokio::spawn` without awaiting the
  call inline
- **WHEN** `lint-unbounded-wait` scans the helper
- **THEN** the closure body produces no finding (the closure runs in
  its own task scope; binding-indirection kin stays tracked in bd
  rc-eow0s)

#### Scenario: Ceiling monotone across the widening

- **GIVEN** the pre-widening ceiling 296 in
  `scripts/xtask/ratchet-unbounded-wait.max` and the full inventory
  of newly-visible helper-fn sites recorded in the change's
  design.md Appendix B
- **WHEN** every inventoried site is adjudicated (bounded, marker, or
  named ceiling entry)
- **THEN** the ceiling equals 296 minus any net reduction, or — only
  for genuinely unreachable sites — one review-justified increase
  recorded in the park notes, and `cargo run -p xtask --
  lint-unbounded-wait` exits 0 at the resulting ceiling

#### Scenario: Global test-lock acquisition in a helper is deadline-bounded

- **GIVEN** a helper fn under `tests/` acquiring a process-global
  test-serialization lock via `LOCK.lock().await`
- **WHEN** the site is converted
- **THEN** the acquisition goes through
  `camel_component_api::test_support::acquire_deadline` with
  `TEST_LOCK_DEADLINE` so a stalled holder fails the waiting test
  with a named lock and site instead of wedging the binary
  (precedent c2a48f20, bd rc-88old)

### Requirement: Shared bounded test-wait helper timeout_wait

`camel_component_api::test_support` SHALL provide a single shared bounded-wait helper
`timeout_wait(what, deadline, fut)` that drives `fut` under `tokio::time::timeout`, and every
crate that needs to bound a one-shot test wait SHALL prefer it over per-site
`tokio::time::timeout` boilerplate; no parallel bounded-wait helper is introduced in any
crate. On deadline elapse the helper SHALL panic naming `what` (the wait under test), the
deadline, and the call site (`#[track_caller]`) — preserving the established guarantee that
a converted wait's failure names the wait and the deadline — and the inner future's output
SHALL pass through unchanged so a channel-closed outcome remains distinct from a hang at the
call site. Adoption of the helper SHALL NOT require changes to
`scripts/xtask/src/lint_unbounded_wait.rs`: a wait rewritten as
`timeout_wait(what, d, expr).await` leaves the lint's wait-class scan because the awaited
expression is the helper call, not a wait-class call.

#### Scenario: Hung wait fails the test loudly

- **GIVEN** a test body containing
  `timeout_wait("orders receive", Duration::from_secs(30), rx.recv()).await`
  where no value is ever sent
- **WHEN** the deadline elapses
- **THEN** the test panics with a message naming `what`, the deadline, and the
  `timeout_wait` call site, instead of parking indefinitely

#### Scenario: Channel closure stays distinct from a hang

- **GIVEN** a test whose bounded receive returns `None` because the sender was dropped
- **WHEN** the test matches on the received value
- **THEN** the `None` is observed as the inner output and asserted like any other value,
  not conflated with the deadline path

#### Scenario: One helper, no per-crate reimplementation

- **WHEN** any crate needs to bound a one-shot test wait
- **THEN** it uses `camel_component_api::test_support::timeout_wait`; a second shared
  bounded-wait helper anywhere in the workspace is a spec violation

#### Scenario: Helper adoption needs no lint modification

- **GIVEN** a flagged unbounded receive rewritten to
  `timeout_wait("orders receive", d, rx.recv()).await` with no other edit
- **WHEN** `cargo xtask lint-unbounded-wait` scans the file
- **THEN** the finding is gone and `scripts/xtask/src/lint_unbounded_wait.rs` is unmodified

### Requirement: Ratchet zero end-state after full adjudication

After every site in the mission-394 inventory (299 findings on main 3dfada1a7) is either
bounded in-tree or marker-adjudicated with a construction-citing reason, the ratchet ceiling
in `scripts/xtask/ratchet-unbounded-wait.max` SHALL be lowered to 0 and
`cargo xtask lint-unbounded-wait` SHALL report 0 findings and exit 0 at that ceiling. Zero
means: no unadjudicated DETECTED sites — waits outside the lint's documented scan scope are
unchanged by this requirement and remain governed by the existing scan-scope rules. The
ceiling SHALL move monotonically down from 296; it SHALL NOT be raised. The ceiling file
SHALL carry a header comment documenting marker legitimacy: a marker is legitimate only
where the wait is bounded by construction (test-owned uncontended lock, test-fed channel,
join of a task whose every progress dependency is bounded or purely in-memory), never to
mask a wait that could hang.

#### Scenario: Ceiling records the burned count and the gate is green

- **GIVEN** all inventoried sites are bounded or marker-adjudicated
- **WHEN** the ceiling file is set to 0 and `cargo xtask lint-unbounded-wait` runs
- **THEN** the lint reports 0 findings and exits 0 at that ceiling

#### Scenario: A new unbounded test wait fails the gate

- **GIVEN** the ceiling is at the burned count
- **WHEN** a change adds a test wait that is neither bounded nor marker-adjudicated
- **THEN** the lint reports the site and fails, and it passes only after the site is
  bounded or adjudicated

#### Scenario: Marker documentation lives in the ceiling header

- **GIVEN** the ceiling file's header comment
- **WHEN** a contributor considers adjudicating a site
- **THEN** the header states the construction-based legitimacy rule for
  `// allow-test-wait:` markers

