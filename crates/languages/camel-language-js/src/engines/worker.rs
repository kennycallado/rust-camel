//! Dedicated worker thread for Boa JS evaluation.
//!
//! All JS execution for one [`BoaEngine`](super::boa::BoaEngine)
//! limits-configuration runs on a single `camel-js-worker` OS thread. Jobs
//! arrive over a bounded `sync_channel(1024)`; each call gets its own reply
//! channel (`sync_channel(1)`) and blocks on it.
//!
//! Realm strategy: the worker owns ONE stable realm per generation. Each
//! eval re-enters that realm, scrubs configurable global keys added since
//! the baseline, installs fresh `console` and `camel` globals, verifies
//! the protected invariants (`camel`/`console` present as objects, `eval`
//! identity), and runs the source inside the wrapper
//! `eval("<escaped source>")`, whose compiled `Script` is cached in a
//! bounded LRU keyed by the full source bytes (cap
//! 256). The wrapper gives every eval a fresh declarative environment
//! (lexical `let`/`const` declarations do not leak across evals). After
//! the eval — on success, error, and limit-failure paths alike — the named
//! integrity set (global `globalThis` own keys, the `eval` function, and
//! the prototypes of `Object`, `Array`, and `Function`; see
//! `integrity.rs` for the measured Requirement-4 shrink) is verified
//! against a baseline captured right after the initial `camel`/`console`
//! install. Any drift, a failed install-verify, or a panic recycles the whole realm (fresh realm, fresh baseline,
//! dropped wrapper cache); the worker never attempts restoration. State
//! OUTSIDE the named integrity set (for example `Date.prototype`
//! mutations, retained object graphs, engine-internal state) may survive
//! across evals, exchanges, and routes until a recycle — see the crate
//! `CONTEXT.md` "Sandbox posture" section for the honest contract.
//!
//! `Validate` jobs are parse-only: the raw source goes through
//! `Script::parse` on the worker's persistent context (no wrapper — the
//! wrapper of an invalid source is itself valid JS and would swallow the
//! parse error) and is never evaluated, so validation creates no global or
//! lexical bindings and the stable realm stays clean.
//!
//! Panic self-heal: every job attempt runs behind `catch_unwind`; a panic is
//! answered with an execution error and the worker rebuilds its full state
//! (fresh `Context` + realm, empty script cache) before the next job, so one
//! bad evaluation cannot brick the worker.
//!
//! Worker-thread death leaves a stale handle behind; later jobs fail with
//! `"JS worker unavailable"`. Recreating the `JsLanguage` is the recovery
//! path.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

use boa_engine::property::PropertyKey;
use boa_engine::realm::Realm;
use boa_engine::{Context, JsObject, JsValue, Script, Source, js_string};

use super::boa::{ResolvedJsLimits, resolve_js_limits};
use super::integrity::IntegrityBaseline;
use crate::{
    bindings,
    engine::{JsEvalResult, JsExchange},
    error::JsLanguageError,
    value::js_to_value,
};

#[cfg(test)]
use serde_json::Value;

/// Bounded capacity of the job queue feeding the worker thread.
const JOB_CHANNEL_CAPACITY: usize = 1024;

/// Bounded number of compiled wrapper scripts kept by [`ScriptCache`].
const SCRIPT_CACHE_CAPACITY: usize = 256;

/// A unit of work for the JS worker thread.
pub(super) enum JsJob {
    Eval {
        source: Arc<str>,
        exchange: JsExchange,
        timeout_ms: u64,
        enqueued: Instant,
        reply: mpsc::SyncSender<Result<JsEvalResult, JsLanguageError>>,
    },
    Validate {
        source: Arc<str>,
        reply: mpsc::SyncSender<Result<(), JsLanguageError>>,
    },
    #[cfg(test)]
    TestBlock {
        millis: u64,
        reply: mpsc::SyncSender<Result<JsEvalResult, JsLanguageError>>,
    },
    #[cfg(test)]
    TestPanic {
        reply: mpsc::SyncSender<Result<JsEvalResult, JsLanguageError>>,
    },
}

/// Handle to the single `camel-js-worker` thread for one limits-configuration.
#[derive(Debug)]
pub(super) struct JsWorkerHandle {
    tx: mpsc::SyncSender<JsJob>,
}

impl JsWorkerHandle {
    /// Spawn the `camel-js-worker` thread.
    ///
    /// If thread creation fails, the job receiver drops with the closure and
    /// every later [`send`](JsWorkerHandle::send) fails — callers surface
    /// `"JS worker unavailable"`.
    pub(super) fn spawn(limits: camel_language_api::JsLimitsConfig) -> JsWorkerHandle {
        let (tx, rx) = mpsc::sync_channel(JOB_CHANNEL_CAPACITY);
        let _ = std::thread::Builder::new()
            .name("camel-js-worker".to_string())
            .spawn(move || worker_loop(rx, limits));
        JsWorkerHandle { tx }
    }

    /// Queue a job. `Err` means the worker thread is gone.
    pub(super) fn send(&self, job: JsJob) -> Result<(), JsLanguageError> {
        self.tx.send(job).map_err(|_| worker_unavailable())
    }
}

/// The error surfaced when the worker thread is gone (stale handle).
pub(super) fn worker_unavailable() -> JsLanguageError {
    JsLanguageError::Execution {
        message: "JS worker unavailable".to_string(),
    }
}

