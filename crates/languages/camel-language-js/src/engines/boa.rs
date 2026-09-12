//! [`BoaEngine`] — JS engine backed by [Boa](https://boajs.dev).
//!
//! Evaluation and validation run on a dedicated `camel-js-worker` OS thread
//! (one worker per limits-configuration; `BoaEngine` clones share the worker
//! through an `Arc<OnceLock<..>>`). Calls enqueue a job over a bounded
//! channel and block on a per-call reply channel.
//!
//! Per-eval realm strategy: evals run on the worker's ONE stable realm
//! through an `eval(...)` wrapper with a bounded compiled-wrapper cache.
//! Each evaluation receives fresh `camel` and `console` bindings and a
//! fresh declarative environment; configurable global additions are
//! removed, and a named integrity set (the `globalThis` own keys, the
//! `eval` function, and the `Object`/`Array`/`Function` prototypes) is
//! verified between evaluations, with the whole realm recycled on drift
//! (see `worker.rs` and `integrity.rs`). JavaScript evaluations do
//! NOT receive realm isolation: global properties, intrinsic state outside
//! the named integrity set, heap state, and engine-internal state may
//! survive across exchanges and routes until realm recycling or process
//! termination — see the crate `CONTEXT.md` "Sandbox posture" section for
//! the full contract.
//!
//! If the worker thread dies, the stale handle surfaces as
//! `"JS worker unavailable"`; recreating the `JsLanguage` is the recovery
//! path.
//!
//! If [`JsLimitsConfig`](camel_language_api::JsLimitsConfig) fields are `None`, the rust-camel runtime defaults apply:
//!
//! | Limit | Default |
//! |---|---|
//! | `execution_timeout_ms` | 5,000 ms |
//! | `max_loop_iterations` | 100,000 (Boa upstream is `u64::MAX`) |
//! | `max_recursion_depth` | 512 (Boa 0.21 upstream default, pinned) |
//! | `max_stack_size` | 10,240 (Boa 0.21 upstream default, pinned) |
//!
//! **Heap cap:** not supported by Boa 0.21.

/// Maximum source-string size accepted by [`BoaEngine::eval`] (DoS cap, M-L1).
///
/// Boa 0.21 exposes no heap/allocation cap (`runtime_limits_mut()` covers only
/// loop iterations, recursion depth, and stack size). This pre-eval source-size
/// check neutralizes large-payload bombs before Boa allocates; the residual
/// in-heap amplification vector (a small source that grows a huge structure via
/// `String.prototype.repeat` or array builders) cannot be bounded without a Boa
/// heap API and is accepted as a documented upstream limitation. The existing
/// loop/recursion/stack/timeout limits neutralize CPU-bombs.
const MAX_SOURCE_BYTES: usize = 1024 * 1024; // 1 MiB

/// Default wall-clock execution budget (mirrors the `JsLanguage` default) used
/// as the worker's queuing-deadline backstop for `Eval` jobs.
const DEFAULT_EXECUTION_TIMEOUT_MS: u64 = 5_000;

use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use crate::{
    engine::{JsEngine, JsEvalResult, JsExchange},
    error::JsLanguageError,
};

use super::worker::{JsJob, JsWorkerHandle, worker_unavailable};

/// A [`JsEngine`] implementation backed by Boa, executing on a dedicated
/// worker thread.
///
/// All jobs for one limits-configuration run on a single `camel-js-worker`
/// thread; clones of `BoaEngine` share that worker. Each evaluation
/// receives fresh `camel`/`console` bindings and a fresh declarative
/// environment through the worker's stable realm; configurable global
/// additions are removed and a named integrity set is verified between
/// evaluations, with the realm recycled on drift. Evaluations do not
/// receive realm isolation — see the crate `CONTEXT.md` "Sandbox posture"
/// section.
#[derive(Debug, Clone)]
pub struct BoaEngine {
    limits: camel_language_api::JsLimitsConfig,
    worker: Arc<OnceLock<JsWorkerHandle>>,
}

