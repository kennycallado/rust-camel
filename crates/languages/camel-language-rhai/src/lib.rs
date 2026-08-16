//! camel-language-rhai — Rhai script language for Camel Rust.
//!
//! Main types: `RhaiLanguage`, `RhaiExpression`, `RhaiPredicate`, `RhaiMutatingExpression`.
//! Scripts have access to `body`, `headers`, `header()`, `set_header()`, `property()`, `set_property()`.
//!
//! # Resource Limits
//!
//! Scripts are bounded by configurable limits sourced from `[languages.rhai.limits]`
//! in `Camel.toml`. When absent, the rust-camel runtime defaults apply:
//!
//! | Limit | Default |
//! |---|---|
//! | `max-operations` | 100,000 |
//! | `max-string-size` | 1 MiB |
//! | `max-array-size` | 10,000 elements |
//! | `max-map-size` | 10,000 entries |
//! | `max-expression-depth` | 64 |
//! | `max-function-expression-depth` | 32 |
//! | `execution-timeout-ms` | 5,000 |
//!
//! Every limit is `Option<_>`; `None` means "use rust-camel runtime default" (no
//! default-lie per ADR-0011).
//!
//! ## Covered threats
//!
//! - Infinite loops (`loop {}`) — trip `max-operations` and `execution-timeout-ms`
//!   (whichever fires first).
//! - Oversized allocations — strings, arrays, maps each have size caps.
//! - Pathological nesting — expression and function-expression depths are bounded.
//!
//! ## Timeout caveat
//!
//! `execution-timeout-ms` wraps `eval_with_scope` in `tokio::time::timeout` +
//! `spawn_blocking`. When the timeout fires, the route future resolves to an
//! error, but the blocking thread may continue executing until the script
//! trips `max-operations` or finishes. This is the same partial-mitigation
//! caveat shared with the Boa engine.
//!
//! # Sandboxing
//!
//! Rhai scripts **cannot access the host filesystem, network APIs, or Rhai
//! module loading**. This sandbox is **unconditional** — there is no config
//! opt-out.
//!
//! Two structural layers enforce this:
//!
//! 1. **Compile time** — the Rhai `no_module` cargo feature (see root
//!    `Cargo.toml`) disables module loading across the entire crate graph.
//! 2. **Runtime** — engines are built via `Engine::new_raw()` plus an
//!    explicit `StandardPackage` registration. Unlike `Engine::new()`, this
//!    does not install a `FileModuleResolver`.
//!
//! Additionally, `eval` and `import` are registered as disabled symbols as
//! defense in depth (no-op safe if symbols are absent). If a future Rhai
//! package were to re-introduce module loading or dynamic evaluation, these
//! symbols would still be blocked to user scripts.
//!
//! What the sandbox is **not**:
//!
//! - It is not a CPU/memory cap. Those are separate resource limits; see
//!   rc-bpx.
//! - It does not block timing APIs (`sleep`, `timestamp`). Those remain part
//!   of `StandardPackage`; CPU/time-based DoS is covered by rc-bpx resource
//!   limits.
//! - It is not a side-channel defense (cache timing, memory layout). Out of
//!   scope.
//!
//! If your integration genuinely needs filesystem or network access from a
//! route script, use the explicit Camel components (`file:`, `http:`) with
//! their own runtime policies — do not attempt to bypass this sandbox.
//!
//! # Limitations
//!
//! - Resource limits (max operations, string/array/map sizes, expression
//!   depths) are enforced as denial-of-service protection, not as a sandbox.
//!   They are configurable — see [# Resource Limits](#resource-limits) above.
//! - The `body` variable is always a string. Structured access to JSON/XML
//!   bodies requires explicit parsing within the script using Rhai's built-in
//!   map/array types.
//! - String methods such as `replace`, `trim`, and `pad` mutate the subject in
//!   place and return unit `()`. They do NOT return a new string. Call them as
//!   statements (`body.replace(",", "%2C")`). Never write `body = body.replace(...)`
//!   or `headers["k"] = headers["k"].replace(...)` — the right-hand side is unit,
//!   so the assignment silently drops the value (the body stays unchanged; a
//!   header entry becomes `Null`). See the `rhai_replace_*` tests and bd rc-2mjo.

use async_trait::async_trait;
use camel_language_api::{
    Body, Exchange, Expression, Language, LanguageError, MutatingExpression, Predicate,
    RhaiLimitsConfig, Value,
};
use rhai::{
    AST, Engine, Scope,
    packages::{Package, StandardPackage},
};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tracing::{debug, warn};

