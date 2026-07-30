//! Public MiniJinja engine module.
//!
//! Extracted from `lib.rs` for reuse by both the inline `Language` SPI and the
//! external `camel-template` Component (ADR-0047 Stage 2). The public
//! [`render`] entry point compiles a `&str` source via the same S5/S7 checks
//! the inline Language applies and renders against a pre-built
//! [`minijinja::Value`] on a blocking thread with a wall-clock timeout.
//!
//! See ADR-0047 §Stage 2 and the `external-template-component` OpenSpec change.

use crate::LimitedWriter;
use crate::autoescape_validator;

use async_trait::async_trait;
use camel_api::{Body, Exchange, Value};
use camel_language_api::{Expression, LanguageError, MinijinjaLimitsConfig};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Process-wide counter of [`MinijinjaExpression::compile`] invocations.
///
/// Bumped exactly once per successful compile. Exposed for the regression
/// test that proves [`Expression::evaluate`] does not re-enter `compile`
/// (compile-once invariant — AC8). Production callers should not rely on
/// the value; it is `#[doc(hidden)]` to keep it out of the public surface.
#[doc(hidden)]
#[cfg(test)]
pub(crate) static COMPILE_INVOCATIONS: AtomicUsize = AtomicUsize::new(0);

/// Per-source compile-counter map keyed by the source's `DefaultHasher` hash.
///
/// Used by the regression test to assert compile-once without interference
/// from concurrent tests' compiles (the test uses a unique source string,
/// so its key is private to the test). Like [`COMPILE_INVOCATIONS`], this is
/// `#[doc(hidden)]` and not part of the supported surface.
#[doc(hidden)]
#[cfg(test)]
pub(crate) static COMPILE_INVOCATIONS_BY_SOURCE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<u64, usize>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Resolved resource limits for a MiniJinja `Environment`.
///
/// Every `Option<_>` from [`MinijinjaLimitsConfig`] is folded to the rust-camel
/// runtime default (per ADR-0011). Values match the spec §4.1 defaults:
///
/// | Field | Default |
/// |---|---|
/// | `max_template_source_size` | 1 MiB |
/// | `max_context_size` | 4 MiB |
/// | `max_output_size` | 4 MiB |
/// | `fuel` | 100,000 |
/// | `max_recursion_depth` | 64 |
/// | `execution_timeout_ms` | 5,000 |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedLimits {
    pub max_template_source_size: usize,
    pub max_context_size: usize,
    pub max_output_size: usize,
    pub fuel: u64,
    pub max_recursion_depth: u32,
    pub execution_timeout_ms: u64,
}

impl Default for ResolvedLimits {
    /// Spec §4.1 defaults.
    fn default() -> Self {
        Self {
            max_template_source_size: 1 << 20, // 1 MiB
            max_context_size: 4 << 20,         // 4 MiB
            max_output_size: 4 << 20,          // 4 MiB
            fuel: 100_000,
            max_recursion_depth: 64,
            execution_timeout_ms: 5_000,
        }
    }
}

impl ResolvedLimits {
    /// Fold `MinijinjaLimitsConfig` (all-`Option<T>`) to concrete values.
    /// Each field independently uses `unwrap_or(default)` — no cute combinator.
    pub fn from_config(cfg: &MinijinjaLimitsConfig) -> Self {
        let d = Self::default();
        Self {
            max_template_source_size: cfg
                .max_template_source_size
                .unwrap_or(d.max_template_source_size),
            max_context_size: cfg.max_context_size.unwrap_or(d.max_context_size),
            max_output_size: cfg.max_output_size.unwrap_or(d.max_output_size),
            fuel: cfg.fuel.unwrap_or(d.fuel),
            max_recursion_depth: cfg.max_recursion_depth.unwrap_or(d.max_recursion_depth),
            execution_timeout_ms: cfg.execution_timeout_ms.unwrap_or(d.execution_timeout_ms),
        }
    }
}

/// A compiled MiniJinja template. Compile-once, evaluate-many.
///
/// `compile` registers the template into a fresh `Environment<'static>` and
/// stores the environment in an `Arc` so evaluations are cheap and the
/// environment can outlive the `compile` call. `compile_count` is bumped
/// exactly once (during `compile`) and never modified by `evaluate` — this
/// is the load-bearing compile-once invariant (AC8).
#[derive(Debug)]
pub struct MinijinjaExpression {
    env: Arc<minijinja::Environment<'static>>,
    template_name: String,
    /// Used at compile time for the S5 source-size check and at render time
    /// for the S6 output-limit and S9 context-limit guards.
    limits: ResolvedLimits,
    /// Compile-once invariant counter (AC8). Initialised to 1 inside `compile`,
    /// never incremented in `evaluate`.
    compile_count: Arc<AtomicUsize>,
}