/// Escape `src` into the body of a valid JS double-quoted string literal
/// (without the surrounding quotes), such that parsing the literal yields
/// exactly `src` for arbitrary Unicode input.
///
/// Backslash is escaped first, then the `"` delimiter, then control
/// characters: LF as `\n`, CR as `\r`, tab as `\t`, U+2028/U+2029 (line/paragraph
/// separators) as `\u2028`/`\u2029`, and every other `char` below U+0020 as
/// `\uXXXX`. No other transformations.
fn escape_js_string_literal(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 2);
    for c in src.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Bounded LRU cache of compiled wrapper scripts.
///
/// Keyed by the full source bytes — `Arc<str>` equality compares content by
/// Rust semantics, so two evals share an entry only when their sources are
/// byte-identical. Values are realm-bound `Script`s: they live and die with
/// the worker thread's stable realm and never cross a thread boundary.
struct ScriptCache {
    map: HashMap<Arc<str>, boa_engine::Script>,
    order: VecDeque<Arc<str>>,
    cap: usize,
}

impl ScriptCache {
    fn new(cap: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            cap,
        }
    }

    /// Returns the compiled script for `key`, refreshing its recency.
    fn get(&mut self, key: &Arc<str>) -> Option<&boa_engine::Script> {
        if self.map.contains_key(key) {
            self.touch(key);
        }
        self.map.get(key)
    }

    /// Inserts `script` under `key`, evicting the least-recently-used entry
    /// while the cache is at capacity.
    fn insert(&mut self, key: Arc<str>, script: boa_engine::Script) {
        if self.map.contains_key(&key) {
            self.touch(&key);
            self.map.insert(key, script);
            return;
        }
        while self.map.len() >= self.cap {
            match self.order.pop_front() {
                Some(evicted) => {
                    self.map.remove(&evicted);
                }
                None => break,
            }
        }
        self.order.push_back(key.clone());
        self.map.insert(key, script);
    }

    /// Number of cached scripts (test assertions; production reads only
    /// observe it via [`ScriptCache::clear`] bookkeeping).
    #[allow(dead_code)]
    fn len(&self) -> usize {
        self.map.len()
    }

    /// Drops every cached script (the wrapper scripts are realm-bound and
    /// die with the recycled realm).
    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }

    /// Moves `key` to the most-recently-used position.
    fn touch(&mut self, key: &Arc<str>) {
        if let Some(pos) = self.order.iter().position(|k| k == key)
            && let Some(k) = self.order.remove(pos)
        {
            self.order.push_back(k);
        }
    }
}

/// The freshly installed per-eval bindings, returned by
/// [`install_bindings`] and identity-verified by
/// [`WorkerState::install_verify`].
struct InstalledGlobals {
    console: JsObject,
    camel: JsObject,
}

/// Why an [`install_bindings`] attempt failed.
enum InstallError {
    /// Deterministic exchange-data failure (conversion error): recurs
    /// identically on a fresh realm, so it is surfaced directly without a
    /// recycle-retry.
    Data(JsLanguageError),
    /// A global `console`/`camel` set was REJECTED (for example a spoofed
    /// non-writable binding): the realm is poisoned and must be recycled.
    Rejected,
}

/// Install fresh `console` (tracing-backed) and `camel` (exchange-bound)
/// globals on the currently active realm. Both global sets use
/// `throw = true` — a rejected set (a silent `Ok(false)` under
/// `throw = false`) is reported as [`InstallError::Rejected`] so the caller
/// can recycle the realm instead of evaluating against a spoofed binding.
fn install_bindings(
    ctx: &mut Context,
    exchange: &JsExchange,
) -> Result<InstalledGlobals, InstallError> {
    let console = bindings::register_console(ctx).map_err(|_| InstallError::Rejected)?;
    let camel_obj = bindings::build_camel_global(exchange, ctx).map_err(|e| {
        InstallError::Data(JsLanguageError::Execution {
            message: e.to_string(),
        })
    })?;
    ctx.global_object()
        .set(
            js_string!("camel"),
            JsValue::from(camel_obj.clone()),
            true,
            ctx,
        )
        .map_err(|_| InstallError::Rejected)?;
    Ok(InstalledGlobals {
        console,
        camel: camel_obj,
    })
}

/// Worker-thread-local engine state. Never crosses a thread boundary; tests
/// may build it directly and call the same methods the worker loop calls.
pub(super) struct WorkerState {
    /// Persistent context shell hosting the stable realm.
    ctx: Context,
    /// The stable realm: entered for every eval, recycled in whole on
    /// integrity drift, install-verify failure, or panic.
    realm: Realm,
    /// Compiled wrapper scripts keyed by full source bytes.
    cache: ScriptCache,
    /// Resolved runtime limits; re-applied when the realm is recycled.
    limits: ResolvedJsLimits,
    /// Integrity baseline for the current realm generation, captured after
    /// the initial `camel`/`console` install.
    baseline: IntegrityBaseline,
    /// Number of realm recycles (drift, install-verify failure, or panic
    /// rebuild) since construction. A plain field, deliberately NOT a
    /// process-wide static: parallel tests share one binary and a global
    /// counter would flake.
    #[allow(dead_code)]
    recycle_count: u64,
}