impl BoaEngine {
    #[must_use]
    pub fn new(limits: camel_language_api::JsLimitsConfig) -> Self {
        Self {
            limits,
            worker: Arc::new(OnceLock::new()),
        }
    }

    /// Lazily spawn (or reuse) the single worker for this limits-configuration.
    ///
    /// `get_or_init` guarantees one worker even under concurrent first calls;
    /// clones share the same `OnceLock` through the `Arc`.
    fn worker(&self) -> &JsWorkerHandle {
        self.worker
            .get_or_init(|| JsWorkerHandle::spawn(self.limits.clone()))
    }

    /// Send a job and block on its reply. `Err` means the worker is gone.
    fn dispatch<T>(
        &self,
        make_job: impl FnOnce(mpsc::SyncSender<Result<T, JsLanguageError>>) -> JsJob,
    ) -> Result<T, JsLanguageError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.worker().send(make_job(reply_tx))?;
        reply_rx.recv().map_err(|_| worker_unavailable())?
    }
}

impl Default for BoaEngine {
    fn default() -> Self {
        Self::new(camel_language_api::JsLimitsConfig::default())
    }
}

// ── Resolver (shared with the worker thread) ──────────────────────────────────

/// Resolved (concrete) JS limits after folding `Option` → `T` with rust-camel
/// runtime defaults. Produced by [`resolve_js_limits`].
///
/// **Heap cap gap:** Boa 0.21 does not expose a heap-size limit. The
/// [`JsLimitsConfig`] struct intentionally lacks a `max_heap_size` field;
/// `deny_unknown_fields` in serde rejects it if a user tries to set it.
///
/// Note: `execution_timeout_ms` is NOT in this struct — it is applied at the
/// [`Language`](camel_language_api::Language) level via `eval_async` tokio
/// timeout in `expression.rs`, not through Boa's `RuntimeLimits`.
#[derive(Clone)]
pub(super) struct ResolvedJsLimits {
    pub(super) max_loop_iterations: u64,
    pub(super) max_recursion_depth: usize,
    pub(super) max_stack_size: usize,
}

/// Resolve a `JsLimitsConfig` (all-`Option`) into concrete values, applying
/// rust-camel runtime defaults where the user did not specify a value.
pub(super) fn resolve_js_limits(limits: &camel_language_api::JsLimitsConfig) -> ResolvedJsLimits {
    ResolvedJsLimits {
        // Boa upstream default for loop is u64::MAX — unacceptable for buggy scripts.
        max_loop_iterations: limits.max_loop_iterations.unwrap_or(100_000),
        max_recursion_depth: limits.max_recursion_depth.unwrap_or(512),
        max_stack_size: limits.max_stack_size.unwrap_or(10_240),
    }
}

impl JsEngine for BoaEngine {
    fn eval(&self, source: &str, exchange: JsExchange) -> Result<JsEvalResult, JsLanguageError> {
        // M-L1: pre-eval source-size cap (Boa 0.21 has no heap cap; see const doc).
        // Stays on the caller side, before the job is sent.
        if source.len() > MAX_SOURCE_BYTES {
            return Err(JsLanguageError::Execution {
                message: format!(
                    "JS source {} bytes exceeds max source bytes {} (Boa 0.21 has no heap cap; \
                     reject oversized input before eval)",
                    source.len(),
                    MAX_SOURCE_BYTES
                ),
            });
        }

        self.dispatch(|reply| JsJob::Eval {
            source: Arc::from(source),
            exchange,
            // Queuing-deadline backstop: a job that sits in the queue past its
            // budget is skipped. The wall-clock execution timeout itself is
            // still applied at the Language level via `eval_async`.
            timeout_ms: self
                .limits
                .execution_timeout_ms
                .unwrap_or(DEFAULT_EXECUTION_TIMEOUT_MS),
            enqueued: Instant::now(),
            reply,
        })
    }