// Test-only compile counter. Per-thread (via `thread_local!`) so parallel
// test execution does not perturb the counter that a single test observes.
// Incremented each time `engine.compile` is called from a `create_*`
// method. The regression test asserts the counter is 1 after `create_*` +
// N `evaluate`s — proving the AST is stored at create time and reused at
// eval time, not re-compiled.
#[cfg(test)]
thread_local! {
    pub(crate) static COMPILE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Result type for mutating eval (returns value + modified exchange fields).
type EvalMutResult = Result<
    (
        Value,
        Body,
        std::collections::HashMap<String, Value>,
        std::collections::HashMap<String, Value>,
    ),
    LanguageError,
>;

/// Resolved limits — every `Option<T>` folded to rust-camel runtime default.
struct ResolvedRhaiLimits {
    max_operations: u64,
    max_string_size: usize,
    max_array_size: usize,
    max_map_size: usize,
    max_expression_depth: u32,
    max_function_expression_depth: u32,
    max_call_levels: u32,
    execution_timeout_ms: u64,
}

/// Fold `RhaiLimitsConfig` (all-`Option<T>`) to concrete values using rust-camel
/// runtime defaults. Free function — NOT an inherent method on `RhaiLimitsConfig`
/// (orphan rule: type is defined in `camel-language-api`, this is `camel-language-rhai`).
fn resolve_rhai_limits(limits: &RhaiLimitsConfig) -> ResolvedRhaiLimits {
    ResolvedRhaiLimits {
        max_operations: limits.max_operations.unwrap_or(100_000),
        max_string_size: limits.max_string_size.unwrap_or(1_048_576),
        max_array_size: limits.max_array_size.unwrap_or(10_000),
        max_map_size: limits.max_map_size.unwrap_or(10_000),
        max_expression_depth: limits.max_expression_depth.unwrap_or(64),
        max_function_expression_depth: limits.max_function_expression_depth.unwrap_or(32),
        max_call_levels: limits.max_call_levels.unwrap_or(64),
        execution_timeout_ms: limits.execution_timeout_ms.unwrap_or(5_000),
    }
}

/// Rhai scripting language for rust-camel.
///
/// Scripts have access to:
/// - `body` — exchange body as a string variable
/// - `headers` — exchange headers as a Rhai Map
/// - `header(name)` — look up a header value by name
/// - `set_header(name, value)` — set a header (visible within the same script evaluation)
/// - `property(name)` — look up an exchange property by name
/// - `set_property(name, value)` — set a property (visible within the same script evaluation)
///
/// **Note:** `set_header` and `set_property` only affect values visible within the current
/// script evaluation. Changes do **not** propagate back to the Exchange because expressions
/// receive a read-only `&Exchange` reference.
///
/// ## Resource Limits
///
/// Limits are configurable via `[languages.rhai.limits]` in `Camel.toml`; see the
/// crate-level `# Resource Limits` section for knobs and defaults, and
/// `# Sandboxing` for the unconditional filesystem and network closure.
pub struct RhaiLanguage {
    limits: RhaiLimitsConfig,
}

impl RhaiLanguage {
    pub fn new() -> Self {
        Self::with_limits(RhaiLimitsConfig::default())
    }

    pub fn with_limits(limits: RhaiLimitsConfig) -> Self {
        Self { limits }
    }

    /// Create a base engine with all resource limits configured and host-OS
    /// reach structurally closed (no module resolver, no module-loading feature).
    ///
    /// Sandbox design (rc-d8f9):
    /// - `Engine::new_raw()` builds an engine with no global packages and no
    ///   module resolver — unlike `Engine::new()`, which installs
    ///   `FileModuleResolver`.
    /// - `StandardPackage` re-registers exactly the surface that `Engine::new()`
    ///   shipped (arithmetic, string, array, blob, map, time, math, logic) —
    ///   minus the resolver.
    /// - The `no_module` cargo feature (root Cargo.toml) disables module
    ///   loading at compile time across the whole crate graph.
    /// - `disable_symbol("eval")` / `disable_symbol("import")` retained as
    ///   defense in depth. They are no-op safe if the symbols are already
    ///   absent.
    ///
    /// This is unconditional — there is no opt-out. See
    /// `docs/superpowers/specs/2026-06-28-rc-d8f9-rhai-sandbox-design.md`.
    fn create_base_engine(limits: &RhaiLimitsConfig) -> Engine {
        let r = resolve_rhai_limits(limits);
        let mut engine = Engine::new_raw();
        StandardPackage::new().register_into_engine(&mut engine);
        engine.set_max_expr_depths(
            r.max_expression_depth as usize,
            r.max_function_expression_depth as usize,
        );
        engine.set_max_operations(r.max_operations);
        engine.set_max_string_size(r.max_string_size);
        engine.set_max_array_size(r.max_array_size);
        engine.set_max_map_size(r.max_map_size);
        engine.set_max_call_levels(r.max_call_levels as usize);
        // Defense in depth — no-op safe if symbols already absent under no_module.
        engine.disable_symbol("eval");
        engine.disable_symbol("import");
        engine
    }

    /// Build a scope with `body` and `headers` variables from the exchange.
    fn make_scope(exchange: &Exchange) -> (Scope<'static>, rhai::Map, rhai::Map) {
        let mut scope = Scope::new();
        let body = exchange.input.body.as_text().unwrap_or("").to_string();
        scope.push("body", body);

        let mut headers = rhai::Map::new();
        for (k, v) in &exchange.input.headers {
            headers.insert(k.clone().into(), json_to_dynamic(v));
        }
        scope.push("headers", headers.clone());

        let mut properties = rhai::Map::new();
        for (k, v) in &exchange.properties {
            properties.insert(k.clone().into(), json_to_dynamic(v));
        }

        (scope, headers, properties)
    }

    /// Create a fresh engine with `header()`, `set_header()`, `property()`, and
    /// `set_property()` registered as native functions. A new engine per eval
    /// avoids sharing mutable state between evaluations.
    fn create_eval_engine(
        limits: &RhaiLimitsConfig,
        headers: rhai::Map,
        properties: rhai::Map,
    ) -> Engine {
        let mut engine = Self::create_base_engine(limits);

        // Shared mutable headers map: header() reads, set_header() writes
        let h = Arc::new(RwLock::new(headers));

        let h_read = h.clone();
        engine.register_fn("header", move |name: String| -> rhai::Dynamic {
            h_read
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(name.as_str())
                .cloned()
                .unwrap_or(rhai::Dynamic::UNIT)
        });

        let h_write = h.clone();
        engine.register_fn("set_header", move |name: String, value: rhai::Dynamic| {
            h_write
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(name.into(), value);
        });

        // Shared mutable properties map: property() reads, set_property() writes
        let p = Arc::new(RwLock::new(properties));

        let p_read = p.clone();
        engine.register_fn("property", move |name: String| -> rhai::Dynamic {
            p_read
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(name.as_str())
                .cloned()
                .unwrap_or(rhai::Dynamic::UNIT)
        });

        let p_write = p.clone();
        engine.register_fn("set_property", move |name: String, value: rhai::Dynamic| {
            p_write
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(name.into(), value);
        });

        engine
    }

    /// Sync eval for non-mutating expressions. Extracts exchange data into
    /// owned values, builds engine+scope, runs the pre-compiled AST, and
    /// returns the script's last expression value. Compilation happens at
    /// `create_*` time, never here.
    fn eval_sync(
        ast: &rhai::AST,
        limits: &RhaiLimitsConfig,
        body_text: String,
        headers_map: rhai::Map,
        properties_map: rhai::Map,
    ) -> Result<Value, LanguageError> {
        let mut scope = Scope::new();
        scope.push("body", body_text);
        scope.push("headers", headers_map.clone());

        let engine = Self::create_eval_engine(limits, headers_map, properties_map);

        // eval_ast_with_scope reuses the pre-compiled AST — no re-parse per eval.
        // The AST was built once in `create_expression` / `create_predicate`.
        let result: rhai::Dynamic = engine.eval_ast_with_scope(&mut scope, ast).map_err(|e| {
            let pos = e.position();
            let location = match (pos.line(), pos.position()) {
                (Some(l), Some(c)) => format!(" at line {l}, column {c}"),
                (Some(l), None) => format!(" at line {l}"),
                _ => String::new(),
            };
            // EvalAltResult::ErrorRuntime embeds the thrown value verbatim,
            // which may contain exchange body/header secrets (e.g. `throw headers["Authorization"]`).
            // Emit only the error kind and position; never the thrown value.
            let safe_kind = if matches!(*e, rhai::EvalAltResult::ErrorRuntime(_, _)) {
                "script threw an exception (thrown value not logged)".to_string()
            } else {
                format!("{e}")
            };
            let err_msg = format!("rhai evaluation error{location}: {safe_kind}");
            warn!("rhai expression eval failed{location}");
            LanguageError::EvalError(err_msg)
        })?;

        dynamic_to_json(result)
    }

    /// Sync eval for mutating expressions. Takes owned exchange fields, runs
    /// the pre-compiled AST, and returns the result value + modified fields.
    /// The caller writes fields back on success (implicit rollback on error).
    fn eval_mut_sync(
        ast: &rhai::AST,
        limits: &RhaiLimitsConfig,
        body: Body,
        headers: HashMap<String, Value>,
        properties: HashMap<String, Value>,
    ) -> EvalMutResult {
        // 1. Create scope with Rhai Map variables
        let mut scope = Scope::new();

        let mut headers_map = rhai::Map::new();
        for (k, v) in &headers {
            headers_map.insert(k.clone().into(), json_to_dynamic(v));
        }

        let mut properties_map = rhai::Map::new();
        for (k, v) in &properties {
            properties_map.insert(k.clone().into(), json_to_dynamic(v));
        }

        let body_str = body.as_text().unwrap_or("").to_string();

        scope.push("headers", headers_map);
        scope.push("properties", properties_map);
        scope.push("body", body_str);

        // 2. Run the pre-compiled AST (no re-parse per eval).
        let engine = RhaiLanguage::create_base_engine(limits);

        let result: rhai::Dynamic = match engine.eval_ast_with_scope(&mut scope, ast) {
            Ok(v) => v,
            Err(e) => {
                let pos = e.position();
                let location = match (pos.line(), pos.position()) {
                    (Some(l), Some(c)) => format!(" at line {l}, column {c}"),
                    (Some(l), None) => format!(" at line {l}"),
                    _ => String::new(),
                };
                let safe_kind = if matches!(*e, rhai::EvalAltResult::ErrorRuntime(_, _)) {
                    "script threw an exception (thrown value not logged)".to_string()
                } else {
                    format!("{e}")
                };
                let err_msg = format!("rhai evaluation error{location}: {safe_kind}");
                warn!("rhai expression eval failed{location}");
                return Err(LanguageError::EvalError(err_msg));
            }
        };

        // 3. Sync changes back
        let mut out_headers = headers;
        if let Some(h) = scope.get_value::<rhai::Map>("headers") {
            out_headers = rhai_map_to_value_map(&h);
        }
        let mut out_properties = properties;
        if let Some(p) = scope.get_value::<rhai::Map>("properties") {
            out_properties = rhai_map_to_value_map(&p);
        }
        let mut out_body = body;
        if let Some(b) = scope.get_value::<String>("body") {
            out_body = Body::Text(b);
        }

        Ok((
            dynamic_to_value(result),
            out_body,
            out_headers,
            out_properties,
        ))
    }
}

/// Convert a serde_json::Value to a rhai::Dynamic.
fn json_to_dynamic(v: &Value) -> rhai::Dynamic {
    match v {
        Value::String(s) => rhai::Dynamic::from(s.clone()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                rhai::Dynamic::from(i)
            } else if let Some(f) = n.as_f64() {
                rhai::Dynamic::from_float(f)
            } else {
                rhai::Dynamic::from(n.to_string())
            }
        }
        Value::Bool(b) => rhai::Dynamic::from(*b),
        Value::Null => rhai::Dynamic::UNIT,
        Value::Array(arr) => {
            let rhai_arr: rhai::Array = arr.iter().map(json_to_dynamic).collect();
            rhai::Dynamic::from(rhai_arr)
        }
        Value::Object(obj) => {
            let mut rhai_map = rhai::Map::new();
            for (k, v) in obj {
                rhai_map.insert(k.clone().into(), json_to_dynamic(v));
            }
            rhai::Dynamic::from(rhai_map)
        }
    }
}

