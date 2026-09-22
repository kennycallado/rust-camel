# unbounded-wait-bounding Specification

## Purpose
TBD - created by archiving change bound-bare-recv-waits. Update Purpose after archive.
## Requirements
### Requirement: Deadline-bounded channel receives in test bodies

Every `.recv().await` on an asynchronous channel inside a `#[test]` /
`#[tokio::test]` function body (including receives inside `async move {}`
blocks passed to `tokio::spawn`, which are in test-body AST scope) SHALL
be deadline-bounded at the call site — via `tokio::time::timeout` /
`timeout_at` future-argument containment or a per-iteration deadline
inside a loop — or carry an `// allow-test-wait:` marker with a
site-specific justification.

#### Scenario: Single expected receive is timeout-wrapped

- **GIVEN** a test that awaits `rx.recv().await` once and asserts on the
  value
- **WHEN** the site is converted
- **THEN** the receive is the future argument of
  `tokio::time::timeout(deadline, rx.recv())`, the `Elapsed` case fails
  the test with a message naming the wait and the deadline, and the
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

#### Scenario: Ratchet counts only unadjudicated receives

- **GIVEN** `lint-unbounded-wait` reports the workspace unadjudicated
  finding count
- **WHEN** all 117 inventoried bare-recv sites (single-line and
  multi-line receive shapes alike — the inventory is derived from the
  lint's AST scanner, not line text) are bounded or individually
  marker-adjudicated
- **THEN** the ratchet ceiling `scripts/xtask/ratchet-unbounded-wait.max`
  equals the pre-change ceiling minus the bounded/adjudicated count
  (548 − 117 = 431 when all sites are addressed), and
  `cargo xtask lint-unbounded-wait` exits 0 at that ceiling

#### Scenario: Marker use requires justification

- **GIVEN** a receive site where a deadline is semantically wrong
- **WHEN** the site is adjudicated instead of bounded
- **THEN** the source line carries `// allow-test-wait:` followed by a
  non-empty reason naming why a deadline is wrong for that wait, and
  the conversion log lists the site with that justification

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
spawn/connect call targets, an awaited spawned-handle binding, or an
opaque (unresolvable) await base. Out-of-class awaits are not sites:
awaited calls resolving to the finite-call targets
(`tokio::time::sleep`, `tokio::task::yield_now`) are dropped, and
out-of-class method awaits (`send`, `notified`, `on_next`, `accept`,
`read`, `write`, `flush`) stay deferred to a later detector revision —
a loop whose only awaits are out-of-class is not reported. A bounding
call that does not wrap a site — a sibling statement, a disjoint
branch, a closure-internal call, or a dropped never-awaited binding —
SHALL NOT bound that site; if any site in the loop is unbounded, the
loop SHALL be reported (line of the `loop` keyword) and contained wait
findings stay subsumed.

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

#### Scenario: Ratchet verdict recorded on count movement

- **GIVEN** the detector change may surface loop findings previously
  hidden by unrelated timeouts in member trees
- **WHEN** `cargo xtask lint-unbounded-wait` runs after the fix
- **THEN** the count is exact-green at the current ceiling (395), or
  the ceiling is raised only together with an inventoried list of the
  new findings adjudicated as true positives, or the ceiling is
  lowered together with the recorded inventory of every finding the
  change removed (each removal classified as fixed, bounded in-tree,
  or reclassified out-of-class)