    fn validate(&self, source: &str) -> Result<(), JsLanguageError> {
        self.dispatch(|reply| JsJob::Validate {
            source: Arc::from(source),
            reply,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_exchange() -> JsExchange {
        JsExchange::from_headers_body_properties(
            [("foo".to_string(), json!("bar"))].into_iter().collect(),
            json!("hello"),
            [("key".to_string(), json!("val"))].into_iter().collect(),
        )
    }

    #[test]
    fn test_eval_return_value() {
        let engine = BoaEngine::default();
        let result = engine.eval("1 + 1", JsExchange::default()).unwrap();
        assert_eq!(result.return_value.as_i64().unwrap(), 2);
    }

    #[test]
    fn test_eval_header_access() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let result = engine.eval("camel.headers.get('foo')", ex).unwrap();
        assert_eq!(result.return_value.as_str().unwrap(), "bar");
    }

    #[test]
    fn test_eval_body_getter() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let result = engine.eval("camel.body", ex).unwrap();
        assert_eq!(result.return_value.as_str().unwrap(), "hello");
    }

    #[test]
    fn test_mutating_set_header_propagates() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let result = engine
            .eval("camel.headers.set('newkey', 'newval'); 'done'", ex)
            .unwrap();
        assert_eq!(result.return_value.as_str().unwrap(), "done");
        assert_eq!(
            result.headers.get("newkey").unwrap().as_str().unwrap(),
            "newval"
        );
    }