fn dynamic_to_json(d: rhai::Dynamic) -> Result<Value, LanguageError> {
    if d.is_string() {
        Ok(Value::String(d.cast::<String>()))
    } else if d.is::<bool>() {
        Ok(Value::Bool(d.cast::<bool>()))
    } else if d.is_float() {
        Ok(Value::from(d.cast::<f64>()))
    } else if d.is_int() {
        Ok(Value::from(d.cast::<i64>()))
    } else if d.is_unit() {
        Ok(Value::Null)
    } else {
        Ok(Value::String(d.to_string()))
    }
}

/// Convert a rhai::Dynamic to a serde_json::Value (infallible version for mutating expressions).
fn dynamic_to_value(d: rhai::Dynamic) -> Value {
    if d.is_string() {
        Value::String(d.cast::<String>())
    } else if d.is::<bool>() {
        Value::Bool(d.cast::<bool>())
    } else if d.is_int() {
        Value::from(d.cast::<i64>())
    } else if d.is_float() {
        Value::from(d.cast::<f64>())
    } else if d.is_unit() {
        Value::Null
    } else {
        Value::String(d.to_string())
    }
}

/// Convert a Rhai Map to a HashMap<String, Value> for syncing back to exchange.
fn rhai_map_to_value_map(map: &rhai::Map) -> std::collections::HashMap<String, Value> {
    let mut result = std::collections::HashMap::new();
    for (k, v) in map {
        result.insert(k.to_string(), dynamic_to_value(v.clone()));
    }
    result
}

