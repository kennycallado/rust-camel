## ADDED Requirements

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
