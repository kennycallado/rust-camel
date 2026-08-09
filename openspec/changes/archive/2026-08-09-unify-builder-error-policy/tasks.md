# Tasks: unify-builder-error-policy

<!--
  Single-phase change (no `## Phases` in design.md, no `## Phase N`
  headings here). The change is one coherent slice: doTry builder error
  policy unification in camel-builder, plus its two external callers and
  the CONTEXT.md policy note.
-->

## camel-builder

### Task 1: Unify doTry builder error policy in do_try.rs

**Files:**
- `crates/camel-builder/src/do_try.rs` (modified)

**Steps:**
1. Remove the `pub fn disposition(mut self, value: ExceptionDisposition) -> Self` method on `DoCatchBuilder` (currently `do_try.rs:151-160`) entirely, including its `# Panics` doc block (lines 148-150).
2. Rewrite `DoCatchBuilder::handled` (currently `do_try.rs:166-168`, which delegates to `self.disposition(ExceptionDisposition::Handled)`) to set the field directly:
   ```rust
   pub fn handled(mut self) -> Self {
       self.disposition = ExceptionDisposition::Handled;
       self
   }
   ```
3. Rewrite `DoCatchBuilder::propagate` (currently `do_try.rs:178-180`) the same way:
   ```rust
   pub fn propagate(mut self) -> Self {
       self.disposition = ExceptionDisposition::Propagate;
       self
   }
   ```
4. Update the doc comments on BOTH sugar methods that currently reference the removed `disposition` method:
   - `handled()` summary line (currently `do_try.rs:162`: `/// Sugar for \`disposition(ExceptionDisposition::Handled)\`.`) — rewrite to describe the behavior directly, e.g. `/// Mark this catch clause as handled: the caught error is absorbed and the clause's exchange becomes the final result (no re-throw).`
   - `propagate()` doc block (currently `do_try.rs:170-177`, references the removed method and `.continued()` non-provision) — rewrite to state that `Handled` and `Propagate` are the only supported dispositions and are set exclusively via these sugar methods (no general `disposition()` setter exists). Keep the note that `.continued()` is intentionally not provided.