impl MinijinjaExpression {
    /// Compile a MiniJinja template into a reusable `Expression`.
    ///
    /// Performs three checks in order:
    /// 1. S5 — source size against `max_template_source_size`.
    /// 2. S7 — autoescape wrapper contract (lexical validator).
    /// 3. minijinja parse/compile (syntax + template registration).
    ///
    /// Configures strict undefined behavior, fuel, and recursion limit on the
    /// fresh environment.
    pub fn compile(source: &str, limits: ResolvedLimits) -> Result<Self, LanguageError> {
        // S5 — source size check
        if source.len() > limits.max_template_source_size {
            return Err(LanguageError::ParseError {
                expr: source.to_string(),
                reason: format!(
                    "template source size {} exceeds limit {}",
                    source.len(),
                    limits.max_template_source_size
                ),
            });
        }

        // S7 — autoescape wrapper contract
        autoescape_validator::validate_autoescape_wrapper(source).map_err(|e| {
            LanguageError::ParseError {
                expr: source.to_string(),
                reason: format!("autoescape wrapper validation failed: {e}"),
            }
        })?;

        // Deterministic template name from source hash (kept stable across
        // re-compiles of the same source within a single process).
        let template_name = template_name_for(source);

        // Fresh environment with strict undefined + configured limits.
        let mut env: minijinja::Environment<'static> = minijinja::Environment::new();
        env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
        env.set_fuel(Some(limits.fuel));
        env.set_recursion_limit(limits.max_recursion_depth as usize);

        env.add_template_owned(template_name.clone(), source.to_string())
            .map_err(|e| LanguageError::ParseError {
                expr: source.to_string(),
                reason: e.to_string(),
            })?;

        // Probe counters — see `COMPILE_INVOCATIONS` doc.
        // Compiled out of non-test builds so production `compile` pays
        // ZERO cost for test observability (no atomic bump, no hash, no
        // mutex lock, no HashMap insert).
        #[cfg(test)]
        {
            COMPILE_INVOCATIONS.fetch_add(1, Ordering::Relaxed);
            let key = {
                let mut h = DefaultHasher::new();
                source.hash(&mut h);
                h.finish()
            };
            let mut map = COMPILE_INVOCATIONS_BY_SOURCE
                .lock()
                .expect("compile probe mutex poisoned");
            *map.entry(key).or_insert(0) += 1;
            drop(map);
        }

        Ok(Self {
            env: Arc::new(env),
            template_name,
            limits,
            compile_count: Arc::new(AtomicUsize::new(1)),
        })
    }

    /// Number of times `compile` ran for the given source string (across the
    /// process). Test-only observability. `0` if the source has not been
    /// compiled yet.
    #[doc(hidden)]
    #[cfg(test)]
    pub(crate) fn compile_count_for_source(source: &str) -> usize {
        let key = {
            let mut h = DefaultHasher::new();
            source.hash(&mut h);
            h.finish()
        };
        COMPILE_INVOCATIONS_BY_SOURCE
            .lock()
            .expect("compile probe mutex poisoned")
            .get(&key)
            .copied()
            .unwrap_or(0)
    }

    /// Registered template name (debug/test access).
    pub fn template_name(&self) -> &str {
        &self.template_name
    }

    /// Underlying MiniJinja environment (debug/test access).
    pub fn environment(&self) -> &minijinja::Environment<'static> {
        &self.env
    }

    /// Number of times `compile` ran. Always exactly 1 for a single
    /// `MinijinjaExpression` — `evaluate` never re-compiles.
    pub fn compile_count(&self) -> usize {
        self.compile_count.load(Ordering::Relaxed)
    }
}

/// Deterministic, hash-based template name (avoids user-supplied names in
/// Phase 1 — every inline script is anonymous).
fn template_name_for(source: &str) -> String {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    format!("t{:016x}", hasher.finish())
}

