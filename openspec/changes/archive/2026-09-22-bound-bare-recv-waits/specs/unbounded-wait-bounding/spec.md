## ADDED Requirements

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