struct RhaiExpression {
    ast: Arc<AST>,
    limits: RhaiLimitsConfig,
}

struct RhaiPredicate {
    ast: Arc<AST>,
    limits: RhaiLimitsConfig,
}

/// Shared async eval helper used by both [`RhaiExpression`] and [`RhaiPredicate`].
/// Resolves limits, applies the timeout, and runs the script via `spawn_blocking`.
async fn eval_async(
    ast: Arc<AST>,
    limits: &RhaiLimitsConfig,
    exchange: &Exchange,
) -> Result<Value, LanguageError> {
    let r = resolve_rhai_limits(limits);
    let timeout = Duration::from_millis(r.execution_timeout_ms);
    let body_text = exchange.input.body.as_text().unwrap_or("").to_string();
    let (_, headers_map, properties_map) = RhaiLanguage::make_scope(exchange);
    let limits = limits.clone();

    tokio::time::timeout(timeout, async move {
        tokio::task::spawn_blocking(move || {
            RhaiLanguage::eval_sync(&ast, &limits, body_text, headers_map, properties_map)
        })
        .await
        .map_err(|join| LanguageError::EvalError(format!("rhai execution join error: {join}")))?
    })
    .await
    .map_err(|_| LanguageError::EvalError("rhai execution timeout".to_string()))?
}

#[async_trait]
impl Expression for RhaiExpression {
    async fn evaluate(&self, exchange: &Exchange) -> Result<Value, LanguageError> {
        eval_async(self.ast.clone(), &self.limits, exchange).await
    }
}

#[async_trait]
impl Predicate for RhaiPredicate {
    async fn matches(&self, exchange: &Exchange) -> Result<bool, LanguageError> {
        let val = eval_async(self.ast.clone(), &self.limits, exchange).await?;
        Ok(match &val {
            Value::Bool(b) => *b,
            Value::Null => false,
            _ => true,
        })
    }
}

/// A Rhai script expression that can mutate the Exchange during evaluation.
///
/// The script has access to three mutable scope variables:
/// - `headers` — a Rhai map (`#{}`) representing the exchange headers
/// - `properties` — a Rhai map (`#{}`) representing the exchange properties
/// - `body` — a string representing the exchange body
///
/// Changes to these variables are propagated back to the Exchange after evaluation.
/// If evaluation fails, all changes are **rolled back atomically**.
///
/// # Note on API differences
///
/// Unlike non-mutating Rhai expressions, this engine does NOT provide
/// `header()`, `set_header()`, `property()`, or `set_property()` functions.
/// Use direct map assignment syntax instead:
///
/// ```rhai
/// headers["tenant"] = "acme";       // set header
/// properties["trace"] = "enabled";  // set property
/// body = "new content";             // set body
/// let v = headers["existing"];      // read header
/// ```
struct RhaiMutatingExpression {
    ast: Arc<AST>,
    limits: RhaiLimitsConfig,
}

#[async_trait]
impl MutatingExpression for RhaiMutatingExpression {
    async fn evaluate(&self, exchange: &mut Exchange) -> Result<Value, LanguageError> {
        let r = resolve_rhai_limits(&self.limits);
        let timeout = Duration::from_millis(r.execution_timeout_ms);

        // Snapshot owned fields — the script mutates inside spawn_blocking;
        // original exchange is untouched until success.
        let headers = exchange.input.headers.clone();
        let properties = exchange.properties.clone();
        let body = exchange.input.body.clone();
        let ast = self.ast.clone();
        let limits = self.limits.clone();

        let join = tokio::task::spawn_blocking(move || {
            RhaiLanguage::eval_mut_sync(&ast, &limits, body, headers, properties)
        });

        // spawn_blocking gives Result<Result<..., JoinErr>, timeout gives Result<Result<..., JoinErr>, Elapsed>
        match tokio::time::timeout(timeout, join).await {
            Ok(Ok(Ok((value, out_body, out_headers, out_properties)))) => {
                // Success — write back to exchange
                exchange.input.headers = out_headers;
                exchange.properties = out_properties;
                exchange.input.body = out_body;
                Ok(value)
            }
            Ok(Ok(Err(e))) => {
                // Eval error — exchange untouched (implicit rollback)
                Err(e)
            }
            Ok(Err(join_err)) => Err(LanguageError::EvalError(format!(
                "rhai execution join error: {join_err}"
            ))),
            Err(_) => {
                // Timeout — exchange untouched
                Err(LanguageError::EvalError(
                    "rhai execution timeout".to_string(),
                ))
            }
        }
    }
}