5. Add a `compile_fail` rustdoc doctest on the `handled()` method (or the `DoCatchBuilder` impl block) proving no `.disposition(ExceptionDisposition::Continued)` call compiles, PAIRED with a positive (compiling) doctest that demonstrates the valid sugar path. Both live on the same doc block so the harness exercises the contrast:
   ```rust
   /// Only `Handled` and `Propagate` are supported. There is intentionally
   /// no general `disposition(value)` setter, so `Continued` is
   /// unrepresentable at the type level.
   ///
   /// Valid use (compiles):
   ///
   /// ```
   /// # use camel_builder::RouteBuilder;
   /// let _ = RouteBuilder::from("direct:start").route_id("x").do_try()
   ///     .do_catch_exception(&["E"])
   ///     .handled();
   /// ```
   ///
   /// Rejected — does not compile (no `disposition` method exists):
   ///
   /// ```compile_fail
   /// # use camel_builder::RouteBuilder;
   /// # use camel_api::error_handler::ExceptionDisposition;
   /// let b = RouteBuilder::from("direct:start").route_id("x").do_try()
   ///     .do_catch_exception(&["E"]);
   /// b.disposition(ExceptionDisposition::Continued);
   /// ```
   ```
   Verify BOTH doctests are picked up by `cargo test --doc -p camel-builder`. Before step 1 (method removal), the `compile_fail` block fails the harness because `disposition` still compiles; after step 1, the positive block compiles and the `compile_fail` block fails-to-compile as expected (both count as PASS).
6. Change `DoTryBuilder::do_finally` (currently `do_try.rs:100-113`) signature from `pub fn do_finally(self) -> DoFinallyBuilder` to `pub fn do_finally(self) -> Result<DoFinallyBuilder, CamelError>`. Replace the existing `if self.finally_set { panic!("do_finally can only be called once per do_try scope") }` body with `if self.finally_set { return Err(CamelError::RouteError("do_finally can only be called once per do_try scope".into())); }`, keeping the existing success-path construction `Ok(DoFinallyBuilder { parent: self, steps: Vec::new(), on_when: None })` unchanged on the success branch.
7. Replace the `# Panics` doc block on `do_finally` (currently lines 101-103) with a `# Errors` block: "Returns `Err(CamelError::RouteError(_))` if `do_finally` has already been called on this scope."
8. Ensure `use camel_api::CamelError;` is in the imports at the top of `do_try.rs` (it is not currently imported; the file imports `ExceptionDisposition` from `camel_api::error_handler` but not `CamelError`). Add it.
9. Rework the inline `mod tests`:
   - **Delete** `disposition_continued_panics` (lines 297-308) entirely — the misuse is now impossible at compile time; the `compile_fail` doctest replaces it.
   - **Rewrite** `do_finally_called_twice_panics` (lines 284-295) into `do_finally_called_twice_returns_err`: drop the `#[should_panic]` attribute, `#[test]` only. The body unwraps the first `do_finally()` (the success path) and asserts the second returns `Err(CamelError::RouteError(_))`:
     ```rust
     #[test]
     fn do_finally_called_twice_returns_err() {
         let result = RouteBuilder::from("direct:start")
             .route_id("do-try-double-finally")
             .do_try()
             .process(passthrough())
             .do_finally().unwrap()
             .process(passthrough())
             .end_do_finally()
             .do_finally();
         match result {
             Err(CamelError::RouteError(msg)) => {
                 assert!(msg.contains("do_finally can only be called once"),
                     "unexpected message: {msg}");
             }
             other => panic!("expected Err(RouteError), got {other:?}"),
         }
     }
     ```
   - **Edit** `do_try_builder_assembles_correct_shape` (line 236): change `.disposition(ExceptionDisposition::Handled)` to `.handled()`, and change the preceding `.do_finally()` (line 239) to `.do_finally().unwrap()` since it now returns `Result`.
   - **Extend** `do_try_builder_disposition_sugar_methods` (lines 256-282) to assert the disposition VALUE set by each sugar method, not just that `build()` succeeds. The inline `mod tests` is a child of the `do_try` module, so it can read the private `disposition` field of `DoCatchBuilder` directly. For each route, bind the `DoCatchBuilder` before `end_do_catch()`, assert the field, then continue to `build()`:
     ```rust
     // handled route
     let catch = RouteBuilder::from("direct:a")
         .route_id("do-try-sugar-a")
         .do_try().process(passthrough())
         .do_catch_exception(&["Io"])
         .handled();
     assert_eq!(catch.disposition, ExceptionDisposition::Handled);
     let _ = catch.end_do_catch().end_do_try().build().unwrap();

     // propagate route
     let catch = RouteBuilder::from("direct:b")
         .route_id("do-try-sugar-b")
         .do_try().process(passthrough())
         .do_catch_exception(&["Io"])
         .propagate();
     assert_eq!(catch.disposition, ExceptionDisposition::Propagate);
     let _ = catch.end_do_catch().end_do_try().build().unwrap();
     ```
     This directly exercises both blessed sugar scenarios ("handled sugar sets Handled disposition", "propagate sugar sets Propagate disposition"). This test does NOT call `.do_finally()`, so it is unaffected by the do_finally signature change. Then handle EVERY `.do_finally(` call site in the file. The complete inventory (verified): line 239 (in `do_try_builder_assembles_correct_shape`, valid first call → append `.unwrap()`); line 291 (in the rewritten `do_finally_called_twice_returns_err`, valid first call → append `.unwrap()`); line 294 (same test, the SECOND call → leave as the asserted `Result`, no `.unwrap()`). No other `.do_finally(` call sites exist in this file.