    #[test]
    fn test_mutating_body_propagates() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let result = engine
            .eval("camel.body = 'modified'; camel.body", ex)
            .unwrap();
        assert_eq!(result.body.as_str().unwrap(), "modified");
    }

    #[test]
    fn test_console_log_no_crash() {
        let engine = BoaEngine::default();
        let result = engine
            .eval("console.log('test'); 42", JsExchange::default())
            .unwrap();
        assert_eq!(result.return_value.as_i64().unwrap(), 42);
    }

    #[test]
    fn test_validate_valid() {
        let engine = BoaEngine::default();
        assert!(engine.validate("let x = 1 + 1;").is_ok());
    }

    #[test]
    fn test_validate_invalid() {
        let engine = BoaEngine::default();
        assert!(engine.validate("let x = {{{").is_err());
    }

    #[test]
    fn test_eval_property_access() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let result = engine.eval("camel.properties.get('key')", ex).unwrap();
        assert_eq!(result.return_value.as_str().unwrap(), "val");
    }

    #[test]
    fn test_eval_property_function_access() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let result = engine.eval("camel.property('key')", ex).unwrap();
        assert_eq!(result.return_value.as_str().unwrap(), "val");
    }

    #[test]
    fn test_eval_runtime_error_returns_err() {
        let engine = BoaEngine::default();
        let result = engine.eval("throw new Error('boom')", JsExchange::default());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("boom")
                || msg.to_lowercase().contains("execution")
                || msg.to_lowercase().contains("error")
        );
    }

    #[test]
    fn test_eval_syntax_error_returns_err() {
        let engine = BoaEngine::default();
        let result = engine.eval("let x = {{{", JsExchange::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_eval_missing_header_returns_undefined() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        // Getting a key that doesn't exist should return undefined (maps to null in serde_json)
        let result = engine.eval("camel.headers.get('nonexistent')", ex).unwrap();
        assert!(result.return_value.is_null());
    }

    #[test]
    fn test_properties_mutation_propagates() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let result = engine
            .eval("camel.properties.set('newprop', 'newval'); 'done'", ex)
            .unwrap();
        assert_eq!(result.return_value.as_str().unwrap(), "done");
        assert_eq!(
            result.properties.get("newprop").unwrap().as_str().unwrap(),
            "newval"
        );
    }

    #[test]
    fn test_set_property_function_mutation_propagates() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let result = engine
            .eval("camel.set_property('newprop', 'newval'); 'done'", ex)
            .unwrap();
        assert_eq!(result.return_value.as_str().unwrap(), "done");
        assert_eq!(
            result.properties.get("newprop").unwrap().as_str().unwrap(),
            "newval"
        );
    }

    #[test]
    fn test_headers_keys() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let result = engine.eval("camel.headers.keys()", ex).unwrap();
        let keys: Vec<&str> = result
            .return_value
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(keys.contains(&"foo"));
    }

    #[test]
    fn test_headers_has() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let r1 = engine.eval("camel.headers.has('foo')", ex.clone()).unwrap();
        assert!(r1.return_value.as_bool().unwrap());
        let r2 = engine.eval("camel.headers.has('missing')", ex).unwrap();
        assert!(!r2.return_value.as_bool().unwrap());
    }

    #[test]
    fn test_headers_remove() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let result = engine
            .eval("camel.headers.remove('foo'); camel.headers.has('foo')", ex)
            .unwrap();
        assert!(!result.return_value.as_bool().unwrap());
        assert!(!result.headers.contains_key("foo"));
    }

    #[test]
    fn test_boa_infinite_loop_trips_loop_iteration_limit() {
        use camel_language_api::JsLimitsConfig;
        let limits = JsLimitsConfig {
            max_loop_iterations: Some(1_000),
            ..Default::default()
        };
        let engine = BoaEngine::new(limits);
        let result = engine.eval("while (true) {}", JsExchange::default());
        assert!(
            result.is_err(),
            "while(true) must trip loop_iteration_limit"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.to_lowercase().contains("loop")
                || msg.to_lowercase().contains("limit")
                || msg.to_lowercase().contains("iteration"),
            "error should reference loop limit: {msg}"
        );
    }

    #[test]
    fn test_eval_rejects_oversized_source() {
        // M-L1: source larger than MAX_SOURCE_BYTES is rejected before Boa eval.
        let engine = BoaEngine::default();
        let big = "x".repeat(MAX_SOURCE_BYTES + 1);
        let result = engine.eval(&big, JsExchange::default());
        assert!(result.is_err(), "oversized source must be rejected");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("source") && msg.to_lowercase().contains("bytes"),
            "error should mention source size: {msg}"
        );
    }

    #[test]
    fn test_eval_accepts_source_under_cap() {
        let engine = BoaEngine::default();
        // Small script well under the cap.
        let result = engine.eval("1 + 1", JsExchange::default()).unwrap();
        assert_eq!(result.return_value.as_i64().unwrap(), 2);
    }

    #[test]
    fn test_documented_heap_amplification_gap() {
        // M-L1 residual gap documentation test: Boa 0.21 exposes no heap cap.
        // The existing loop/recursion/stack/timeout limits neutralize CPU-bombs;
        // an in-heap amplification bomb ('x'.repeat(huge)) cannot be bounded
        // without a Boa heap API. This test asserts the CPU-bomb variant IS
        // caught by the loop limit, documenting that the heap-amplification
        // vector is the accepted residual gap.
        use camel_language_api::JsLimitsConfig;
        let limits = JsLimitsConfig {
            max_loop_iterations: Some(1_000),
            ..Default::default()
        };
        let engine = BoaEngine::new(limits);
        // A CPU-bound loop is bounded by the iteration limit.
        let result = engine.eval("let i=0; while(true){i++;}", JsExchange::default());
        assert!(result.is_err(), "CPU-bomb must trip the loop limit");
    }

    #[test]
    fn test_boa_deep_recursion_trips_recursion_limit() {
        use camel_language_api::JsLimitsConfig;
        let limits = JsLimitsConfig {
            max_recursion_depth: Some(10),
            ..Default::default()
        };
        let engine = BoaEngine::new(limits);
        // Recursive fn that immediately recurses (no base case).
        let script = "(function f() { return f(); })()";
        let result = engine.eval(script, JsExchange::default());
        assert!(result.is_err(), "deep recursion must trip recursion_limit");
    }
}