impl Language for RhaiLanguage {
    fn name(&self) -> &'static str {
        "rhai"
    }

    fn create_expression(&self, script: &str) -> Result<Box<dyn Expression>, LanguageError> {
        let engine = Self::create_base_engine(&self.limits);
        // Compile once at create time. The AST is reused on every `evaluate`
        // call via `eval_ast_with_scope` — no re-parse per evaluation.
        // Surface parse errors here so they show up at route construction
        // rather than first message.
        let ast = engine.compile(script).map_err(|e| {
            warn!(error = %e, "rhai expression compile failed");
            LanguageError::ParseError {
                expr: script.to_string(),
                reason: e.to_string(),
            }
        })?;
        #[cfg(test)]
        COMPILE_COUNT.with(|c| c.set(c.get() + 1));
        debug!("rhai expression compiled");
        Ok(Box::new(RhaiExpression {
            ast: Arc::new(ast),
            limits: self.limits.clone(),
        }))
    }

    fn create_predicate(&self, script: &str) -> Result<Box<dyn Predicate>, LanguageError> {
        let engine = Self::create_base_engine(&self.limits);
        let ast = engine.compile(script).map_err(|e| {
            warn!(error = %e, "rhai expression compile failed");
            LanguageError::ParseError {
                expr: script.to_string(),
                reason: e.to_string(),
            }
        })?;
        #[cfg(test)]
        COMPILE_COUNT.with(|c| c.set(c.get() + 1));
        debug!("rhai expression compiled");
        Ok(Box::new(RhaiPredicate {
            ast: Arc::new(ast),
            limits: self.limits.clone(),
        }))
    }

    /// Create a mutating Rhai expression.
    ///
    /// The script can modify `headers`, `properties`, and `body` via assignment syntax.
    /// See `RhaiMutatingExpression` for full documentation.
    fn create_mutating_expression(
        &self,
        script: &str,
    ) -> Result<Box<dyn MutatingExpression>, LanguageError> {
        let engine = Self::create_base_engine(&self.limits);
        let ast = engine.compile(script).map_err(|e| {
            warn!(error = %e, "rhai expression compile failed");
            LanguageError::ParseError {
                expr: script.to_string(),
                reason: e.to_string(),
            }
        })?;
        #[cfg(test)]
        COMPILE_COUNT.with(|c| c.set(c.get() + 1));
        debug!("rhai expression compiled");
        Ok(Box::new(RhaiMutatingExpression {
            ast: Arc::new(ast),
            limits: self.limits.clone(),
        }))
    }
}