impl WorkerState {
    /// Build the worker state: fresh `Context` with resolved runtime limits,
    /// its default realm kept as the stable realm (entered explicitly for
    /// clarity), an empty wrapper-script cache, a minimal initial
    /// `camel`/`console` install, and the integrity baseline captured
    /// AFTER that install.
    ///
    /// Soundness: the baseline must include `camel`/`console` in its global
    /// key set so the per-eval reinstall (which replaces their VALUES each
    /// eval) cannot count as drift — their values are excluded from
    /// identity comparison, but their key presence is checked. On the
    /// (practically impossible) initial-install failure the baseline stays
    /// empty, the next eval's install-verify fails, and the recycle path
    /// rebuilds a healthy state.
    pub(super) fn new(limits: ResolvedJsLimits) -> Self {
        let mut ctx = Context::default();
        apply_runtime_limits(&mut ctx, &limits);
        let realm = ctx.realm().clone();
        ctx.enter_realm(realm.clone());
        let mut state = Self {
            ctx,
            realm,
            cache: ScriptCache::new(SCRIPT_CACHE_CAPACITY),
            limits,
            baseline: IntegrityBaseline::empty(),
            recycle_count: 0,
        };
        if install_bindings(&mut state.ctx, &JsExchange::default()).is_ok() {
            state.baseline = IntegrityBaseline::capture(&mut state.ctx);
        }
        state
    }

    /// Run one `Eval` job end to end: deadline check → scrub stray global
    /// keys → install fresh `console`/`camel` → install-verify (on failure:
    /// recycle the realm, recheck the deadline, retry install+verify once;
    /// a second consecutive failure fails the job after one more recycle)
    /// → evaluate the cached wrapper → convert + extract → integrity-check
    /// (drift → recycle).
    ///
    /// Scrub and the integrity check run on every completion path —
    /// success, script error, and limit failure (the panic path is covered
    /// by the dispatch loop's `catch_unwind` plus a full state rebuild,
    /// which subsumes recycling). Wrapper and inner-eval failures map to
    /// [`JsLanguageError::Execution`], exactly as the pre-worker `ctx.eval`
    /// path mapped them.
    pub(super) fn run_eval(
        &mut self,
        source: &str,
        exchange: JsExchange,
        deadline: Instant,
    ) -> Result<JsEvalResult, JsLanguageError> {
        // Job deadline. The dequeue-time check in the dispatch loop already
        // skipped expired jobs; this re-check guards the budget for the
        // recycle-retry path inside this method.
        if Instant::now() > deadline {
            return Err(JsLanguageError::Execution {
                message: "JS execution timeout".to_string(),
            });
        }
        // Re-enter the stable realm explicitly: the persistent context must
        // always evaluate against the same realm.
        self.ctx.enter_realm(self.realm.clone());

        // Install fresh bindings and verify the protected invariants. A
        // rejected global set (spoofed non-writable binding) or a failed
        // verify recycles the realm, rechecks the job deadline (a recycle
        // costs on the order of a fresh realm build), and retries
        // install+verify exactly once. Deterministic exchange-data errors
        // are surfaced directly — they recur identically on a fresh realm.
        let mut verified = false;
        for attempt in 0..2 {
            self.scrub_globals();
            match install_bindings(&mut self.ctx, &exchange) {
                Ok(installed) => {
                    if self.install_verify(&installed) {
                        verified = true;
                        break;
                    }
                }
                Err(InstallError::Data(e)) => return Err(e),
                Err(InstallError::Rejected) => {}
            }
            self.recycle();
            if attempt == 0 && Instant::now() > deadline {
                return Err(JsLanguageError::Execution {
                    message: "JS execution timeout".to_string(),
                });
            }
        }
        if !verified {
            // Second consecutive install/verify failure. The loop's last
            // `recycle` already ran; surface the failure to the caller.
            return Err(JsLanguageError::Execution {
                message: "JS engine realm failed verification".to_string(),
            });
        }

        let outcome = self.eval_cached(source);

        // Integrity verification runs after the eval regardless of its
        // outcome; drift recycles the realm for the NEXT job. The current
        // job's result is still reported.
        if !self.integrity_check() {
            self.recycle();
        }
        outcome
    }

    /// Evaluate the cached wrapper for `source`, convert the completion
    /// value, and extract the exchange state.
    ///
    /// Cache lookup by full source bytes; on a miss, compile the wrapper
    /// `eval("<escaped source>")` (parse is pure — it creates no bindings),
    /// evaluate it — the wrapper's completion value is the inner eval's
    /// completion value — and cache the compiled script.
    fn eval_cached(&mut self, source: &str) -> Result<JsEvalResult, JsLanguageError> {
        // Field split so the cache and the context can be borrowed disjointly.
        let WorkerState { ctx, cache, .. } = self;

        let key: Arc<str> = Arc::from(source);

        let outcome = match cache.get(&key) {
            Some(script) => script.evaluate(ctx),
            None => {
                let wrapper = format!("eval(\"{}\")", escape_js_string_literal(source));
                let script = Script::parse(Source::from_bytes(wrapper.as_bytes()), None, ctx)
                    .map_err(|e| JsLanguageError::Execution {
                        message: e.to_string(),
                    })?;
                let outcome = script.evaluate(ctx);
                cache.insert(key, script);
                outcome
            }
        };

        let result = outcome.map_err(|e| JsLanguageError::Execution {
            message: e.to_string(),
        })?;

        let return_value = js_to_value(&result, ctx)?;

        // Extract modified exchange state.
        let modified = bindings::extract_camel_state(ctx)?;

        Ok(JsEvalResult {
            return_value,
            headers: modified.headers,
            body: modified.body,
            properties: modified.properties,
        })
    }