10. Run `cargo fmt -p camel-builder` and `cargo clippy -p camel-builder --all-targets -- -D warnings` and fix any findings.

**Tests:** (executable spec)
- `do_finally_called_twice_returns_err`: name = `do_finally_called_twice_returns_err`; setup = a `RouteBuilder` whose `doTry` scope already closed one `doFinally` block via `end_do_finally()`; action = call `do_finally()` a second time; assert = `Err(CamelError::RouteError(msg))` where `msg` contains `"do_finally can only be called once"`; command = `cargo test -p camel-builder --lib do_finally_called_twice_returns_err`; expected = passes after step 6 (before step 6 the test does not compile — signature mismatch).
- `do_try_builder_assembles_correct_shape` (existing, edited): name = `do_try_builder_assembles_correct_shape`; setup = a full doTry route with catch + finally; action = `build().unwrap()`; assert = exactly one `BuilderStep::Processor`; command = `cargo test -p camel-builder --lib do_try_builder_assembles_correct_shape`; expected = passes after the step-7/step-9 edits land (the test fails to compile in the intermediate state between the API change and the `.disposition()`→`.handled()` + `.do_finally().unwrap()` edits, all within this task).
- `do_try_builder_disposition_sugar_methods` (existing, EXTENDED to assert disposition values — covers both blessed sugar scenarios): name = `do_try_builder_disposition_sugar_methods`; setup = two routes, one using `.handled()` and one using `.propagate()`; action = bind each `DoCatchBuilder` before `end_do_catch()`, read its private `disposition` field, assert `== Handled` / `== Propagate` respectively, then `end_do_catch().end_do_try().build().unwrap()`; assert = the field equals the expected `ExceptionDisposition` variant for each route AND both builds succeed; command = `cargo test -p camel-builder --lib do_try_builder_disposition_sugar_methods`; expected = the field assertions are the new part (the test must be extended per step 9 to read `catch.disposition` directly); passes after the step-9 extension lands.
- `disposition_continued_panics`: DELETED — `rg 'disposition_continued_panics' crates/camel-builder/` returns 0 hits after this task.
- compile_fail + positive doctests: command = `cargo test --doc -p camel-builder`; expected = both doctests registered and pass (positive compiles, compile_fail fails-to-compile as expected).

**Acceptance:**
- `cargo build -p camel-builder` succeeds.
- `cargo test -p camel-builder --lib` passes (all inline tests green).
- `cargo test --doc -p camel-builder` passes (compile_fail doctest registered).
- `cargo clippy -p camel-builder --lib -- -D clippy::panic` exits 0 (no `panic!` in the library target).
- `rg 'pub fn disposition' crates/camel-builder/src/` returns 0 hits (method removed).
- `rg 'disposition_continued_panics' crates/camel-builder/` returns 0 hits (test deleted).

- [x] 1

## external-callers

### Task 2: Update the two external do_finally callers for the Result signature

**Depends on:** Task 1 (the `do_finally` signature change).

**Files:**
- `examples/do-try/src/main.rs` (modified)
- `crates/camel-test/tests/do_try_test.rs` (modified)

**Steps:**
1. In `examples/do-try/src/main.rs`, the `do_finally()` call at line 103 sits mid-chain inside the `route3` builder expression (lines 95-113). The relevant tail of the chain is:
   ```rust
       .end_do_catch()
       .do_finally()                                    // line 103
       .process(BoxProcessor::from_fn(move |ex| {       // lines 104-110 (cleanup_clone counter incr)
           /* unchanged closure body */
       }))
       .end_do_finally()
       .end_do_try()
       .build()?;
   ```
   The enclosing `main` returns `Result<(), CamelError>` (verified at `main.rs` signature), so `?` is the clean path. The minimal edit is to insert `?` immediately after `.do_finally()` on line 103 — `?` is an expression operator, so the chain continues on the unwrapped `DoFinallyBuilder`:
   ```rust
       .end_do_catch()
       .do_finally()?                                   // <- insert ?
       .process(BoxProcessor::from_fn(move |ex| {       // lines 104-110 UNCHANGED
           /* unchanged closure body */
       }))
       .end_do_finally()
       .end_do_try()
       .build()?;
   ```
   Do NOT rename variables, restructure the chain, or alter the closure body (lines 104-110 stay byte-identical) — only insert `?` on line 103. Verify the example builds and its single `do_finally` call is the valid first-call path.
