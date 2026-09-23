## ADDED Requirements

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

## MODIFIED Requirements

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