    /// Delete every own key of the global object that is not in the
    /// baseline key set, ignoring delete failures — a non-configurable key
    /// remains and is caught as drift by the integrity check, which
    /// recycles the realm.
    ///
    /// Redundant by design: any key scrub can remove would also register
    /// as drift in the integrity check and trigger a recycle. The spec
    /// blesses keeping the scrub; it is this trivial loop (revisit at
    /// archive).
    fn scrub_globals(&mut self) {
        let global = self.ctx.global_object();
        let Ok(keys) = global.own_property_keys(&mut self.ctx) else {
            return;
        };
        for key in keys {
            if !self.baseline.contains_global_key(&key) {
                let _ = global.delete_property_or_throw(key, &mut self.ctx);
            }
        }
    }

    /// Verify the protected invariants for the next eval: the global OWN
    /// values of `camel` and `console` strictly equal the freshly installed
    /// bindings (catches a spoofed non-writable binding, whose rejected
    /// replacement leaves the spoof in place, and accessor traps whose set
    /// "succeeds" without replacing the value), and the global `eval` own
    /// value strictly equals the baseline-captured `eval` function.
    /// `false` on any miss.
    ///
    /// Own-key lookups, deliberately not `get`: the global object's
    /// prototype chain is user-mutable, so a chain lookup could
    /// false-positive on an inherited binding after a deletion.
    fn install_verify(&mut self, installed: &InstalledGlobals) -> bool {
        let Some(eval_object) = self.baseline.eval_object() else {
            return false;
        };
        let eval_object = eval_object.clone();
        let global = self.ctx.global_object();
        for (key, expected) in [
            (js_string!("camel"), JsValue::from(installed.camel.clone())),
            (
                js_string!("console"),
                JsValue::from(installed.console.clone()),
            ),
            (js_string!("eval"), JsValue::from(eval_object)),
        ] {
            let Some(desc) = global.borrow().properties().get(&PropertyKey::from(key)) else {
                return false;
            };
            let Some(value) = desc.value() else {
                return false;
            };
            if !value.strict_equals(&expected) {
                return false;
            }
        }
        true
    }

    /// Verify the named integrity set against the baseline (see
    /// [`IntegrityBaseline::verify`]): global own-key set (including symbol
    /// keys) and `eval` value identity, plus each root's key set,
    /// descriptor hash, and prototype identity. Any difference → `false`.
    fn integrity_check(&mut self) -> bool {
        let WorkerState { ctx, baseline, .. } = self;
        baseline.verify(ctx)
    }

    /// Replace the current realm with a fresh one: create it on the
    /// persistent context, enter it, re-apply the runtime limits, re-install
    /// the minimal `camel`/`console` bindings, re-capture the integrity
    /// baseline, and drop the realm-bound wrapper cache. On `create_realm`
    /// failure (or a failed minimal install on the new realm), rebuild the
    /// whole `WorkerState`. Recycling is the entire recovery story — the
    /// worker never attempts restoration of drifted state.
    fn recycle(&mut self) {
        self.recycle_count += 1;
        let limits = self.limits.clone();
        if let Ok(realm) = self.ctx.create_realm() {
            // `create_realm` binds default globals on the new realm but
            // RETURNS with the OLD realm re-activated — enter the new one.
            let _previous = self.ctx.enter_realm(realm.clone());
            self.realm = realm;
            apply_runtime_limits(&mut self.ctx, &self.limits);
            if install_bindings(&mut self.ctx, &JsExchange::default()).is_ok() {
                self.baseline = IntegrityBaseline::capture(&mut self.ctx);
                self.cache.clear();
                return;
            }
        }
        // `create_realm` failed or the minimal install on the fresh realm
        // failed: rebuild everything from scratch, preserving the count so
        // tests can still observe that a recycle happened.
        let count = self.recycle_count;
        *self = WorkerState::new(limits);
        self.recycle_count = count;
    }

    /// Parse-only validation on the worker's persistent context. The RAW
    /// source goes through `Script::parse` — no wrapper: the wrapper of an
    /// invalid source is itself valid JS, which would swallow the parse
    /// error — and is never evaluated, so no global or lexical bindings are
    /// created and the stable realm stays clean. Failures map to
    /// [`JsLanguageError::Parse`].
    pub(super) fn run_validate(&mut self, source: &str) -> Result<(), JsLanguageError> {
        let _script = Script::parse(Source::from_bytes(source.as_bytes()), None, &mut self.ctx)
            .map_err(|e| JsLanguageError::Parse {
                message: e.to_string(),
            })?;
        Ok(())
    }

    /// Sleep for `millis`, then reply with a default result (test-only job).
    #[cfg(test)]
    fn run_test_block(&self, millis: u64) -> Result<JsEvalResult, JsLanguageError> {
        std::thread::sleep(Duration::from_millis(millis));
        Ok(JsEvalResult {
            return_value: Value::Null,
            headers: HashMap::new(),
            body: Value::Null,
            properties: HashMap::new(),
        })
    }