#[async_trait]
impl Expression for MinijinjaExpression {
    async fn evaluate(&self, exchange: &Exchange) -> Result<Value, LanguageError> {
        // S4 (kept verbatim from Task 11)
        if matches!(&exchange.input.body, Body::Stream(_)) {
            return Err(LanguageError::EvalError(
                "minijinja cannot render a Body::Stream directly; add `stream_cache` upstream \
                 (crates/camel-processor/src/stream_cache.rs:37-54)"
                    .to_string(),
            ));
        }

        // S9: bounded context — measure into io::sink() via LimitedWriter, then
        // build the minijinja::Value directly from the source data.
        let ctx = build_context_bounded(exchange, self.limits.max_context_size)?;

        // Spawn synchronous MiniJinja render onto a blocking thread, with
        // a wall-clock timeout enforced via tokio::time::timeout. The closure
        // operates on the pre-compiled `self.env` (no re-compile on evaluate —
        // AC8 compile-once invariant). The pre-compiled env was registered
        // exactly once in `compile` and validated (S5/S7) at that point.
        let env = Arc::clone(&self.env);
        let name = self.template_name.clone();
        let max_output = self.limits.max_output_size as u64;
        let timeout = std::time::Duration::from_millis(self.limits.execution_timeout_ms);
        let join = tokio::task::spawn_blocking(move || -> Result<String, LanguageError> {
            let tmpl = env
                .get_template(&name)
                .map_err(|e| LanguageError::EvalError(format!("template lookup: {e}")))?;
            let mut buf = Vec::new();
            let mut writer = LimitedWriter::new(&mut buf, max_output);
            // render_captured_to is the non-deprecated equivalent of
            // render_to_write (deprecated in minijinja 2.18.0). Both take
            // S: Serialize and W: io::Write by value. The returned Captured
            // is dropped — we keep only the bytes already written to `buf`.
            tmpl.render_captured_to(&ctx, &mut writer)
                .map_err(|e| LanguageError::EvalError(format!("render: {e}")))?;
            String::from_utf8(buf)
                .map_err(|e| LanguageError::EvalError(format!("non-utf8 output: {e}")))
        });
        let rendered = match tokio::time::timeout(timeout, join).await {
            Ok(Ok(inner)) => inner?,
            Ok(Err(join_err)) => {
                return Err(LanguageError::EvalError(format!(
                    "minijinja spawn_blocking join: {join_err}"
                )));
            }
            Err(_) => {
                return Err(LanguageError::EvalError(
                    "minijinja execution timeout".to_string(),
                ));
            }
        };

        Ok(Value::String(rendered))
    }
}

/// S9: bounded context build. Performs a measurement pass via `serde_json` into
/// a `LimitedWriter` (bound = `max_context_size` bytes) before allocating the
/// `minijinja::Value`. The pre-bound check on `Body::Bytes` exists because
/// `String::from_utf8_lossy` would otherwise allocate a full body-sized
/// intermediate buffer — defeating the S9 "no unbounded pre-measurement
/// allocation" rule when bytes alone exceed the limit.
///
/// `pub` so the external `camel-template` Component can reuse the exact
/// same bounded build (ADR-0047 Stage 2 invariant: templates written
/// against the inline language must render identically against the
/// external component, including their security bounds). Duplicating
/// this function is a known regression risk — the Task 4.2 first cut
/// dropped the S9 bound by re-deriving the context shape without it.
pub fn build_context_bounded(
    exchange: &Exchange,
    max_context_size: usize,
) -> Result<minijinja::Value, LanguageError> {
    if let Body::Bytes(b) = &exchange.input.body
        && b.len() > max_context_size
    {
        return Err(LanguageError::EvalError(format!(
            "max-context-size {max_context_size} bytes exceeded (body bytes alone: {})",
            b.len()
        )));
    }

    let body_view = BodyAsJson::from(&exchange.input.body);
    let measurement = serde_json::to_writer(
        LimitedWriter::new(std::io::sink(), max_context_size as u64),
        &MeasurementCtx {
            body: &body_view,
            headers: &exchange.input.headers,
            exchange_property: &exchange.properties,
        },
    );
    measurement.map_err(|_| {
        LanguageError::EvalError(format!(
            "max-context-size {max_context_size} bytes exceeded"
        ))
    })?;

    // Happy path: build the minijinja::Value directly via from_serialize.
    let map: BTreeMap<String, minijinja::Value> = BTreeMap::from([
        (
            "body".to_string(),
            minijinja::Value::from_serialize(&body_view),
        ),
        (
            "headers".to_string(),
            minijinja::Value::from_serialize(&exchange.input.headers),
        ),
        (
            "exchangeProperty".to_string(),
            minijinja::Value::from_serialize(&exchange.properties),
        ),
    ]);
    Ok(minijinja::Value::from_serialize(&map))
}

