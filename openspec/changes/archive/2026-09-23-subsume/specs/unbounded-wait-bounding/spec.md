## ADDED Requirements

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