    /// Deliberate panic to exercise the self-heal path (test-only job).
    #[cfg(test)]
    fn run_test_panic(&self) -> Result<JsEvalResult, JsLanguageError> {
        panic!("TestPanic: deliberate worker panic for the self-heal test");
    }
}

/// Apply the resolved runtime limits to a context.
fn apply_runtime_limits(ctx: &mut Context, limits: &ResolvedJsLimits) {
    let runtime_limits = ctx.runtime_limits_mut();
    runtime_limits.set_loop_iteration_limit(limits.max_loop_iterations);
    runtime_limits.set_recursion_limit(limits.max_recursion_depth);
    runtime_limits.set_stack_size_limit(limits.max_stack_size);
}

/// Worker-thread main loop: a thin recv-dispatch shell. All per-job logic
/// lives in [`WorkerState`] methods.
fn worker_loop(rx: mpsc::Receiver<JsJob>, limits: camel_language_api::JsLimitsConfig) {
    let resolved = resolve_js_limits(&limits);
    let mut state = WorkerState::new(resolved);
    while let Ok(job) = rx.recv() {
        let panicked = match job {
            JsJob::Eval {
                source,
                exchange,
                timeout_ms,
                enqueued,
                reply,
            } => {
                // Deadline check at dequeue: an expired job never executes.
                if enqueued.elapsed() > Duration::from_millis(timeout_ms) {
                    let _ = reply.send(Err(JsLanguageError::Execution {
                        message: "JS execution timeout".to_string(),
                    }));
                    continue;
                }
                let deadline = enqueued + Duration::from_millis(timeout_ms);
                let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    state.run_eval(&source, exchange, deadline)
                }));
                reply_outcome(&reply, attempt)
            }
            JsJob::Validate { source, reply } => {
                let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    state.run_validate(&source)
                }));
                reply_outcome(&reply, attempt)
            }
            #[cfg(test)]
            JsJob::TestBlock { millis, reply } => {
                let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    state.run_test_block(millis)
                }));
                reply_outcome(&reply, attempt)
            }
            #[cfg(test)]
            JsJob::TestPanic { reply } => {
                let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    state.run_test_panic()
                }));
                reply_outcome(&reply, attempt)
            }
        };
        if panicked {
            // Self-heal: a panic invalidates the engine state — rebuild the
            // full worker state (fresh `Context` + realm, empty script cache)
            // before the next job. Preserve the recycle count for test
            // observability, matching `recycle()`'s rebuild semantics.
            let count = state.recycle_count;
            state = WorkerState::new(resolve_js_limits(&limits));
            state.recycle_count = count;
        }
    }
}