/// Zero-allocation `&Body` → JSON view. Used for both the S9 measurement
/// pass and the happy-path minijinja::Value build. `Stream` is unreachable
/// because the S4 guard rejects it earlier in `evaluate`. `Bytes` is
/// pre-bounded by the caller (see `build_context_bounded`).
struct BodyAsJson<'a> {
    inner: &'a Body,
}

impl<'a> BodyAsJson<'a> {
    fn from(body: &'a Body) -> Self {
        Self { inner: body }
    }
}

impl<'a> serde::Serialize for BodyAsJson<'a> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.inner {
            Body::Empty => s.serialize_none(),
            Body::Text(t) => s.serialize_str(t),
            Body::Json(v) => v.serialize(s),
            Body::Xml(x) => s.serialize_str(x),
            Body::Bytes(b) => s.serialize_str(&String::from_utf8_lossy(b)),
            Body::Stream(_) => unreachable!("S4 rejects Body::Stream before context build"),
        }
    }
}

#[derive(serde::Serialize)]
struct MeasurementCtx<'a> {
    body: &'a BodyAsJson<'a>,
    headers: &'a HashMap<String, Value>,
    #[serde(rename = "exchangeProperty")]
    exchange_property: &'a HashMap<String, Value>,
}

/// Public MiniJinja render entry point (standalone one-shot helper).
///
/// Compiles `source` via [`MinijinjaExpression::compile`] (applying the same
/// S5 source-size and S7 autoescape-wrapper checks the inline Language uses),
/// then renders against `context` on a blocking thread bounded by
/// `limits.max_output_size` and `limits.execution_timeout_ms`.
///
/// This is the standalone compile+render path kept for reuse by the external
/// `camel-template` Component (Phase 4) and for one-shot entry from any
/// caller that does not need a compile-once [`Expression`] handle. The
/// inline `Language` SPI's [`Expression::evaluate`] does NOT route through
/// this entry point — it renders the pre-compiled `Environment` directly
/// to preserve the compile-once invariant (AC8).
///
/// The returned `String` is the rendered template output; the caller is
/// responsible for any body replacement.
pub async fn render(
    source: &str,
    context: &minijinja::Value,
    limits: ResolvedLimits,
) -> Result<String, LanguageError> {
    let expr = MinijinjaExpression::compile(source, limits)?;
    let env = Arc::clone(&expr.env);
    let name = expr.template_name.clone();
    let max_output = limits.max_output_size as u64;
    let timeout = std::time::Duration::from_millis(limits.execution_timeout_ms);
    // `context` is a borrow whose lifetime is the `render` body's; cloning
    // hands the closure an owned `Value` (cheap — `minijinja::Value` is
    // internally `Arc`-backed, so the clone is a refcount bump for the
    // common variants). The closure must be `'static`, so a borrow cannot
    // be moved into it.
    let context = context.clone();

    let join = tokio::task::spawn_blocking(move || -> Result<String, LanguageError> {
        let tmpl = env
            .get_template(&name)
            .map_err(|e| LanguageError::EvalError(format!("template lookup: {e}")))?;
        let mut buf = Vec::new();
        let mut writer = LimitedWriter::new(&mut buf, max_output);
        // render_captured_to is the non-deprecated equivalent of
        // render_to_write (deprecated in minijinja 2.18.0). Both take
        // S: Serialize and W: io::Write by value. The returned Captured
        // is dropped — we keep only the bytes already written to `buf`.
        tmpl.render_captured_to(&context, &mut writer)
            .map_err(|e| LanguageError::EvalError(format!("render: {e}")))?;
        String::from_utf8(buf)
            .map_err(|e| LanguageError::EvalError(format!("non-utf8 output: {e}")))
    });

    match tokio::time::timeout(timeout, join).await {
        Ok(Ok(inner)) => inner,
        Ok(Err(join_err)) => Err(LanguageError::EvalError(format!(
            "minijinja spawn_blocking join: {join_err}"
        ))),
        Err(_) => Err(LanguageError::EvalError(
            "minijinja execution timeout".to_string(),
        )),
    }
}