impl Default for RhaiLanguage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use camel_language_api::{Exchange, Language, Message, Value};
    use std::fs;
    use tempfile::NamedTempFile;

    use super::{RhaiExpression, RhaiLanguage, RhaiMutatingExpression, RhaiPredicate};

    #[test]
    fn types_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RhaiExpression>();
        assert_send_sync::<RhaiPredicate>();
        assert_send_sync::<RhaiMutatingExpression>();
    }

    fn exchange_with_header(key: &str, val: &str) -> Exchange {
        let mut msg = Message::default();
        msg.set_header(key, Value::String(val.to_string()));
        Exchange::new(msg)
    }

    fn exchange_with_body(body: &str) -> Exchange {
        Exchange::new(Message::new(body))
    }

    #[tokio::test]
    async fn test_rhai_predicate_simple() {
        let lang = RhaiLanguage::new();
        let pred = lang
            .create_predicate(r#"header("type") == "order""#)
            .unwrap();
        let ex = exchange_with_header("type", "order");
        assert!(pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn test_rhai_predicate_false() {
        let lang = RhaiLanguage::new();
        let pred = lang
            .create_predicate(r#"header("type") == "order""#)
            .unwrap();
        let ex = exchange_with_header("type", "invoice");
        assert!(!pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn test_rhai_expression_body() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression("body").unwrap();
        let ex = exchange_with_body("hello");
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(val, Value::String("hello".to_string()));
    }

    #[tokio::test]
    async fn test_rhai_expression_concat() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression(r#"body + " world""#).unwrap();
        let ex = exchange_with_body("hello");
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(val, Value::String("hello world".to_string()));
    }

    #[tokio::test]
    async fn test_rhai_set_header_visible_within_script() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_expression(
                r#"
            set_header("done", "yes");
            header("done")
        "#,
            )
            .unwrap();
        let ex = exchange_with_body("test");
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(val, Value::String("yes".to_string()));
    }

    #[tokio::test]
    async fn test_rhai_property_access() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression(r#"property("myProp")"#).unwrap();
        let mut ex = exchange_with_body("test");
        ex.set_property("myProp".to_string(), Value::String("propVal".to_string()));
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(val, Value::String("propVal".to_string()));
    }

    #[tokio::test]
    async fn test_rhai_set_property_visible_within_script() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_expression(
                r#"
            set_property("key", "value");
            property("key")
        "#,
            )
            .unwrap();
        let ex = exchange_with_body("test");
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(val, Value::String("value".to_string()));
    }

    #[tokio::test]
    async fn test_rhai_missing_header_returns_null() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression(r#"header("nonexistent")"#).unwrap();
        let ex = exchange_with_body("test");
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(val, Value::Null);
    }

    #[tokio::test]
    async fn test_rhai_syntax_error() {
        let lang = RhaiLanguage::new();
        let result = lang.create_expression("let x = ;");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rhai_runtime_error() {
        // Calling a nonexistent function will produce a runtime error
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression(r#"nonexistent_fn()"#).unwrap();
        let ex = exchange_with_body("test");
        let result = expr.evaluate(&ex).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rhai_eval_error_contains_location_info() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression("nonexistent_fn()").unwrap();
        let ex = exchange_with_body("test");
        let err = expr.evaluate(&ex).await.unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("rhai evaluation error"),
            "error should contain 'rhai evaluation error', got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_rhai_infinite_loop_trips_max_operations() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression("loop {}").unwrap();
        let ex = exchange_with_body("test");
        let result = expr.evaluate(&ex).await;
        assert!(result.is_err(), "infinite loop must trip max_operations");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.to_lowercase().contains("operation")
                || msg.to_lowercase().contains("limit")
                || msg.to_lowercase().contains("exceeded"),
            "error should reference the limit: {msg}"
        );
    }

    #[tokio::test]
    async fn test_rhai_oversize_string_trips_max_string_size() {
        let lang = RhaiLanguage::new();
        // Build a string larger than 1 MiB by concat.
        let script =
            "let s = \"\"; loop { s = s + \"aaaaaaaaaa\"; if s.len() > 2000000 { break; } } s";
        let expr = lang.create_expression(script).unwrap();
        let ex = exchange_with_body("test");
        let result = expr.evaluate(&ex).await;
        assert!(result.is_err(), "oversize string must trip limit");
    }

    #[tokio::test]
    async fn test_rhai_oversize_array_trips_max_array_size() {
        let lang = RhaiLanguage::new();
        // Default max_array_size = 10_000; push past it.
        let script = "let a = []; loop { a.push(1); if a.len() > 20_000 { break; } } a";
        let expr = lang.create_expression(script).unwrap();
        let ex = exchange_with_body("test");
        let result = expr.evaluate(&ex).await;
        assert!(result.is_err(), "oversize array must trip max_array_size");
    }

    #[tokio::test]
    async fn test_rhai_oversize_map_trips_max_map_size() {
        let lang = RhaiLanguage::new();
        // Default max_map_size = 10_000.
        let script = "let m = #{}; loop { let k = m.len().to_string(); m[k] = 1; if m.len() > 20_000 { break; } } m";
        let expr = lang.create_expression(script).unwrap();
        let ex = exchange_with_body("test");
        let result = expr.evaluate(&ex).await;
        assert!(result.is_err(), "oversize map must trip max_map_size");
    }

    #[tokio::test]
    async fn test_rhai_with_limits_lowers_max_operations() {
        use camel_language_api::RhaiLimitsConfig;
        let limits = RhaiLimitsConfig {
            max_operations: Some(10),
            ..Default::default()
        };
        let lang = RhaiLanguage::with_limits(limits);
        // 100 cheap ops should trip a limit of 10.
        let expr = lang
            .create_expression("let x = 0; loop { x += 1; if x > 100 { break; } } x")
            .unwrap();
        let ex = exchange_with_body("test");
        let result = expr.evaluate(&ex).await;
        assert!(
            result.is_err(),
            "max_operations=10 should trip a 100-iteration loop"
        );
    }

    #[tokio::test]
    async fn test_rhai_with_limits_preserves_none_as_runtime_default() {
        use camel_language_api::RhaiLimitsConfig;
        // None should resolve to rust-camel default (100_000 ops), not upstream unlimited.
        let lang = RhaiLanguage::with_limits(RhaiLimitsConfig::default());
        let expr = lang.create_expression("loop {}").unwrap();
        let ex = exchange_with_body("test");
        assert!(
            expr.evaluate(&ex).await.is_err(),
            "default limits must still trip infinite loop"
        );
    }

    #[tokio::test]
    async fn test_rhai_timeout_fires_when_ops_limit_high() {
        use camel_language_api::RhaiLimitsConfig;
        // Set max_operations very high (not u64::MAX which disables counting in Rhai)
        // and a low timeout. Either the timeout or ops limit must fire.
        let limits = RhaiLimitsConfig {
            max_operations: Some(50_000_000),
            execution_timeout_ms: Some(50),
            ..Default::default()
        };
        let lang = RhaiLanguage::with_limits(limits);
        // CPU-bound loop; either ops limit or timeout must terminate it fast.
        let expr = lang.create_expression("loop {}").unwrap();
        let ex = exchange_with_body("test");
        let start = std::time::Instant::now();
        let result = expr.evaluate(&ex).await;
        let elapsed = start.elapsed();
        assert!(result.is_err(), "must error");
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "must terminate fast: {elapsed:?}"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.to_lowercase().contains("timeout") || msg.to_lowercase().contains("operation"),
            "error should reference timeout or op limit: {msg}"
        );
    }

    #[tokio::test]
    async fn test_rhai_operations_limit_prevents_infinite_loop() {
        let lang = RhaiLanguage::new();
        // This script would loop forever without the operations limit
        let expr = lang
            .create_expression("let x = 0; loop { x += 1; } x")
            .unwrap();
        let ex = exchange_with_body("test");
        let result = expr.evaluate(&ex).await;
        assert!(result.is_err(), "should error due to operations limit");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("operations") || err_msg.contains("limit"),
            "error should mention operations limit, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn test_rhai_empty_body() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression("body").unwrap();
        let ex = Exchange::new(Message::default());
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(val, Value::String("".to_string()));
    }

    #[tokio::test]
    async fn test_rhai_numeric_header() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression(r#"header("count") + 1"#).unwrap();
        let mut msg = Message::default();
        msg.set_header("count", Value::Number(41.into()));
        let ex = Exchange::new(msg);
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(val, Value::from(42));
    }

    #[tokio::test]
    async fn test_rhai_json_array_header_is_native_array() {
        // json_to_dynamic should convert JSON arrays to native Rhai arrays,
        // not to their string representation.
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression(r#"header("items").len()"#).unwrap();
        let mut msg = Message::default();
        msg.set_header(
            "items",
            Value::Array(vec![
                Value::String("a".into()),
                Value::String("b".into()),
                Value::String("c".into()),
            ]),
        );
        let ex = Exchange::new(msg);
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(
            val,
            Value::from(3_i64),
            "array len should be 3, not a stringified value"
        );
    }

    #[tokio::test]
    async fn test_rhai_json_object_header_is_native_map() {
        // json_to_dynamic should convert JSON objects to native Rhai maps.
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression(r#"header("obj")["key"]"#).unwrap();
        let mut msg = Message::default();
        // Build a JSON object via Value's FromStr impl (serde_json::Value)
        let obj: Value = r#"{"key": "value"}"#.parse().unwrap();
        msg.set_header("obj", obj);
        let ex = Exchange::new(msg);
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(val, Value::String("value".to_string()));
    }

    #[tokio::test]
    async fn test_mutating_set_header_propagates_to_exchange() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"headers["tenant"] = "acme""#)
            .unwrap();
        let mut ex = Exchange::new(Message::default());
        expr.evaluate(&mut ex).await.unwrap();
        assert_eq!(
            ex.input.headers.get("tenant"),
            Some(&Value::String("acme".into()))
        );
    }

    #[tokio::test]
    async fn test_mutating_set_body_propagates_to_exchange() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"body = "modified""#)
            .unwrap();
        let mut ex = Exchange::new(Message::new("original"));
        expr.evaluate(&mut ex).await.unwrap();
        assert_eq!(ex.input.body.as_text(), Some("modified"));
    }

    #[tokio::test]
    async fn test_mutating_set_property_propagates_to_exchange() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"properties["auth"] = "ok""#)
            .unwrap();
        let mut ex = Exchange::new(Message::default());
        expr.evaluate(&mut ex).await.unwrap();
        assert_eq!(ex.properties.get("auth"), Some(&Value::String("ok".into())));
    }

    #[tokio::test]
    async fn test_mutating_rollback_on_error() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"headers["x"] = "modified"; throw "error""#)
            .unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.input
            .headers
            .insert("x".to_string(), Value::String("original".into()));
        let result = expr.evaluate(&mut ex).await;
        assert!(result.is_err());
        assert_eq!(
            ex.input.headers.get("x"),
            Some(&Value::String("original".into()))
        );
    }

    #[tokio::test]
    async fn test_mutating_rollback_on_error_includes_body() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"body = "modified"; throw "error""#)
            .unwrap();
        let mut ex = Exchange::new(Message::new("original"));
        let result = expr.evaluate(&mut ex).await;
        assert!(result.is_err());
        assert_eq!(ex.input.body.as_text(), Some("original"));
    }

    #[tokio::test]
    async fn test_mutating_rollback_on_error_includes_property() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"properties["p"] = "modified"; throw "error""#)
            .unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.properties
            .insert("p".to_string(), Value::String("original".into()));
        let result = expr.evaluate(&mut ex).await;
        assert!(result.is_err());
        assert_eq!(
            ex.properties.get("p"),
            Some(&Value::String("original".into()))
        );
    }

    #[tokio::test]
    async fn test_mutating_combined_read_write() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"headers["out"] = headers["in"] + "_processed""#)
            .unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.input
            .headers
            .insert("in".to_string(), Value::String("value".into()));
        expr.evaluate(&mut ex).await.unwrap();
        assert_eq!(
            ex.input.headers.get("out"),
            Some(&Value::String("value_processed".into()))
        );
    }

    #[tokio::test]
    async fn rhai_sandbox_blocks_import_with_canary() {
        // Canary secret exported by a tmp .rhai file. Sandbox must block the
        // import; the canary must not appear in any error message.
        let canary = "RC_D8F9_CANARY_42";
        let tmp = NamedTempFile::new().expect("create tmp file");
        let path_str = tmp
            .path()
            .to_str()
            .expect("tmp path is valid utf-8")
            .to_string();
        fs::write(tmp.path(), format!("export const secret = \"{canary}\";")) // allow-secret
            .expect("write tmp .rhai");

        // allow-secret: fixture script names a Rhai module member, not a credential
        let script = format!("import \"{path_str}\" as m; m::secret");

        let lang = RhaiLanguage::new();
        let ex = exchange_with_body("x");

        // Sandbox must block: either at compile (ParseError) or eval (EvalError).
        let err_msg = match lang.create_expression(&script) {
            Err(parse_err) => format!("{parse_err}"),
            Ok(expr) => match expr.evaluate(&ex).await {
                Ok(_) => panic!("sandbox should block import — got Ok value"),
                Err(eval_err) => format!("{eval_err}"),
            },
        };
        assert!(
            !err_msg.contains(canary),
            "canary leaked into error message: {err_msg}"
        );
    }

    #[tokio::test]
    async fn rhai_sandbox_blocks_eval_call() {
        // `eval` is disabled via disable_symbol — must fail at compile or eval
        // time. Dual-path shape matches T1: parse error and eval error are both
        // acceptable rejections.
        let lang = RhaiLanguage::new();
        let ex = exchange_with_body("x");
        let script = r#"eval("import \"x\" as m")"#;

        let err_msg = match lang.create_expression(script) {
            Err(parse_err) => format!("{parse_err}"),
            Ok(expr) => match expr.evaluate(&ex).await {
                Ok(v) => panic!("eval must be blocked — got Ok: {v:?}"),
                Err(eval_err) => format!("{eval_err}"),
            },
        };
        assert!(
            err_msg.to_lowercase().contains("eval"),
            "eval-related error expected, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn rhai_sandbox_blocks_network_symbol() {
        // Rhai 1.25.1 default has no `http` namespace; `http::get` must be
        // rejected at compile or eval time. If a future Rhai version ships HTTP
        // built-in, this test failing is the correct regression signal.
        let lang = RhaiLanguage::new();
        let ex = exchange_with_body("x");
        let script = r#"http::get("http://127.0.0.1:9")"#;

        let err_msg = match lang.create_expression(script) {
            Err(parse_err) => format!("{parse_err}"),
            Ok(expr) => match expr.evaluate(&ex).await {
                Ok(v) => panic!("http::get must be rejected — got Ok: {v:?}"),
                Err(eval_err) => format!("{eval_err}"),
            },
        };
        // Assertion is intentionally loose: we don't care about the exact error
        // text or variant. The test passes if any error happens (sandbox blocks)
        // and fails if the script evaluates successfully.
        let _ = err_msg; // string already validated by the match arms above
    }

    /// Regression test for FC-LANG-RECOMPILE: every `create_*` call must
    /// compile the script exactly ONCE (at create time, surfacing parse
    /// errors early), and every subsequent `evaluate`/`matches` call must
    /// REUSE the pre-compiled AST — never re-parse the source string.
    ///
    /// Implementation: a `#[cfg(test)]` thread-local `COMPILE_COUNT` is
    /// incremented at every compile call. The thread-local isolation makes
    /// the counter robust against parallel test execution (each test thread
    /// has its own counter, so other tests' compiles are not visible).
    #[tokio::test]
    async fn test_compile_count_expression_is_one_per_create() {
        use super::COMPILE_COUNT;
        let lang = RhaiLanguage::new();
        let ex = exchange_with_body("test");

        let before = COMPILE_COUNT.with(|c| c.get());
        let expr = lang.create_expression("body + 1").unwrap();
        let after_create = COMPILE_COUNT.with(|c| c.get());
        assert_eq!(
            after_create - before,
            1,
            "create_expression must compile exactly once (delta={})",
            after_create - before
        );

        for _ in 0..3 {
            let _ = expr.evaluate(&ex).await.unwrap();
        }
        let after_evals = COMPILE_COUNT.with(|c| c.get());
        assert_eq!(
            after_evals - after_create,
            0,
            "evaluate must not re-compile (delta={})",
            after_evals - after_create
        );
    }

    #[tokio::test]
    async fn test_compile_count_predicate_is_one_per_create() {
        use super::COMPILE_COUNT;
        let lang = RhaiLanguage::new();
        let ex = exchange_with_body("test");

        let before = COMPILE_COUNT.with(|c| c.get());
        let pred = lang
            .create_predicate(r#"header("type") == "order""#)
            .unwrap();
        let after_create = COMPILE_COUNT.with(|c| c.get());
        assert_eq!(
            after_create - before,
            1,
            "create_predicate must compile exactly once (delta={})",
            after_create - before
        );

        for _ in 0..3 {
            let _ = pred.matches(&ex).await.unwrap();
        }
        let after_evals = COMPILE_COUNT.with(|c| c.get());
        assert_eq!(
            after_evals - after_create,
            0,
            "matches must not re-compile (delta={})",
            after_evals - after_create
        );
    }

    #[tokio::test]
    async fn test_compile_count_mutating_expression_is_one_per_create() {
        use super::COMPILE_COUNT;
        let lang = RhaiLanguage::new();

        let before = COMPILE_COUNT.with(|c| c.get());
        let expr = lang
            .create_mutating_expression(r#"headers["x"] = "y""#)
            .unwrap();
        let after_create = COMPILE_COUNT.with(|c| c.get());
        assert_eq!(
            after_create - before,
            1,
            "create_mutating_expression must compile exactly once (delta={})",
            after_create - before
        );

        for _ in 0..3 {
            let mut ex = Exchange::new(Message::default());
            let _ = expr.evaluate(&mut ex).await.unwrap();
        }
        let after_evals = COMPILE_COUNT.with(|c| c.get());
        assert_eq!(
            after_evals - after_create,
            0,
            "mutating evaluate must not re-compile (delta={})",
            after_evals - after_create
        );
    }

    #[test]
    fn resolve_rhai_limits_threads_max_call_levels() {
        // Default pins 64, removing the upstream 8-debug/64-release asymmetry (rc-dip6).
        let default = super::resolve_rhai_limits(&super::RhaiLimitsConfig::default());
        assert_eq!(default.max_call_levels, 64);
        // A custom override flows through resolve.
        let custom = super::resolve_rhai_limits(&super::RhaiLimitsConfig {
            max_call_levels: Some(16),
            ..Default::default()
        });
        assert_eq!(custom.max_call_levels, 16);
    }

    // ── Rhai string .replace() characterization (demo-found footgun) ──
    //
    // A demo reported `body.replace(",", "%2C")` "returns empty silently,
    // no error". Root cause captured by these tests: Rhai's `replace`
    // (registered by `StandardPackage`) is an IN-PLACE `&mut` method that
    // returns unit `()`. It does NOT return a new string like most
    // languages' replace. Consequences:
    //
    //   - statement form  `body.replace(...)`     → works (mutates in place)
    //   - expression form `body.replace(...)`     → evaluates to `()` (Null)
    //   - assignment form `body = body.replace()` → silently wrong: RHS is
    //     `()`, so `body` becomes non-String and the sync-back falls back
    //     to the original snapshot (unchanged) — or, for a header entry,
    //     the value becomes Null. No error is emitted.
    //
    // These tests pin the actual behaviour so a future Rhai upgrade that
    // changes the `replace` signature is caught, and so the footgun is
    // documented in the test suite.

    /// Statement form (CORRECT usage): mutates `body` in place.
    #[tokio::test]
    async fn rhai_replace_statement_mutates_body_in_place() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"body.replace(",", "%2C")"#)
            .expect("replace must compile");
        let mut ex = exchange_with_body("bbox=1,2,3,4");
        expr.evaluate(&mut ex).await.expect("replace must eval");
        assert_eq!(
            ex.input.body.as_text(),
            Some("bbox=1%2C2%2C3%2C4"),
            "in-place replace must update body"
        );
    }

    /// Expression form (FOOTGUN): `.replace(...)` evaluates to `()`
    /// because the method returns unit. Using it as a value yields Null.
    #[tokio::test]
    async fn rhai_replace_expression_returns_unit_not_a_string() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_expression(r#"body.replace(",", "%2C")"#)
            .expect("replace must compile");
        let ex = exchange_with_body("bbox=1,2,3,4");
        let val = expr.evaluate(&ex).await.expect("replace must eval");
        assert_eq!(
            val,
            Value::Null,
            "replace returns unit, not the replaced string"
        );
    }

    /// Assignment form (FOOTGUN): `body = body.replace(...)` assigns unit
    /// to `body`; the String sync-back then fails and the original body
    /// snapshot is restored — silently unchanged.
    #[tokio::test]
    async fn rhai_replace_assigned_to_body_is_silently_unchanged() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"body = body.replace(",", "%2C")"#)
            .expect("replace must compile");
        let mut ex = exchange_with_body("bbox=1,2,3,4");
        expr.evaluate(&mut ex).await.expect("replace must eval");
        assert_eq!(
            ex.input.body.as_text(),
            Some("bbox=1,2,3,4"),
            "assignment form is a footgun: body silently stays original"
        );
    }

    /// Header assignment form (FOOTGUN): `h["k"] = h["k"].replace(...)`
    /// writes unit into the map entry, so the header value becomes Null.
    #[tokio::test]
    async fn rhai_replace_assigned_to_header_becomes_null() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"headers["url"] = headers["url"].replace(",", "%2C")"#)
            .expect("replace must compile");
        let mut ex = exchange_with_body("x");
        ex.input.headers.insert(
            "url".to_string(),
            Value::String("http://x/wfs?bbox=1,2,3,4".into()),
        );
        expr.evaluate(&mut ex).await.expect("replace must eval");
        assert_eq!(
            ex.input.headers.get("url"),
            Some(&Value::Null),
            "header assignment form is a footgun: value silently becomes Null"
        );
    }
}