/// Send the job outcome, never blocking on a gone receiver; returns `true`
/// when the attempt panicked so the loop can rebuild its state.
fn reply_outcome<T>(
    reply: &mpsc::SyncSender<Result<T, JsLanguageError>>,
    attempt: std::thread::Result<Result<T, JsLanguageError>>,
) -> bool {
    match attempt {
        Ok(outcome) => {
            let _ = reply.send(outcome);
            false
        }
        Err(_) => {
            let _ = reply.send(Err(JsLanguageError::Execution {
                message: "JS worker panic recovered".to_string(),
            }));
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    use camel_language_api::{Exchange, JsLimitsConfig, Language, Message};

    use crate::engine::JsEngine as _;
    use crate::language::JsLanguage;

    /// Run an eval with a generous 5 s deadline (test helper mirroring the
    /// dispatch loop's deadline computation).
    fn eval_now(
        state: &mut WorkerState,
        source: &str,
        exchange: JsExchange,
    ) -> Result<JsEvalResult, JsLanguageError> {
        state.run_eval(source, exchange, Instant::now() + Duration::from_secs(5))
    }

    /// Local `WorkerState` under default limits.
    fn worker_state() -> WorkerState {
        WorkerState::new(resolve_js_limits(&JsLimitsConfig::default()))
    }

    #[test]
    fn expired_queued_job_is_skipped() {
        let handle = JsWorkerHandle::spawn(JsLimitsConfig::default());

        // Occupy the worker for 300 ms so the next job queues behind it.
        let (block_tx, _block_rx) = mpsc::sync_channel(1);
        handle
            .tx
            .send(JsJob::TestBlock {
                millis: 300,
                reply: block_tx,
            })
            .unwrap();

        // Queue an `Eval` whose 1 ms budget expires while the worker is busy.
        let (tx, rx) = mpsc::sync_channel(1);
        handle
            .tx
            .send(JsJob::Eval {
                source: Arc::from("1+1"),
                exchange: JsExchange::default(),
                timeout_ms: 1,
                enqueued: Instant::now(),
                reply: tx,
            })
            .unwrap();

        let reply = rx.recv().unwrap();
        let err = reply.expect_err("expired job must be skipped with an error");
        assert!(
            err.to_string().contains("timeout"),
            "expected timeout error, got: {err}"
        );

        // The worker stays healthy after skipping the expired job.
        let (tx, rx) = mpsc::sync_channel(1);
        handle
            .tx
            .send(JsJob::Eval {
                source: Arc::from("1+1"),
                exchange: JsExchange::default(),
                timeout_ms: 5_000,
                enqueued: Instant::now(),
                reply: tx,
            })
            .unwrap();
        let result = rx.recv().unwrap().unwrap();
        assert_eq!(result.return_value.as_i64().unwrap(), 2);
    }

    #[test]
    fn worker_self_heals_after_panic() {
        let handle = JsWorkerHandle::spawn(JsLimitsConfig::default());

        let (tx, rx) = mpsc::sync_channel(1);
        handle.tx.send(JsJob::TestPanic { reply: tx }).unwrap();
        let reply = rx.recv().unwrap();
        let err = reply.expect_err("TestPanic must produce an error");
        assert!(
            err.to_string().contains("panic"),
            "expected panic error, got: {err}"
        );

        // The worker rebuilt its state and still evaluates normally.
        let (tx, rx) = mpsc::sync_channel(1);
        handle
            .tx
            .send(JsJob::Eval {
                source: Arc::from("1+1"),
                exchange: JsExchange::default(),
                timeout_ms: 5_000,
                enqueued: Instant::now(),
                reply: tx,
            })
            .unwrap();
        let result = rx.recv().unwrap().unwrap();
        assert_eq!(result.return_value.as_i64().unwrap(), 2);
    }

    #[test]
    fn lexical_declarations_are_fresh_per_eval() {
        // NOTE (task 1.2 deviation): the task text specifies the source
        // `let count = (typeof count === 'undefined') ? 0 : count; count++; count`.
        // That source throws `ReferenceError: count is not defined` in Boa 0.22 —
        // the initializer references its own `let` binding while it is still in
        // the temporal dead zone (`typeof` throws; it does not yield
        // `'undefined'`) — on today's fresh-`Context` path as well, so the
        // "both return 1" assertion is unachievable as written. The test below
        // keeps the documented intent (repeated evals of a lexically declaring
        // source return the fresh value; no redeclaration error) with the
        // minimal TDZ-free source. Reported as test-design-gap.
        let engine = super::super::boa::BoaEngine::default();
        let source = "let count = 0; count++; count";

        let r1 = engine.eval(source, JsExchange::default()).unwrap();
        let r2 = engine.eval(source, JsExchange::default()).unwrap();

        assert_eq!(r1.return_value.as_i64().unwrap(), 1);
        assert_eq!(r2.return_value.as_i64().unwrap(), 1);
    }

    #[test]
    fn wrapper_escapes_adversarial_sources() {
        // Expected-value table: source → exact result under today's raw-source
        // semantics (recorded empirically against Boa 0.22). Sources 3–6 are
        // the string-literal forms: as bare tokens they are invalid JS (or
        // reference undefined identifiers), while the recorded expected values
        // (`back\slash`, `trailing\`, …) are the raw-eval results of the
        // quoted forms.
        let table: Vec<(String, String)> = vec![
            (r#"'it\'s'"#.to_string(), "it's".to_string()),
            (r#""quo\"te""#.to_string(), "quo\"te".to_string()),
            (r#""back\\slash""#.to_string(), "back\\slash".to_string()),
            ("\"cr\\rLf\\n\"".to_string(), "cr\rLf\n".to_string()),
            (
                // Raw U+2028/U+2029 bytes inside the literal: the wrapper
                // escape must turn them into \u2028/\u2029 escapes.
                "\"ls\u{2028}ps\u{2029}\"".to_string(),
                "ls\u{2028}ps\u{2029}".to_string(),
            ),
            ("\"trailing\\\\\"".to_string(), "trailing\\".to_string()),
        ];
        let distinct_sources = table.len();

        let mut state = worker_state();
        for (source, expected) in &table {
            for _ in 0..3 {
                let result = eval_now(&mut state, source, JsExchange::default())
                    .unwrap_or_else(|e| panic!("eval must succeed for {source:?}: {e}"));
                assert_eq!(
                    result.return_value.as_str(),
                    Some(expected.as_str()),
                    "adversarial source must round-trip: {source:?}"
                );
            }
        }

        // One wrapper entry per distinct source after 3 repeats each.
        assert_eq!(state.cache.len(), distinct_sources);
    }

    #[test]
    fn invalid_source_maps_to_execution_error() {
        let mut state = worker_state();

        // Eval: the wrapper of an invalid source is itself valid JS, so the
        // failure surfaces at inner-eval time → Execution (never Parse).
        let err = eval_now(&mut state, "let x = {{{", JsExchange::default())
            .expect_err("invalid source must fail eval");
        assert!(
            matches!(err, JsLanguageError::Execution { .. }),
            "eval of invalid source must be Execution, got: {err:?}"
        );

        // Validate: raw-source parse → Parse.
        let verr = state
            .run_validate("let x = {{{")
            .expect_err("invalid source must fail validate");
        assert!(
            matches!(verr, JsLanguageError::Parse { .. }),
            "validate of invalid source must be Parse, got: {verr:?}"
        );
    }

    #[test]
    fn identical_results_across_repeats() {
        let mut state = worker_state();
        let source = "1 + 1";
        let exchange = JsExchange::default();

        let r1 = eval_now(&mut state, source, exchange.clone()).unwrap();
        let r2 = eval_now(&mut state, source, exchange.clone()).unwrap();
        let r3 = eval_now(&mut state, source, exchange).unwrap();

        assert_eq!(r1, r2);
        assert_eq!(r2, r3);
    }

    #[test]
    fn identical_errors_first_and_cached() {
        let mut state = worker_state();
        let source = "throw new Error('boom')";

        let e1 =
            eval_now(&mut state, source, JsExchange::default()).expect_err("first eval must fail");
        let e2 = eval_now(&mut state, source, JsExchange::default())
            .expect_err("cached eval must fail identically");

        // JsLanguageError has no PartialEq: compare via variant + message.
        let message_of = |e: &JsLanguageError| match e {
            JsLanguageError::Execution { message } => message.clone(),
            other => panic!("expected Execution error, got: {other:?}"),
        };
        let m1 = message_of(&e1);
        let m2 = message_of(&e2);
        assert!(m1.contains("boom"), "error must carry the JS message: {m1}");
        assert_eq!(m1, m2, "first and cached error must be identical");
    }

    #[tokio::test]
    async fn route_rebuild_reuses_cache_entry() {
        // WorkerState level: two "expression builds" sharing one engine (the
        // watch-reload shape) reuse one cache entry.
        let mut state = worker_state();
        let source = "1 + 1";
        let r1 = eval_now(&mut state, source, JsExchange::default()).unwrap();
        let r2 = eval_now(&mut state, source, JsExchange::default()).unwrap();
        assert_eq!(r1, r2);
        assert_eq!(state.cache.len(), 1);

        // Engine level: one JsLanguage, create_expression twice, identical
        // evaluated values.
        let lang = JsLanguage::new();
        let expr1 = lang.create_expression("1 + 1").unwrap();
        let expr2 = lang.create_expression("1 + 1").unwrap();
        let exchange = Exchange::new(Message::default());
        let v1 = expr1.evaluate(&exchange).await.unwrap();
        let v2 = expr2.evaluate(&exchange).await.unwrap();
        assert_eq!(v1, v2);
    }

    #[test]
    fn lru_eviction_bounds_cache_at_256() {
        let mut ctx = Context::default();
        let mut cache = ScriptCache::new(256);

        let keys: Vec<Arc<str>> = (0..257)
            .map(|i| Arc::<str>::from(format!("source-{i}").as_str()))
            .collect();
        for key in &keys {
            let script = Script::parse(Source::from_bytes(b"0"), None, &mut ctx).unwrap();
            cache.insert(key.clone(), script);
        }

        assert_eq!(cache.len(), 256, "cache must stay bounded at the cap");
        assert!(cache.get(&keys[0]).is_none(), "oldest key must be evicted");
        assert!(
            cache.get(&keys[256]).is_some(),
            "most recently inserted key must be present"
        );
    }

    // ── Task 1.3: cleanup, install-verify, integrity set, realm recycling ──

    #[test]
    fn global_assignment_does_not_persist() {
        let mut state = worker_state();
        let r = eval_now(&mut state, "globalThis.leak = 42; 0", JsExchange::default()).unwrap();
        assert_eq!(r.return_value.as_i64(), Some(0));

        let r = eval_now(&mut state, "typeof leak", JsExchange::default()).unwrap();
        assert_eq!(
            r.return_value.as_str(),
            Some("undefined"),
            "global assignment must not persist across evals"
        );
    }

    #[test]
    fn poisoned_eval_recovers() {
        // Through one BoaEngine: behavioral proof that `eval = 1` poisoning
        // triggers recovery (install-verify eval-identity check + recycle)
        // BEFORE the next eval runs.
        let engine = super::super::boa::BoaEngine::default();
        let r1 = engine.eval("eval = 1; 0", JsExchange::default()).unwrap();
        assert_eq!(r1.return_value.as_i64(), Some(0));

        let r2 = engine.eval("typeof eval", JsExchange::default()).unwrap();
        assert_eq!(
            r2.return_value.as_str(),
            Some("function"),
            "poisoned eval must be restored via realm recycle before the next eval"
        );
    }

    #[test]
    fn undeletable_global_recycles_realm() {
        let mut state = worker_state();
        eval_now(
            &mut state,
            "Object.defineProperty(globalThis, 'pwn', { value: 1, configurable: false }); 0",
            JsExchange::default(),
        )
        .unwrap();

        let r = eval_now(&mut state, "typeof pwn", JsExchange::default()).unwrap();
        assert_eq!(
            r.return_value.as_str(),
            Some("undefined"),
            "undeletable global key must be detected as drift and the realm recycled"
        );
    }

    #[test]
    fn frozen_prototype_recycles_realm() {
        let mut state = worker_state();
        eval_now(
            &mut state,
            "Object.freeze(Object.prototype); 0",
            JsExchange::default(),
        )
        .unwrap();

        let r = eval_now(
            &mut state,
            "Object.isFrozen(Object.prototype)",
            JsExchange::default(),
        )
        .unwrap();
        assert_eq!(
            r.return_value.as_bool(),
            Some(false),
            "descriptor drift on Object.prototype must recycle the realm"
        );
    }

    #[test]
    fn failed_eval_residue_is_cleaned() {
        let mut state = worker_state();
        let err = eval_now(
            &mut state,
            "globalThis.x = 1; throw new Error('boom')",
            JsExchange::default(),
        );
        assert!(err.is_err(), "eval must fail with the thrown error");

        let r = eval_now(&mut state, "typeof x", JsExchange::default()).unwrap();
        assert_eq!(
            r.return_value.as_str(),
            Some("undefined"),
            "cleanup must run on the error path"
        );
    }

    #[test]
    fn limit_failure_residue_is_cleaned() {
        let mut state = worker_state();
        let err = eval_now(
            &mut state,
            "globalThis.x = 1; while (true) {}",
            JsExchange::default(),
        );
        assert!(
            err.is_err(),
            "infinite loop must trip the loop-iteration limit"
        );

        let r = eval_now(&mut state, "typeof x", JsExchange::default()).unwrap();
        assert_eq!(
            r.return_value.as_str(),
            Some("undefined"),
            "cleanup must run on the limit-failure path"
        );
    }

    #[test]
    fn console_and_camel_are_fresh_each_eval() {
        let mut state = worker_state();
        eval_now(
            &mut state,
            "console.log = function(){}; camel.headers = 1; 0",
            JsExchange::default(),
        )
        .unwrap();

        // The re-installed console must still route to tracing (fresh
        // console.log) and the re-installed camel must serve this exchange.
        let exchange = JsExchange::from_headers_body_properties(
            [("k".to_string(), serde_json::json!("v"))]
                .into_iter()
                .collect(),
            serde_json::Value::Null,
            HashMap::new(),
        );
        let r = eval_now(
            &mut state,
            "console.log('x'); camel.headers.get('k')",
            exchange,
        )
        .unwrap();
        assert_eq!(
            r.return_value.as_str(),
            Some("v"),
            "camel/console must be re-installed fresh with the new exchange"
        );
    }

    #[test]
    fn spoofed_non_writable_binding_recycles() {
        // Spoof: replace the camel global with a non-writable binding. No
        // key-set drift occurs (camel/console values are excluded from
        // identity comparison), so the integrity check cannot catch it —
        // install-verify must, by strict-equaling the global own values
        // against the freshly installed bindings, and recycle the realm
        // at the next eval's start.
        let mut state = worker_state();
        eval_now(
            &mut state,
            "Object.defineProperty(globalThis, 'camel', { value: { pwn: 1 }, writable: false }); 0",
            JsExchange::default(),
        )
        .unwrap();

        let r = eval_now(&mut state, "camel.pwn", JsExchange::default()).unwrap();
        assert!(
            r.return_value.is_null(),
            "spoofed non-writable camel must be replaced via realm recycle before the eval, got {:?}",
            r.return_value
        );

        // The recycled realm serves the real camel binding.
        let exchange = JsExchange::from_headers_body_properties(
            HashMap::new(),
            serde_json::Value::String("payload".to_string()),
            HashMap::new(),
        );
        let r = eval_now(&mut state, "camel.body", exchange).unwrap();
        assert_eq!(r.return_value.as_str(), Some("payload"));
    }

    #[test]
    fn date_prototype_persists_until_recycle() {
        // Declared non-guarantee: `Date.prototype` is OUTSIDE the named
        // integrity set, so its mutation persists between evals of one realm
        // generation and disappears after a drift-triggered recycle.
        let mut state = worker_state();
        eval_now(
            &mut state,
            "Date.prototype.probe = 1; 0",
            JsExchange::default(),
        )
        .unwrap();

        let r = eval_now(&mut state, "new Date().probe", JsExchange::default()).unwrap();
        assert_eq!(
            r.return_value.as_i64(),
            Some(1),
            "state outside the integrity set persists within a realm generation"
        );

        // Drift trigger: an undeletable global recycles the realm.
        eval_now(
            &mut state,
            "Object.defineProperty(globalThis, 'pwn', { value: 1, configurable: false }); 0",
            JsExchange::default(),
        )
        .unwrap();

        let r = eval_now(&mut state, "new Date().probe", JsExchange::default()).unwrap();
        assert!(
            r.return_value.is_null(),
            "recycled realm must drop the persisted Date.prototype state, got {:?}",
            r.return_value
        );
    }

    #[test]
    fn no_recycle_storm_on_steady_state() {
        // camel/console are re-installed (new object identity) every eval;
        // that churn must NOT register as drift.
        let mut state = worker_state();
        let exchange = JsExchange::from_headers_body_properties(
            HashMap::new(),
            serde_json::Value::String("payload".to_string()),
            HashMap::new(),
        );
        for _ in 0..100 {
            eval_now(&mut state, "camel.body", exchange.clone()).unwrap();
        }
        assert_eq!(
            state.recycle_count, 0,
            "steady-state evals must not trigger realm recycles"
        );
    }

    #[test]
    #[ignore = "slow test: timing diagnostic; run via cargo test -p camel-language-js integrity_detector -- --ignored --nocapture; printed mean feeds the Req-4 shrink decision"]
    fn integrity_detector_cost_diagnostic() {
        let mut state = worker_state();
        // Warm up outside the measurement window.
        for _ in 0..20 {
            state.integrity_check();
        }
        let start = Instant::now();
        for _ in 0..200 {
            state.integrity_check();
        }
        let mean_us = start.elapsed().as_micros() as f64 / 200.0;
        println!(
            "integrity_detector_cost_diagnostic: mean {mean_us:.2} µs per integrity_check over 200 calls"
        );
    }
}