2. In `crates/camel-test/tests/do_try_test.rs`, the `do_finally()` call at line 131 sits inside `async fn do_try_finally_runs_after_handled_catch` (line 106), which returns `()`. The chain tail (lines 131-139) is `.do_finally()` at line 131, a `.process()` call at line 132, and `.end_do_finally()`. The test file is already `.unwrap()`-heavy in local style, so append `.unwrap()` to the single valid first-call `do_finally()` on line 131 — the only edit is inserting `.unwrap()` between `.do_finally()` and the `.process()` call at line 132 (lines 132+ stay byte-identical).
   `.unwrap()` on the single valid call in test code is acceptable — the library panic policy forbids panics in the *public API*, not in test glue; `lint-unwrap` already passes on this file's existing `.unwrap()`s. Do not convert the fn to return `Result` (would churn the body).
3. Audit both files for any OTHER `do_finally(` call site (there is exactly one per file based on the pre-change grep; confirm with `rg 'do_finally\(' examples/do-try/src/main.rs crates/camel-test/tests/do_try_test.rs`). Every call site must handle the `Result`. NOTE: Task 1 intentionally leaves these two external callers uncompilable (the workspace does not build between Task 1 and Task 2); Task 2 restores workspace-wide compilation.
4. Run `cargo build -p camel-test --tests` and `cargo build -p do-try` and fix until both compile.
5. Run the affected tests: `cargo test -p camel-test --test do_try_test`.

**Tests:** (executable spec)
- name = `do-try` example builds; setup = Task 1 signature change applied; action = `cargo build -p do-try`; assert = exit code 0; command = `cargo build -p do-try`; expected = fails before this task (do_finally returns Result unhandled), passes after step 1 inserts `?`.
- name = `do_try_test` compiles and passes; setup = Task 1 + Task 2 step 2 applied; action = run the test binary; assert = all tests pass, 0 failures; command = `cargo test -p camel-test --test do_try_test`; expected = fails to compile before step 2, passes after `.unwrap()` inserted.
- name = no bare do_finally chaining on the Result; setup = both files edited; action = grep for `.do_finally()` directly followed by a builder method (not `?`/`.unwrap()`/`.expect()`); assert = 0 hits; command = `rg '\.do_finally\(\)\s*\.(process|on_when|end_do_finally)' examples/do-try/src/main.rs crates/camel-test/tests/do_try_test.rs`; expected = 0 hits after this task (every call is followed by `?` or `.unwrap()` before chaining).

**Acceptance:**
- `cargo build --workspace` exits 0 (the whole workspace compiles, confirming no other hidden caller broke).
- `cargo test -p camel-test --test do_try_test` passes.
- `cargo build -p do-try` exits 0.
- `rg '\.do_finally\(\)\s*\.(process|on_when|end_do_finally)' crates/ examples/ tests/` returns 0 hits (no bare chaining on the Result anywhere in the workspace).

- [x] 2

## docs

### Task 3: Update camel-builder CONTEXT.md to prescribe the unified policy

**Depends on:** Task 1 (the policy describes the resolved sites Task 1 implements).

**Files:**
- `crates/camel-builder/CONTEXT.md` (modified)

