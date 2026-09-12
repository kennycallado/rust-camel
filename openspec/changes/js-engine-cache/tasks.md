# Tasks: js-engine-cache

## camel-language-js

### Task 1.1: Worker skeleton — thread, bounded channel, deadline skip, panic self-heal

**Files:**
- `crates/languages/camel-language-js/src/engines/worker.rs` (new)
- `crates/languages/camel-language-js/src/engines/boa.rs` (modified)
- `crates/languages/camel-language-js/src/engines/mod.rs` (modified)

**Steps:**
1. Create `engines/worker.rs` with `pub(super) enum JsJob`:
   `Eval { source: Arc<str>, exchange: JsExchange, timeout_ms: u64, enqueued: Instant, reply: SyncSender<Result<JsEvalResult, JsLanguageError>> }`,
   `Validate { source: Arc<str>, reply: SyncSender<Result<(), JsLanguageError>> }`, and
   `#[cfg(test)] TestBlock { millis: u64, reply: SyncSender<Result<JsEvalResult, JsLanguageError>> }` plus
   `#[cfg(test)] TestPanic { reply: SyncSender<Result<JsEvalResult, JsLanguageError>> }`.
2. Add `pub(super) struct JsWorkerHandle { tx: std::sync::mpsc::SyncSender<JsJob> }` (derives `Clone` and `Debug`, so `BoaEngine`'s existing derives keep compiling) with
   `pub(super) fn spawn(limits: camel_language_api::JsLimitsConfig) -> JsWorkerHandle`:
   create `sync_channel(1024)`, `std::thread::Builder::new().name("camel-js-worker")`, run a
   loop that owns a `WorkerState { ctx: Context, limits: ResolvedJsLimits }` built on the
   worker thread. Structure the loop as a thin recv-dispatch shell: all per-job logic lives
   in `WorkerState` methods (added in tasks 1.2–1.3) so unit tests can build a `WorkerState`
   directly on the test thread and call the same methods the thread calls.
3. In this task the worker executes each `Eval` job with TODAY's exact semantics, and the
   dispatch loop wraps the ENTIRE job attempt (every job kind, from install through
   extract) in `std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| /* job attempt */))`: fresh `Context::default()`,
   apply resolved runtime limits, `bindings::register_console`, `bindings::build_camel_global`
   + set `camel` global, `ctx.eval(Source)`, `js_to_value`, `bindings::extract_camel_state` —
   mirroring the current `BoaEngine::eval` body. On panic (including `TestPanic`): reply
   `Err(JsLanguageError::Execution { message: "JS worker panic recovered" })` and rebuild
   `WorkerState` (fresh `Context`) before the next job. `Validate` runs `Script::parse` on
   the raw source against a fresh `Context` as today.
4. At dequeue, before executing an `Eval` job, check
   `job.enqueued.elapsed() > Duration::from_millis(job.timeout_ms)`: if expired, reply
   `Err(JsLanguageError::Execution { message: "JS execution timeout" })` and SKIP execution.
   The worker never blocks on a reply `send`: on `SendError` it discards the result and
   continues.
5. Move `ResolvedJsLimits` and `resolve_js_limits` from `boa.rs` visibility to
   `pub(super)` so `worker.rs` can use them. Modify `BoaEngine` to hold
   `limits: JsLimitsConfig` plus `worker: Arc<OnceLock<JsWorkerHandle>>` (the `Arc` makes
   `BoaEngine` clones share ONE worker, preserving "one worker per limits-configuration");
   `eval` and `validate` lazily spawn the worker, clone `source` into `Arc<str>`, send the
   job, and block on `reply.recv()`; on `RecvError` (worker gone) return
   `Err(JsLanguageError::Execution { message: "JS worker unavailable" })`. Delete the
   direct Boa code paths from `boa.rs` (they now live in the worker). Update
   `engines/mod.rs` to declare `mod worker;`. Per-call reply channel via
   `sync_channel(1)`; lazy-spawn races resolved by the `get_or_init` pattern (a racing
   loser's transient worker exits via recv error on its dropped receiver).
6. Update the doc header of `BoaEngine` and `boa.rs` module docs: evaluation runs on a
   dedicated worker thread; per-eval realm strategy is staged by tasks 1.2–1.3 (state here:
   fresh `Context` per eval, same as before).

**Tests:** (in `worker.rs` `#[cfg(test)] mod tests`, plus existing suite)
- `expired_queued_job_is_skipped`: spawn worker → send `TestBlock { millis: 300 }` → send `Eval` of `"1+1"` with `timeout_ms: 1` and `enqueued: Instant::now()` → assert the `Eval` reply is `Err` whose message contains `"timeout"`, and that a follow-up `Eval` of `"1+1"` returns `2` (worker healthy, expired job did not execute).
- `worker_self_heals_after_panic`: send `TestPanic` → assert `Err` with `"panic"` in message → send `Eval` of `"1+1"` → assert `Ok` with return value `2`.
- Existing `boa.rs`, `expression.rs`, `language.rs` suites stay green through `cargo test -p camel-language-js` (behavior-identical redirection).

**Acceptance:**
- `cargo test -p camel-language-js` exits 0 (including the two new worker tests).
- `cargo clippy -p camel-language-js --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.

- [x] 1.1

### Task 1.2: Stable realm, indirect-eval wrapper, bounded ScriptCache

**Files:**
- `crates/languages/camel-language-js/src/engines/worker.rs` (modified)
- `crates/languages/camel-language-js/src/engines/engine.rs` (modified)

**Steps:**
1. Add `fn escape_js_string_literal(src: &str) -> String`: emits a valid JS double-quoted string literal for arbitrary Unicode source — escape backslash first, then `"`, then control characters: LF as `\n`, CR as `\r`, U+2028 as `\u2028`, U+2029 as `\u2029`, tab as `\t`, all other `char`s below U+0020 as `\u{:04x}`; no other transformations.
2. Add `struct ScriptCache { map: HashMap<Arc<str>, boa_engine::Script>, order: VecDeque<Arc<str>>, cap: usize }` with `fn new(cap: usize) -> Self` (cap 256), `fn get(&mut self, key: &Arc<str>) -> Option<&boa_engine::Script>` (refreshes recency), `fn insert(&mut self, key: Arc<str>, script: boa_engine::Script)` (evicts least-recently-used when at cap), `fn len(&self) -> usize`, `fn clear(&mut self)`. No new dependency; `Arc<str>` equality compares full source bytes by Rust semantics.
3. Rework `WorkerState` to `{ ctx: Context, realm: boa_engine::Realm, cache: ScriptCache, limits: ResolvedJsLimits }`: build once on the worker thread — `Context::default()`, apply limits, and keep the default realm as the stable realm (enter it explicitly for clarity). Add `pub(super) fn run_eval(&mut self, source: &str, exchange: JsExchange) -> Result<JsEvalResult, JsLanguageError>` holding the per-eval body (the worker thread's dispatch calls it; tests call it directly on a locally built `WorkerState`): enter stable realm → `bindings::register_console(&mut ctx)` → `bindings::build_camel_global` + set `camel` global (fresh per eval, as today) → look up wrapper by `Arc<str>` key; on miss compile
   `Script::parse(Source::from_bytes(format!("eval(\"{}\")", escape_js_string_literal(source)).as_bytes()), None, &mut ctx)` and insert → `script.evaluate(&mut ctx)` → `js_to_value` → `bindings::extract_camel_state`.
4. `Validate` jobs parse the RAW source via `Script::parse` on the worker's persistent
   `Context` (no wrapper — the wrapper of an invalid source is itself valid JS, which would
   swallow the parse error) and map parse failures to `JsLanguageError::Parse`. `Eval` jobs
   map wrapper/inner failures to `JsLanguageError::Execution` exactly as today's `ctx.eval`
   did. Also add `#[derive(PartialEq)]` to `JsEvalResult` in `src/engine.rs` so repeat-result
   tests can use `assert_eq!`. Invariant: validate is parse-only — never `Script::run()` /
   `evaluate` (parse creates no global or lexical bindings, so the stable realm stays clean).
5. Keep the panic self-heal from 1.1 but rebuild the full `WorkerState` (fresh `Context` + realm + cleared cache).

**Tests:** (in `worker.rs` tests; unit tests build `WorkerState` directly on the test thread unless a test name says "through engine")
- `lexical_declarations_are_fresh_per_eval`: through one `BoaEngine`, evaluate `let count = (typeof count === 'undefined') ? 0 : count; count++; count` twice with `JsExchange::default()` → both return `1` (no redeclaration error).
- `wrapper_escapes_adversarial_sources`: for each entry in an expected-value table — `r#"'it\'s'"#` → `it's`, `r#""quo\"te""#` → `quo"te`, `r#"back\\slash"#` → `back\slash`, `"cr\rLf\n"` mix, `"ls\u{2028}ps\u{2029}"` (U+2028/U+2029 as Rust escapes), `r#"trailing\"#` (trailing backslash) — call `WorkerState::run_eval` on a local `WorkerState` → assert the exact expected string result (today's raw-source semantics recorded in the table); after 3 repeats per source assert `cache.len()` is exactly the number of distinct sources (one entry per source).
- `invalid_source_maps_to_execution_error`: evaluate `"let x = {{{"` → `Err` whose variant is `JsLanguageError::Execution` (never `Parse`); `validate("let x = {{{")` → `Err` variant `JsLanguageError::Parse`.
- `identical_results_across_repeats`: local `WorkerState`, one source + one `JsExchange`, `run_eval` three times → all three `JsEvalResult`s are structurally equal (`assert_eq!` on the structs).
- `identical_errors_first_and_cached`: local `WorkerState`, source `throw new Error('boom')`, `run_eval` twice → both `Err` with the same variant and identical message string (compare via `match` on variant + `assert_eq!` on the message, since `JsLanguageError` has no `PartialEq`).
- `route_rebuild_reuses_cache_entry`: local `WorkerState`, `run_eval` the same source twice (two "expression builds" sharing one engine, the watch-reload shape) → `cache.len() == 1` and both results identical; plus engine-level: one `JsLanguage`, `create_expression(script)` called twice, both expressions evaluated → identical values.
- `lru_eviction_bounds_cache_at_256`: construct `ScriptCache::new(256)` directly, insert 257 distinct `Arc<str>` keys with dummy `Script` values obtained from `Script::parse(Source::from_bytes(b"0"), None, &mut ctx)` → assert `len() == 256` and the first key is gone while the 257th is present.
- Existing suites stay green (`cargo test -p camel-language-js`), including the `SlowEngine` custom-engine tests (worker bypass) and the existing loop/recursion limit tests (limits unchanged).

**Acceptance:**
- `cargo test -p camel-language-js` exits 0.
- `cargo clippy -p camel-language-js --all-targets -- -D warnings` exits 0; `cargo fmt --check` exits 0.

- [x] 1.2

### Task 1.3: Cleanup, install-verify, integrity set, realm recycling, honest contract docs

**Files:**
- `crates/languages/camel-language-js/src/engines/worker.rs` (modified)
- `crates/languages/camel-language-js/src/engines/boa.rs` (modified)
- `crates/languages/camel-language-js/src/language.rs` (modified)
- `crates/languages/camel-language-js/CONTEXT.md` (modified)

**Steps:**
1. Add `struct IntegrityBaseline { global_keys: Vec<boa_engine::property::PropertyKey>, roots: Vec<RootSnapshot> }` and `struct RootSnapshot { name: &'static str, object: JsObject, keys: Vec<boa_engine::property::PropertyKey>, descriptors_hash: u64, proto: Option<JsObject> }` captured once after the initial `camel`/`console` install on the stable realm: `global_keys` = `ctx.global_object().own_property_keys(&mut ctx)`; roots = the `eval` function (value under key `"eval"`) and the prototypes of `Object`, `Array`, `Function`, `String`, `Number`, `Boolean` obtained via `ctx.intrinsics().constructors().object().prototype()` and the analogous `array()`, `function()`, `string()`, `number()`, `boolean()` accessors; `descriptors_hash` = a `DefaultHasher` fold over each key's own-property descriptor fields (value identity via `JsValue::strict_eq` against a stored snapshot list, writable/enumerable/configurable flags). `camel`/`console` stay in `global_keys` (deletion still counts as drift) but are EXCLUDED from value-identity comparison.
2. Add `fn scrub_globals(&mut self)`: delete every own key of the global object that is not in `baseline.global_keys`, ignoring delete failures (non-configurable keys remain — the detector catches them next). NOTE: scrub is redundant with integrity-check-on-globalThis-keys + recycle (any key scrub can remove would also trigger drift); the spec blesses it, so implement as this trivial loop and revisit at archive.
3. Add `fn install_verify(&mut self) -> bool`: re-run `register_console` + `build_camel_global`+set for a fresh `JsExchange::default()`-independent minimal install is NOT needed — instead verify presence: `camel` and `console` own keys exist and are objects, and the `eval` value strictly equals `baseline`'s captured `eval` object. Return `false` on any miss.
4. Add `fn integrity_check(&mut self) -> bool`: recompute the snapshot fields for each root and compare against baseline (key sets, descriptor hashes, prototype identity, `eval` value identity). Any difference → `false`.
5. Add `fn recycle(&mut self)`: `let realm = self.ctx.create_realm()?` → `enter_realm` it → re-apply runtime limits → re-install `camel`/`console` → re-capture `IntegrityBaseline` → `cache.clear()`. On `create_realm` error, rebuild the whole `WorkerState`.
6. Move the full per-eval orchestration INTO `WorkerState::run_eval` (so local-`WorkerState` tests exercise the real path): deadline check → `scrub_globals` → install fresh `console`/`camel` → `install_verify`; if `false` → `recycle` → recheck job deadline → retry install+verify once; second consecutive failure → reply `Err(Execution { message: "JS engine realm failed verification" })`, `recycle`, continue → cache lookup/insert wrapper → `script.evaluate` → convert + extract → reply → `integrity_check`; `false` → `recycle`. The dispatch loop's `catch_unwind` (task 1.1) stays around the whole job attempt; scrub and integrity therefore run on success, error, panic, and limit failure paths.
7. Add a `recycle_count: u64` field on `WorkerState` incremented in `recycle` (plain field — NOT a process-wide static, which would flake under parallel tests sharing the binary).
8. Update `boa.rs` docs and `language.rs` Thread-Safety section: replace the "fresh Context per eval" claim with the honest contract. Rewrite the `CONTEXT.md` "Sandbox posture" section to carry the contract wording from spec Requirement 2 (fresh camel/console + fresh declarative environment; configurable global additions removed; named intrinsic roots verified; NO realm isolation — surviving state enumerated; route reload/restart do not reset; trusted operator config;    untrusted code → `function:` per ADR-0005), and update the "Boa boundary" file list to
   include `src/engines/worker.rs`. The `worker.rs` module docs note: worker-thread death
   leaves a stale handle surfaced as `"JS worker unavailable"`; `JsLanguage` recreation is
   the recovery path.

**Tests:** (in `worker.rs` tests; unit tests build `WorkerState` directly on the test thread unless a test name says "through engine")
- `global_assignment_does_not_persist`: eval `globalThis.leak = 42; 0` then `typeof leak` → `'undefined'`.
- `poisoned_eval_recovers`: through one `BoaEngine`, eval `eval = 1; 0` then `typeof eval` → `'function'` (behavioral proof of recycle).
- `undeletable_global_recycles_realm`: eval `Object.defineProperty(globalThis, 'pwn', { value: 1, configurable: false }); 0` then `typeof pwn` → `'undefined'` (recycle detected the undeletable key).
- `frozen_prototype_recycles_realm`: eval `Object.freeze(Object.prototype); 0` then `Object.isFrozen(Object.prototype)` → `false`.
- `failed_eval_residue_is_cleaned`: eval `globalThis.x = 1; throw new Error('boom')` (Err) then `typeof x` → `'undefined'`.
- `limit_failure_residue_is_cleaned`: eval `globalThis.x = 1; while (true) {}` (Err — loop limit trips) then `typeof x` → `'undefined'` (cleanup ran on the limit-failure path).
- `console_and_camel_are_fresh_each_eval`: eval `console.log = function(){}; camel.headers = 1; 0` then `console.log('x'); camel.headers.get('k')` with an exchange carrying header `k` → Ok, console still routes to tracing, and the re-installed `camel.headers` returns the header value.
- `date_prototype_persists_until_recycle`: eval `Date.prototype.probe = 1; 0` then `new Date().probe` → `1`; then run the undeletable-global drift trigger; then `new Date().probe` → `undefined` (recycled realm dropped it).
- `no_recycle_storm_on_steady_state`: local `WorkerState`, 100 × `run_eval` of `camel.body` with a small exchange → `recycle_count` stays 0 (camel/console identity churn does not trigger drift).
- `concurrent_expressions_no_cross_talk` (`#[tokio::test]` in `expression.rs` tests): two `JsExpression`s, sources `camel.headers.set('a', '1'); camel.headers.get('a')` and `camel.headers.set('b', '2'); camel.headers.get('b')`, evaluated interleaved via `tokio::join!` → each result matches its own script.
- `integrity_detector_cost_diagnostic` (`#[ignore]` diagnostic, like `jstax_probe`): build `WorkerState` on the test thread, time 200 `integrity_check` calls → print the measured mean; run explicitly via `cargo test -p camel-language-js integrity_detector -- --ignored --nocapture`; the printed number feeds the Requirement-4 shrink decision (no absolute assertion in the always-run suite — CI-stable principle).

**Acceptance:**
- `cargo test -p camel-language-js` exits 0 including all isolation/recycle tests.
- `cargo clippy -p camel-language-js --all-targets -- -D warnings` exits 0; `cargo fmt --check` exits 0.
- `CONTEXT.md` contains the full contract wording (spec Requirement 2) and lists `worker.rs` in the Boa boundary.

- [x] 1.3

### Task 1.4: Amortization timing test, probe commit, steady-state documentation

**Files:**
- `crates/languages/camel-language-js/tests/jstax_probe.rs` (new — commit the existing diagnostic probe as authored during STAGE 1)
- `crates/languages/camel-language-js/tests/js_engine_cache_amortization.rs` (new)

**Steps:**
1. Commit `tests/jstax_probe.rs` unchanged (diagnostic, `#[ignore]`d, header documents it as mission evidence and how to run it).
2. Create `tests/js_engine_cache_amortization.rs` with `#[test] fn amortization_ratio_at_least_20x()`: build one `BoaEngine::default()` (cold worker), take eval 1 wall-clock of the representative transform `camel.headers.set('n', (parseInt(camel.headers.get('n'))||0)+1); camel.body` with a small `JsExchange`, then time evals 2..=1000 of the same source+exchange; assert `mean(evals 2..=1000) < eval1 / 20`; print the measured steady-state µs per eval with a label (`steady_state_us_per_eval`) for the mission report.
3. Ratio-headroom contingency: the Req-4 integrity-set shrink is ALREADY APPLIED (task 1.3: detector measured ~47 µs release pre-shrink, 1.9× the 25 µs budget — condition met; post-shrink ~30 µs). If the measured ratio still lands below 20x, do not weaken the assertion and do not cut further named roots — record both measured numbers in the test doc comment and report the shortfall to the conductor as a spec-vs-reality finding for an honest spec amendment.
4. In the test's doc comment, record the target from the design: local steady-state ≤ 25 µs per eval including integrity check and inner re-parse; note the measured dev-box number once available (fill with the actual measurement after first green run, not a placeholder).

**Tests:**
- `amortization_ratio_at_least_20x`: cold `BoaEngine` → 1000 evals as above → assert mean of evals 2..=1000 is under 5% of eval 1; test is `#[test]` (CI-stable ratio, no absolute-ms assertion).

**Acceptance:**
- `cargo test -p camel-language-js` exits 0 with the new test included.
- `cargo test -p camel-language-js --test jstax_probe -- --ignored --nocapture` still runs (diagnostic preserved).
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-language-js --no-deps` exits 0.

- [x] 1.4