**Steps:**
1. Locate the "panic-vs-`Result` policy" paragraph in the "Architecture notes" section of `crates/camel-builder/CONTEXT.md`. Its heading is `**panic-vs-\`Result\` policy (mixed — decision noted, not prescribed here).**` and its body describes the two panicking misuse paths and states "This asymmetry is a recorded finding (camel-builder audit I1) whose resolution is deferred to the code stream — this document records the **current state**, not the fix direction."
2. Rewrite that paragraph to prescribe the now-applied policy. New text (STE-friendly, English, no AI slop):
   - Heading: `**panic-vs-\`Result\` policy (prescribed).**`
   - State the policy: "Builder public APIs do not panic on user-reachable misuse. Misuse is prevented at the type level where cheaply possible, and reported via `Result<_, CamelError>` otherwise. No public method panics on user input or state."
   - Record the two resolved sites: `DoCatchBuilder` exposes only `handled()` / `propagate()` sugar — `Continued` is unrepresentable (type-level prevention); `DoTryBuilder::do_finally` returns `Result<DoFinallyBuilder, CamelError>` and a second call yields `Err(CamelError::RouteError(_))`.
   - Note the mechanical enforcement: `cargo clippy -p camel-builder --lib -- -D clippy::panic` gates the library target; a `compile_fail` doctest guards the removed `disposition` method.
   - Reference bd issue `rc-0lhn` and audit finding I1 as the origin, now resolved by this change.
3. Update the "Related decisions" trailing line that currently reads `**Open finding (does not block this doc):** I1 — unify the panic-vs-\`Result\` policy on \`do_finally\` / \`disposition\`. Tracked in the code stream; this doc records current state either way.` — change it to `**Resolved:** I1 — panic-vs-\`Result\` policy unified on \`do_finally\` / \`disposition\` (see "Architecture notes"). Resolved by the \`unify-builder-error-policy\` change (bd \`rc-0lhn\`).`
4. Verify no stale reference to the old mixed-policy phrasing remains: `rg 'decision noted, not prescribed|two misuse paths.*panic' crates/camel-builder/CONTEXT.md` returns 0 hits after the edit.
5. If the file references a HEAD commit hash for the "current state" claim (e.g. `7f9d8a03`), leave the historical reference intact but ensure the *current* state described matches the post-change reality (the policy is now prescribed, not "noted").

**Tests:** (executable spec)
- name = no stale "decision noted" phrasing; setup = Task 3 edits applied; action = grep for the old phrasing; assert = 0 hits; command = `rg 'decision noted, not prescribed' crates/camel-builder/CONTEXT.md`; expected = 1 hit before this task, 0 hits after.
- name = policy prescribed; setup = Task 3 edits applied; action = grep for the policy sentence; assert = ≥1 hit; command = `rg 'do not panic on user-reachable misuse' crates/camel-builder/CONTEXT.md`; expected = 0 hits before, ≥1 hit after.
- name = I1 marked resolved; setup = Task 3 edits applied; action = grep for the resolved marker; assert = ≥1 hit; command = `rg '\*\*Resolved:\*\* I1' crates/camel-builder/CONTEXT.md`; expected = 0 hits before, ≥1 hit after.
- name = enforcement documented; setup = Task 3 edits applied; action = grep for the clippy gate string; assert = ≥1 hit; command = `rg 'clippy::panic' crates/camel-builder/CONTEXT.md`; expected = 0 hits before, ≥1 hit after.

**Acceptance:**
- `rg 'decision noted, not prescribed|two misuse paths.*panic' crates/camel-builder/CONTEXT.md` returns 0 hits.
- `rg 'do not panic on user-reachable misuse' crates/camel-builder/CONTEXT.md` returns ≥1 hit.
- `rg '\*\*Resolved:\*\* I1' crates/camel-builder/CONTEXT.md` returns ≥1 hit.
- `rg 'clippy::panic' crates/camel-builder/CONTEXT.md` returns ≥1 hit.
- STE-writing prose pass applied to the rewritten paragraph (no AI slop, ASD-STE100-leaning; identifiers/commands untranslated).

- [x] 3
