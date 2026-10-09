//! camel-language-rhai — Rhai script language for Camel Rust.
//!
//! Main types: `RhaiLanguage`, `RhaiExpression`, `RhaiPredicate`, `RhaiMutatingExpression`.
//! Read-only scripts access `body`, `headers`, `header()`, and `property()`;
//! `set_header()`/`set_property()` calls are rejected at create time — use a
//! mutating script expression (`headers["k"] = ...`) instead.
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
//! - The `body` variable mirrors the exchange body natively (task 2.2, B1):
//!   strings, JSON maps/arrays, blobs and unit for empty bodies. A streaming
//!   body binds as an access-aware refusal marker — any materializing read
//!   of it fails with a typed conversion error; the stream itself is never
//!   touched. See `docs/src/languages/rhai.md`.
//! - String methods such as `replace`, `trim`, and `pad` mutate the subject in
//!   place and return unit `()`. They do NOT return a new string. Call them as
//!   statements (`body.replace(",", "%2C");`). Never write
//!   `body = body.replace(...)` or `headers["k"] = headers["k"].replace(...)`:
//!   the right-hand side is unit, so the assignment stores unit. The body is
//!   cleared to `Empty` (unit maps back to the empty body) and a header entry
//!   becomes `Null`. See the `rhai_replace_*` tests and bd rc-2mjo.

use async_trait::async_trait;
use camel_api::{ErrorPosition, ExpressionErrorClass};
use camel_language_api::{
    Body, Exchange, Expression, Language, LanguageError, MutatingExpression, Predicate,
    RhaiLimitsConfig, Value,
};
use rhai::{
    AST, Engine, OptimizationLevel, Scope,
    packages::{Package, StandardPackage},
};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tracing::debug;

mod converter;
mod json;
mod stream_body;
mod transaction;

use converter::{dynamic_to_value, json_to_dynamic};
use stream_body::{StreamBodyRef, StreamCounters};
use transaction::{EntryDelta, map_deltas, rhai_values_differ, value_to_body};

// Test-only compile counter. Per-thread (via `thread_local!`) so parallel
// test execution does not perturb the counter that a single test observes.
// Incremented each time `engine.compile` is called from a `create_*`
// method. Read-only `create_*` calls compile TWICE (walk-AST at
// `OptimizationLevel::None` + exec-AST at `Simple`); the mutating path
// compiles once. The regression tests assert the counter delta per
// `create_*` call and 0 after N `evaluate`s — proving the AST is stored at
// create time and reused at eval time, not re-compiled.
#[cfg(test)]
thread_local! {
    pub(crate) static COMPILE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Result type for mutating eval (returns value + modified exchange fields).
///
/// The body is `Option<Body>`: `None` means the script never assigned it, so
/// the caller must leave the original body (variant included) untouched.
type EvalMutResult = Result<
    (
        Value,
        Option<Body>,
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

/// Classify a `rhai::EvalAltResult` by its innermost kind.
///
/// Payload-blind: classification keys on the variant shape only, never on
/// wrapped value text. A later mapping layer (change
/// `language-value-boundary` task 2.2) may inspect ONLY the dedicated guard
/// sentinel and the type-signature/type-list fields of
/// `ErrorFunctionNotFound`/`ErrorIndexingType` — never `ErrorRuntime`
/// payloads or `Display` text.
fn eval_alt_class(e: &rhai::EvalAltResult) -> ExpressionErrorClass {
    use rhai::EvalAltResult;
    match e.unwrap_inner() {
        EvalAltResult::ErrorArithmetic(..) => ExpressionErrorClass::Arithmetic,
        EvalAltResult::ErrorMismatchOutputType(..) | EvalAltResult::ErrorMismatchDataType(..) => {
            ExpressionErrorClass::TypeMismatch
        }
        EvalAltResult::ErrorFunctionNotFound(..) => ExpressionErrorClass::FunctionNotFound,
        EvalAltResult::ErrorTooManyOperations(_)
        | EvalAltResult::ErrorTooManyModules(_)
        | EvalAltResult::ErrorStackOverflow(_)
        | EvalAltResult::ErrorDataTooLarge(..)
        | EvalAltResult::ErrorTooManyVariables(_) => ExpressionErrorClass::Limit,
        EvalAltResult::ErrorTerminated(..) => ExpressionErrorClass::Timeout,
        EvalAltResult::ErrorParsing(..) => ExpressionErrorClass::Parse,
        _ => ExpressionErrorClass::Runtime,
    }
}

/// Optional `detail` text for a `rhai::EvalAltResult`.
///
/// Redaction contract (ADR-0012, change `language-value-boundary`): value
/// bearing kinds (`ErrorArithmetic`, `ErrorRuntime`, `ErrorMismatchDataType`,
/// `ErrorIndexingType`, `ErrorPropertyNotFound`, ...) carry `None` — thrown
/// values and operands never reach the error. Only short static
/// operand-free strings are attached for function-not-found and
/// limit/timeout kinds.
fn eval_alt_detail(e: &rhai::EvalAltResult) -> Option<String> {
    use rhai::EvalAltResult;
    let detail = match e.unwrap_inner() {
        EvalAltResult::ErrorFunctionNotFound(..) => "function not found",
        EvalAltResult::ErrorTooManyOperations(_)
        | EvalAltResult::ErrorTooManyModules(_)
        | EvalAltResult::ErrorStackOverflow(_)
        | EvalAltResult::ErrorDataTooLarge(..)
        | EvalAltResult::ErrorTooManyVariables(_) => "evaluation limit exceeded",
        EvalAltResult::ErrorTerminated(..) => "script terminated",
        _ => return None,
    };
    Some(detail.to_string())
}

/// Map a rhai `Position` to an [`ErrorPosition`].
///
/// `column: 0` means "top of the line" (rhai reports no column there);
/// `Position::NONE` maps to `None`.
fn to_error_position(p: rhai::Position) -> Option<ErrorPosition> {
    let line = u32::try_from(p.line()?).ok()?;
    let column = p
        .position()
        .map_or(0, |c| u32::try_from(c).unwrap_or(u32::MAX));
    Some(ErrorPosition { line, column })
}

/// Map an eval error, honoring the stream-refusal mapping first (task 2.2):
/// the guard sentinel and structured type-signature/type-list mentions of
/// `StreamBodyRef` become the `Body::Stream` conversion error; everything
/// else falls through to the structured 1.7 mapper.
fn eval_error(e: &rhai::EvalAltResult, target: &str) -> LanguageError {
    stream_body::map_stream_refusal(e, target).unwrap_or_else(|| map_eval_alt(e))
}

/// Map a `rhai::EvalAltResult` to a structured, redacted [`LanguageError`].
///
/// Classification uses the innermost error (`unwrap_inner`); position is the
/// deepest available (inner first, outer fallback).
fn map_eval_alt(e: &rhai::EvalAltResult) -> LanguageError {
    let inner = e.unwrap_inner();
    let position = to_error_position(inner.position().or_else(e.position()));
    LanguageError::EvalFailure {
        class: eval_alt_class(e),
        position,
        detail: eval_alt_detail(e),
    }
}

/// Type name of a [`Value`] for `TypeMismatch` diagnostics. Type names only —
/// never runtime values.
fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "bool",
        Value::Null => "null",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Callee names that mutate the exchange — forbidden in read-only
/// expressions and predicates (change `language-value-boundary`, B4).
const READ_ONLY_MUTATION_FNS: [&str; 2] = ["set_property", "set_header"];

/// Walk the whole AST (statements, expressions, nested script-function
/// bodies, blocks, `if`/`switch`/loop bodies) and return the first forbidden
/// mutation callee found, if any.
///
/// Uses the rhai `internals` walker (`AST::walk`), which recurses through
/// every `Stmt`/`Expr` shape including dotted chains and method calls.
fn find_read_only_mutation(ast: &AST) -> Option<&'static str> {
    let mut found: Option<&'static str> = None;
    ast.walk(&mut |path: &[rhai::ASTNode]| {
        for node in path {
            let callee = match node {
                rhai::ASTNode::Expr(
                    rhai::Expr::FnCall(call, ..) | rhai::Expr::MethodCall(call, ..),
                ) => Some(call.name.as_str()),
                rhai::ASTNode::Stmt(rhai::Stmt::FnCall(call, ..)) => Some(call.name.as_str()),
                _ => None,
            };
            let Some(callee) = callee else { continue };
            for forbidden in READ_ONLY_MUTATION_FNS {
                if callee == forbidden {
                    found = Some(forbidden);
                    return false;
                }
            }
        }
        true
    });
    found
}

/// Rhai scripting language for rust-camel.
///
/// Read-only expressions and predicates have access to:
/// - `body` — exchange body, natively typed (string / map / array / blob /
///   unit); a streaming body binds as a refusal marker that fails any
///   materializing read
/// - `headers` — exchange headers as a Rhai Map
/// - `header(name)` — look up a header value by name
/// - `property(name)` — look up an exchange property by name
///
/// `set_header()` and `set_property()` are **rejected at create time** for
/// read-only expressions (parse error); they are also never registered on
/// the eval engine, so dynamic calls cannot slip through. To mutate the
/// exchange, use a mutating script expression with map assignment syntax
/// (see `RhaiMutatingExpression`).
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
        // Shared JSON host module (task 1.5): registers `parse_json`/`to_json`
        // and the `json value`/`json number` wrapper surface. Rhai inserts
        // global-module functions at index 1, so the host module takes
        // precedence over the stock `StandardPackage` JSON helpers.
        engine.register_global_module(json::host::host_module());
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
        // Stream-body refusal guards (task 2.2, B1): the marker type plus
        // the registered guard surface (`to_string`, `to_debug`, the binary
        // operators over marker/scalar shapes). Compile-time engines get
        // them too — registrations have no effect on parsing or folding.
        stream_body::register_stream_guards(&mut engine);
        engine
    }

    /// Convert the exchange body into the rhai `body` variable (task 2.2, B1).
    ///
    /// Eager variants bind natively: `Text`/`Xml` as strings, `Json` through
    /// the fallible inbound converter, `Empty` as unit, `Bytes` as a blob.
    /// `Stream` binds as an access-aware [`StreamBodyRef`] marker — no
    /// stream data is read and no handle crosses the boundary. Callers that
    /// evaluate a script must pair this with [`StreamCounters::of_dynamic`]
    /// (via [`bind_body`]) so the post-eval rule can observe the marker.
    fn body_to_dynamic(body: &Body, target: &str) -> Result<rhai::Dynamic, LanguageError> {
        Ok(match body {
            Body::Text(s) | Body::Xml(s) => rhai::Dynamic::from(s.clone()),
            Body::Json(v) => json_to_dynamic(v, target)?,
            Body::Empty => rhai::Dynamic::UNIT,
            Body::Bytes(b) => rhai::Dynamic::from(rhai::Blob::from(b.as_ref())),
            Body::Stream(_) => rhai::Dynamic::from(StreamBodyRef::new()),
            // Forward compatibility (Body is #[non_exhaustive]): an unknown
            // future variant must fail the inbound conversion, never bind as
            // a silent empty string.
            _ => {
                return Err(LanguageError::ConversionError {
                    source_type: "unrecognized Body variant".to_string(),
                    target: target.to_string(),
                });
            }
        })
    }

    /// Bind the `body` scope variable: the converted value plus, for a
    /// stream body, the marker's counters for the post-eval refusal check.
    /// Counters are borrowed before the marker enters the scope (plain `Arc`
    /// handles — no marker clone, `reads` stays 0).
    fn bind_body(body: &Body) -> Result<(rhai::Dynamic, Option<StreamCounters>), LanguageError> {
        let d = Self::body_to_dynamic(body, "body")?;
        let counters = StreamCounters::of_dynamic(&d);
        Ok((d, counters))
    }

    /// Build a scope with `body` and `headers` variables from the exchange.
    ///
    /// Inbound conversion is fallible: a header or property holding a value
    /// rhai cannot represent (u64 > i64::MAX, sealed Q4) fails the evaluation
    /// before the script runs.
    fn make_scope(
        exchange: &Exchange,
    ) -> Result<(Scope<'static>, rhai::Map, rhai::Map), LanguageError> {
        let mut scope = Scope::new();
        let (body, _) = Self::bind_body(&exchange.input.body)?;
        scope.push("body", body);

        let mut headers = rhai::Map::new();
        for (k, v) in &exchange.input.headers {
            headers.insert(k.clone().into(), json_to_dynamic(v, "header entry")?);
        }
        scope.push("headers", headers.clone());

        let mut properties = rhai::Map::new();
        for (k, v) in &exchange.properties {
            properties.insert(k.clone().into(), json_to_dynamic(v, "property entry")?);
        }

        Ok((scope, headers, properties))
    }

    /// Create a read-only eval engine with `header()` and `property()`
    /// registered as native readers. A new engine per eval avoids sharing
    /// mutable state between evaluations.
    ///
    /// `set_header()`/`set_property()` are deliberately NOT registered:
    /// read-only expressions reject them at create time (see
    /// [`RhaiLanguage::compile_read_only`]), and leaving them unregistered
    /// is the dynamic-call backstop.
    fn create_eval_engine(
        limits: &RhaiLimitsConfig,
        headers: rhai::Map,
        properties: rhai::Map,
    ) -> Engine {
        let mut engine = Self::create_base_engine(limits);

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

        engine
    }

    /// Compile a read-only expression or predicate under the two-AST
    /// discipline (change `language-value-boundary`, B4):
    ///
    /// 1. **Walk-AST** — compiled at `OptimizationLevel::None` on a
    ///    compile-scoped engine so no constant folding can hide or reorder a
    ///    forbidden setter call. The AST is walked
    ///    ([`find_read_only_mutation`]) and any `set_property`/`set_header`
    ///    call is rejected at create time.
    /// 2. **Exec-AST** — a SECOND compilation at `OptimizationLevel::Simple`
    ///    (explicit Simple). This
    ///    is the AST that is cached and evaluated; task 2.2's
    ///    discarded-statement exemption depends on `Simple` folding.
    ///
    /// The walk-AST is validation-only and discarded after the check.
    fn compile_read_only(script: &str, limits: &RhaiLimitsConfig) -> Result<AST, LanguageError> {
        let mut walk_engine = Self::create_base_engine(limits);
        walk_engine.set_optimization_level(OptimizationLevel::None);
        let walk_ast = walk_engine.compile(script).map_err(|e| {
            debug!(error = %e, "rhai expression compile failed");
            LanguageError::ParseError {
                expr: script.to_string(),
                reason: e.to_string(),
            }
        })?;
        #[cfg(test)]
        COMPILE_COUNT.with(|c| c.set(c.get() + 1));

        if let Some(callee) = find_read_only_mutation(&walk_ast) {
            return Err(LanguageError::ParseError {
                expr: script.to_string(),
                reason: format!(
                    "{callee}() cannot be used in a read-only expression; use a script: step instead"
                ),
            });
        }

        let mut exec_engine = Self::create_base_engine(limits);
        // Explicit `Simple` (not left to the engine default): task 2.2's
        // discarded-statement exemption (`body;` optimized away) depends on
        // this exact level.
        exec_engine.set_optimization_level(OptimizationLevel::Simple);
        let exec_ast = exec_engine.compile(script).map_err(|e| {
            debug!(error = %e, "rhai expression compile failed");
            LanguageError::ParseError {
                expr: script.to_string(),
                reason: e.to_string(),
            }
        })?;
        #[cfg(test)]
        COMPILE_COUNT.with(|c| c.set(c.get() + 1));
        Ok(exec_ast)
    }

    /// Sync eval for non-mutating expressions. Extracts exchange data into
    /// owned values, builds engine+scope, runs the pre-compiled AST, and
    /// returns the script's last expression value. Compilation happens at
    /// `create_*` time, never here.
    ///
    /// A stream body binds as the access-aware marker; the post-eval rule
    /// (task 2.2) refuses the evaluation when the script materialized the
    /// stream without a forgiving registered guard hit.
    fn eval_sync(
        ast: &rhai::AST,
        limits: &RhaiLimitsConfig,
        body: rhai::Dynamic,
        headers_map: rhai::Map,
        properties_map: rhai::Map,
    ) -> Result<Value, LanguageError> {
        let mut scope = Scope::new();
        let stream = StreamCounters::of_dynamic(&body);
        scope.push("body", body);
        scope.push("headers", headers_map.clone());

        let engine = Self::create_eval_engine(limits, headers_map, properties_map);

        // eval_ast_with_scope reuses the pre-compiled AST — no re-parse per eval.
        // The AST was built once in `create_expression` / `create_predicate`.
        let result: rhai::Dynamic = engine
            .eval_ast_with_scope(&mut scope, ast)
            .map_err(|e| eval_error(&e, "value"))?;

        // POST-EVAL RULE: an unforgiven stream materialization refuses.
        if let Some(counters) = &stream {
            counters.refuse_if_unforgiven("value")?;
        }

        dynamic_to_value(result, "value")
    }

    /// Sync eval for mutating expressions. Takes owned exchange fields, runs
    /// the pre-compiled AST, and returns the result value + committed fields.
    ///
    /// The write-back is a validate-all-then-commit transaction (task 2.3,
    /// B2 + B3): the pre-eval native snapshots are compared with the post-eval
    /// scope values using [`rhai_values_differ`] (type-sensitive). Only
    /// added/changed/removed entries are converted (generic targets) and
    /// committed; the body is written only when it was assigned, so a
    /// header-only script leaves every body variant bit-identical. A
    /// conversion failure returns `Err` with no mutation applied.
    ///
    /// A stream body binds as the access-aware marker; the post-eval rule
    /// (task 2.2) refuses the evaluation when the script materialized the
    /// stream without a forgiving registered guard hit. A still-marker body is
    /// classified unassigned without cloning or counting it.
    fn eval_mut_sync(
        ast: &rhai::AST,
        limits: &RhaiLimitsConfig,
        body: Body,
        headers: HashMap<String, Value>,
        properties: HashMap<String, Value>,
    ) -> EvalMutResult {
        // 1. Pre-eval snapshots: native rhai values built from the exchange.
        let mut pre_headers = rhai::Map::new();
        for (k, v) in &headers {
            pre_headers.insert(k.clone().into(), json_to_dynamic(v, "header entry")?);
        }
        let mut pre_properties = rhai::Map::new();
        for (k, v) in &properties {
            pre_properties.insert(k.clone().into(), json_to_dynamic(v, "property entry")?);
        }

        // A stream marker must not be cloned for snapshotting (a clone counts
        // as a read and would wrongly trip the refusal rule). Eager bodies are
        // cloned once so the post-eval comparison can detect an assignment.
        let (body_dyn, stream) = Self::bind_body(&body)?;
        let pre_body = if stream.is_none() {
            Some(body_dyn.clone())
        } else {
            None
        };

        // 2. Scope with the pre-eval snapshots.
        let mut scope = Scope::new();
        scope.push("headers", pre_headers.clone());
        scope.push("properties", pre_properties.clone());
        scope.push_dynamic("body", body_dyn);

        // 3. Run the pre-compiled AST (no re-parse per eval). The engine is
        // reused below as the frozen scalar-comparison engine.
        let engine = RhaiLanguage::create_base_engine(limits);
        let result: rhai::Dynamic = engine
            .eval_ast_with_scope(&mut scope, ast)
            .map_err(|e| eval_error(&e, "value"))?;

        // POST-EVAL RULE: an unforgiven stream materialization refuses
        // before any write-back — no partial mutation.
        if let Some(counters) = &stream {
            counters.refuse_if_unforgiven("value")?;
        }

        // 4. Post-eval maps. A whole-variable reassignment to a non-map is a
        // typed boundary violation, never a silent fallback: reusing the
        // pre-eval snapshot would drop every mutation the script made to that
        // container before the invalid assignment while still committing
        // unrelated body/other-map changes. Borrow the scope value (never
        // clone it) and refuse a wrong type with a payload-blind error naming
        // the generic container; the transaction commits nothing.
        let post_headers = Self::scope_map(&scope, "headers")?;
        let post_properties = Self::scope_map(&scope, "properties")?;

        // 5. Validate every pending change BEFORE committing any of it —
        // generic entry targets only, runtime keys never enter diagnostics.
        let header_deltas = map_deltas(&engine, &pre_headers, &post_headers, "header entry")?;
        let property_deltas =
            map_deltas(&engine, &pre_properties, &post_properties, "property entry")?;
        let body_change = Self::body_change(&scope, &engine, pre_body.as_ref(), stream.as_ref())?;
        let result_value = dynamic_to_value(result, "value")?;

        // 6. Commit: start from the original owned maps so untouched entries
        // keep their original value handle.
        let mut out_headers = headers;
        for (k, delta) in header_deltas {
            match delta {
                EntryDelta::Set(v) => {
                    out_headers.insert(k, v);
                }
                EntryDelta::Remove => {
                    out_headers.remove(&k);
                }
            }
        }
        let mut out_properties = properties;
        for (k, delta) in property_deltas {
            match delta {
                EntryDelta::Set(v) => {
                    out_properties.insert(k, v);
                }
                EntryDelta::Remove => {
                    out_properties.remove(&k);
                }
            }
        }

        Ok((result_value, body_change, out_headers, out_properties))
    }

    /// Classify the post-eval `body` as assigned (converted) or unassigned.
    ///
    /// A streaming body that is still the marker is unassigned by definition;
    /// the check borrows the marker without cloning it. An eager body is
    /// compared with its pre-eval snapshot through [`rhai_values_differ`].
    fn body_change(
        scope: &Scope<'_>,
        engine: &Engine,
        pre_body: Option<&rhai::Dynamic>,
        stream: Option<&StreamCounters>,
    ) -> Result<Option<Body>, LanguageError> {
        let Some(post) = scope.get("body") else {
            return Ok(None);
        };
        if stream.is_some() {
            // Still-marker => unassigned (comparison marker==marker).
            if post.read_lock::<StreamBodyRef>().is_some() {
                return Ok(None);
            }
            return Ok(Some(value_to_body(dynamic_to_value(post.clone(), "body")?)));
        }
        match pre_body {
            Some(pre_body) if rhai_values_differ(engine, pre_body, post) => {
                Ok(Some(value_to_body(dynamic_to_value(post.clone(), "body")?)))
            }
            _ => Ok(None),
        }
    }

    /// Borrow the post-eval `headers`/`properties` scope value as a map.
    ///
    /// A whole-variable reassignment to a non-map is a typed boundary
    /// violation, not a silent no-op: falling back to the pre-eval snapshot
    /// would discard every mutation the script made to that container before
    /// the invalid assignment while still committing unrelated changes.
    /// Borrows through a read lock (no clone, so a container holding a stream
    /// marker is not counted as a read) and refuses a wrong type with a
    /// payload-blind [`LanguageError::ConversionError`] naming the generic
    /// container — never a runtime key or value.
    fn scope_map<'a>(
        scope: &'a Scope<'_>,
        container: &'static str,
    ) -> Result<rhai::DynamicReadLock<'a, rhai::Map>, LanguageError> {
        let value = scope
            .get(container)
            .ok_or_else(|| LanguageError::ConversionError {
                source_type: "missing".to_string(),
                target: container.to_string(),
            })?;
        value
            .read_lock::<rhai::Map>()
            .ok_or_else(|| LanguageError::ConversionError {
                source_type: value.type_name().to_string(),
                target: container.to_string(),
            })
    }
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
    let body = RhaiLanguage::body_to_dynamic(&exchange.input.body, "body")?;
    let (_, headers_map, properties_map) = RhaiLanguage::make_scope(exchange)?;
    let limits = limits.clone();

    tokio::time::timeout(timeout, async move {
        tokio::task::spawn_blocking(move || {
            RhaiLanguage::eval_sync(&ast, &limits, body, headers_map, properties_map)
        })
        .await
        .map_err(|_join| {
            // Panic payload may carry exchange data — never render it.
            LanguageError::EvalFailure {
                class: ExpressionErrorClass::Runtime,
                position: None,
                detail: None,
            }
        })?
    })
    .await
    .map_err(|_elapsed| LanguageError::EvalFailure {
        class: ExpressionErrorClass::Timeout,
        position: None,
        detail: None,
    })?
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
        // Strict bool: predicates must evaluate to a real boolean. No
        // truthiness coercion — any other value (including Null) is a type
        // error (change `language-value-boundary`).
        match &val {
            Value::Bool(b) => Ok(*b),
            other => Err(LanguageError::TypeMismatch {
                expected: "bool".to_string(),
                actual: value_type_name(other).to_string(),
                position: None,
            }),
        }
    }
}

/// A Rhai script expression that can mutate the Exchange during evaluation.
///
/// The script has access to three mutable scope variables:
/// - `headers` — a Rhai map (`#{}`) representing the exchange headers
/// - `properties` — a Rhai map (`#{}`) representing the exchange properties
/// - `body` — the exchange body, natively typed (a streaming body binds as
///   a refusal marker; assigning a value replaces it)
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
                // Commit the transaction: only an assigned body is replaced,
                // so a header-only script leaves the body bit-identical.
                if let Some(body) = out_body {
                    exchange.input.body = body;
                }
                exchange.input.headers = out_headers;
                exchange.properties = out_properties;
                Ok(value)
            }
            Ok(Ok(Err(e))) => {
                // Eval error — exchange untouched (implicit rollback)
                Err(e)
            }
            Ok(Err(_join_err)) => {
                // Panic payload may carry exchange data — never render it.
                Err(LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Runtime,
                    position: None,
                    detail: None,
                })
            }
            Err(_elapsed) => {
                // Timeout — exchange untouched
                Err(LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Timeout,
                    position: None,
                    detail: None,
                })
            }
        }
    }
}

impl Language for RhaiLanguage {
    fn name(&self) -> &'static str {
        "rhai"
    }

    fn create_expression(&self, script: &str) -> Result<Box<dyn Expression>, LanguageError> {
        // Two-AST discipline: walk-AST (None) enforces read-only purity,
        // exec-AST (Simple) is cached for evaluation. Parse errors surface
        // here at route construction rather than first message.
        let ast = Self::compile_read_only(script, &self.limits)?;
        debug!("rhai expression compiled");
        Ok(Box::new(RhaiExpression {
            ast: Arc::new(ast),
            limits: self.limits.clone(),
        }))
    }

    fn create_predicate(&self, script: &str) -> Result<Box<dyn Predicate>, LanguageError> {
        let ast = Self::compile_read_only(script, &self.limits)?;
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
            debug!(error = %e, "rhai expression compile failed");
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
    use camel_api::ExpressionErrorClass;
    use camel_language_api::{
        EvalMeta, Exchange, Language, LanguageError, Message, Value, to_expression_failed,
    };
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
    async fn test_rhai_property_access() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression(r#"property("myProp")"#).unwrap();
        let mut ex = exchange_with_body("test");
        ex.set_property("myProp".to_string(), Value::String("propVal".to_string()));
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(val, Value::String("propVal".to_string()));
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
    async fn test_rhai_function_not_found_has_class_and_position() {
        // Calling a nonexistent function yields a structured FunctionNotFound
        // failure with the engine-reported position.
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression("nonexistent_fn()").unwrap();
        let ex = exchange_with_body("test");
        let err = expr.evaluate(&ex).await.unwrap_err();
        match &err {
            LanguageError::EvalFailure {
                class, position, ..
            } => {
                assert!(
                    matches!(class, ExpressionErrorClass::FunctionNotFound),
                    "expected FunctionNotFound, got: {class:?}"
                );
                assert!(position.is_some(), "position must be reported: {err:?}");
            }
            other => panic!("expected EvalFailure, got: {other:?}"),
        }
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
        // Ops limit high enough that the timeout deterministically wins
        // (rhai runs at opt-level 2 in tests); either guard must terminate
        // the script fast.
        let limits = RhaiLimitsConfig {
            max_operations: Some(500_000_000),
            execution_timeout_ms: Some(50),
            ..Default::default()
        };
        let lang = RhaiLanguage::with_limits(limits);
        // CPU-bound loop; the timeout must terminate it fast.
        let expr = lang.create_expression("loop {}").unwrap();
        let ex = exchange_with_body("test");
        let start = std::time::Instant::now();
        let result = expr.evaluate(&ex).await;
        let elapsed = start.elapsed();
        let err = result.expect_err("must error");
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "must terminate fast: {elapsed:?}"
        );
        assert!(
            matches!(
                &err,
                LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Timeout,
                    ..
                }
            ),
            "expected Timeout class, got: {err:?}"
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
        // B1 (task 2.2): an Empty body binds as unit, not as an empty string.
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression("body").unwrap();
        let ex = Exchange::new(Message::default());
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(val, Value::Null);
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
    async fn test_compile_count_expression_is_two_per_create() {
        use super::COMPILE_COUNT;
        let lang = RhaiLanguage::new();
        let ex = exchange_with_body("test");

        let before = COMPILE_COUNT.with(|c| c.get());
        let expr = lang.create_expression("body + 1").unwrap();
        let after_create = COMPILE_COUNT.with(|c| c.get());
        // Two-AST discipline: walk-AST (None) + exec-AST (Simple).
        assert_eq!(
            after_create - before,
            2,
            "create_expression must compile exactly twice (delta={})",
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
    async fn test_compile_count_predicate_is_two_per_create() {
        use super::COMPILE_COUNT;
        let lang = RhaiLanguage::new();
        let ex = exchange_with_body("test");

        let before = COMPILE_COUNT.with(|c| c.get());
        let pred = lang
            .create_predicate(r#"header("type") == "order""#)
            .unwrap();
        let after_create = COMPILE_COUNT.with(|c| c.get());
        // Two-AST discipline: walk-AST (None) + exec-AST (Simple).
        assert_eq!(
            after_create - before,
            2,
            "create_predicate must compile exactly twice (delta={})",
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
    //   - assignment form `body = body.replace()` → wrong: RHS is `()`, so the
    //     task 2.3 transaction records an explicit body assignment and writes
    //     `Empty` (the replaced string is discarded) — or, for a header entry,
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

    /// Assignment form (FOOTGUN): `body = body.replace(...)` mutates `body`
    /// in place and then assigns the method's unit result to it. The task 2.3
    /// transaction detects the assignment (unit differs from the original
    /// string) and writes the body back as `Empty` — the replaced string is
    /// discarded, never silently kept.
    #[tokio::test]
    async fn rhai_replace_assigned_to_body_becomes_empty() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"body = body.replace(",", "%2C")"#)
            .expect("replace must compile");
        let mut ex = exchange_with_body("bbox=1,2,3,4");
        expr.evaluate(&mut ex).await.expect("replace must eval");
        assert!(
            matches!(ex.input.body, Body::Empty),
            "assignment form assigns unit: body becomes Empty, got {:?}",
            ex.input.body
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

    // ── language-value-boundary task 1.7 ──
    //
    // Structured, redacted error mapping; strict-bool predicates; read-only
    // mutation rejection. The tests below pin the NEW contract; they were
    // written red first (against the old stringly/coercing behavior).

    /// Trusted route metadata for `to_expression_failed` renderings.
    fn eval_meta() -> EvalMeta {
        EvalMeta {
            language: "rhai".to_string(),
            route_id: "route".to_string(),
            step_id: "step".to_string(),
            verb: "set_property".to_string(),
            target: Some("body".to_string()),
        }
    }

    #[tokio::test]
    async fn parse_float_error_is_class_and_position_only() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_expression(r#""SECRET".parse_float()"#)
            .expect("compile must succeed (raw parse errors surface at eval)");
        let ex = exchange_with_body("test");
        let err = expr.evaluate(&ex).await.expect_err("parse_float must fail");
        assert!(
            matches!(
                &err,
                LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Arithmetic,
                    ..
                }
            ),
            "expected Arithmetic EvalFailure, got: {err:?}"
        );
        // Every rendering of the error chain must be redacted.
        let dbg = format!("{err:?}");
        let disp = format!("{err}");
        let camel = format!("{}", to_expression_failed(err, &eval_meta()));
        for rendering in [&dbg, &disp, &camel] {
            assert!(!rendering.contains("SECRET"), "secret leaked: {rendering}");
        }
    }

    #[tokio::test]
    async fn predicate_non_bool_is_type_mismatch() {
        let lang = RhaiLanguage::new();
        let ex = exchange_with_body("test");

        let pred = lang.create_predicate(r#""false""#).expect("compile");
        let err = pred
            .matches(&ex)
            .await
            .expect_err("string predicate must not coerce");
        assert!(
            matches!(
                &err,
                LanguageError::TypeMismatch {
                    expected,
                    actual,
                    ..
                } if expected == "bool" && actual == "string"
            ),
            "expected bool/string TypeMismatch, got: {err:?}"
        );

        let pred = lang.create_predicate("42").expect("compile");
        let err = pred
            .matches(&ex)
            .await
            .expect_err("number predicate must not coerce");
        assert!(
            matches!(
                &err,
                LanguageError::TypeMismatch {
                    expected,
                    actual,
                    ..
                } if expected == "bool" && actual == "number"
            ),
            "expected bool/number TypeMismatch, got: {err:?}"
        );

        let pred = lang.create_predicate("true").expect("compile");
        assert!(pred.matches(&ex).await.expect("bool predicate matches"));
    }

    #[tokio::test]
    async fn predicate_null_is_type_mismatch() {
        let lang = RhaiLanguage::new();
        let pred = lang.create_predicate("()").expect("compile");
        let ex = exchange_with_body("test");
        let err = pred
            .matches(&ex)
            .await
            .expect_err("null must not coerce to false");
        assert!(
            matches!(
                &err,
                LanguageError::TypeMismatch {
                    expected,
                    actual,
                    ..
                } if expected == "bool" && actual == "null"
            ),
            "expected bool/null TypeMismatch, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn read_only_set_property_is_compile_error() {
        let lang = RhaiLanguage::new();
        for script in [r#"set_property("k", 1)"#, r#"set_header("k", 1)"#] {
            let Err(err) = lang.create_expression(script) else {
                panic!("read-only setter must be rejected at create time: {script}");
            };
            assert!(
                matches!(&err, LanguageError::ParseError { reason, .. } if reason.contains("script:")),
                "expected ParseError mentioning `script:`, got: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn nested_read_only_setter_is_compile_error() {
        let lang = RhaiLanguage::new();
        let Err(err) = lang.create_expression(r#"if true { set_property("k", 1) }"#) else {
            panic!("setter nested in a block must be rejected");
        };
        assert!(
            matches!(&err, LanguageError::ParseError { .. }),
            "expected ParseError, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn limit_error_class_is_limit_not_runtime() {
        use camel_language_api::RhaiLimitsConfig;
        let limits = RhaiLimitsConfig {
            max_operations: Some(10),
            ..Default::default()
        };
        let lang = RhaiLanguage::with_limits(limits);
        let expr = lang.create_expression("loop { }").expect("compile");
        let ex = exchange_with_body("test");
        let err = expr.evaluate(&ex).await.expect_err("must trip the limit");
        assert!(
            matches!(
                &err,
                LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Limit,
                    ..
                }
            ),
            "expected Limit class, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn wrapped_error_in_function_classifies_inner() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_expression(r#"fn f() { "no".parse_float(); } f()"#)
            .expect("compile");
        let ex = exchange_with_body("test");
        let err = expr.evaluate(&ex).await.expect_err("must fail");
        match &err {
            LanguageError::EvalFailure {
                class, position, ..
            } => {
                assert!(
                    matches!(class, ExpressionErrorClass::Arithmetic),
                    "inner Arithmetic must not collapse to Runtime, got: {class:?}"
                );
                let pos = position.expect("deepest available position must be reported");
                assert_eq!(pos.line, 1, "position must be inside the script: {pos}");
            }
            other => panic!("expected EvalFailure, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn throw_text_mentioning_marker_stays_runtime() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_expression(r#"throw "StreamBodyRef""#)
            .expect("compile");
        let ex = exchange_with_body("test");
        let err = expr.evaluate(&ex).await.expect_err("throw must fail");
        assert!(
            matches!(
                &err,
                LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Runtime,
                    detail: None,
                    ..
                }
            ),
            "thrown text must stay Runtime and redacted, got: {err:?}"
        );
        assert!(!format!("{err:?}").contains("StreamBodyRef"));
        assert!(!format!("{err}").contains("StreamBodyRef"));
    }

    #[tokio::test]
    async fn read_only_sentinel_text_throw_stays_runtime() {
        // Spoof attempt: the operator controls the thrown value. Even the
        // exact string once used as the guard sentinel must not be
        // reclassified as a `Body::Stream` conversion on a Text body. The
        // guard sentinel is a private typed payload, not a string, so no
        // user-visible value can imitate it.
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_expression(r#"throw "STREAM_SENTINEL""#)
            .expect("compile");
        let ex = exchange_with_body("test");
        let err = expr.evaluate(&ex).await.expect_err("throw must fail");
        assert!(
            matches!(
                &err,
                LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Runtime,
                    detail: None,
                    ..
                }
            ),
            "thrown sentinel text must stay Runtime and redacted, got: {err:?}"
        );
        assert!(
            !matches!(&err, LanguageError::ConversionError { .. }),
            "thrown text must not become a conversion refusal: {err:?}"
        );
        for rendering in [format!("{err:?}"), format!("{err}")] {
            assert!(
                !rendering.contains("STREAM_SENTINEL"),
                "thrown value leaked: {rendering}"
            );
        }
    }

    #[tokio::test]
    async fn mutating_sentinel_text_throw_stays_runtime() {
        // Same spoof attempt on the mutating path: a Text body plus
        // `throw "STREAM_SENTINEL"` is a plain redacted runtime failure, and
        // the exchange is unchanged (implicit rollback).
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"throw "STREAM_SENTINEL""#)
            .expect("compile");
        let mut ex = exchange_with_body("test");
        let err = expr.evaluate(&mut ex).await.expect_err("throw must fail");
        assert!(
            matches!(
                &err,
                LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Runtime,
                    detail: None,
                    ..
                }
            ),
            "thrown sentinel text must stay Runtime and redacted, got: {err:?}"
        );
        assert!(
            !matches!(&err, LanguageError::ConversionError { .. }),
            "thrown text must not become a conversion refusal: {err:?}"
        );
        for rendering in [format!("{err:?}"), format!("{err}")] {
            assert!(
                !rendering.contains("STREAM_SENTINEL"),
                "thrown value leaked: {rendering}"
            );
        }
        assert_eq!(ex.input.body.as_text(), Some("test"), "rollback must hold");
    }

    #[tokio::test]
    async fn timeout_maps_to_timeout_class() {
        use camel_language_api::RhaiLimitsConfig;
        // Ops limit high enough that the 20 ms timeout always wins (rhai runs
        // at opt-level 2 in tests, so an empty loop burns millions of
        // operations per millisecond); the detached worker still terminates
        // on its own afterwards.
        let limits = RhaiLimitsConfig {
            max_operations: Some(500_000_000),
            execution_timeout_ms: Some(20),
            ..Default::default()
        };
        let lang = RhaiLanguage::with_limits(limits);
        let expr = lang.create_expression("loop {}").expect("compile");
        let ex = exchange_with_body("test");
        let err = expr.evaluate(&ex).await.expect_err("timeout must fire");
        assert!(
            matches!(
                &err,
                LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Timeout,
                    ..
                }
            ),
            "expected Timeout class, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn throw_still_redacted() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_expression(r#"throw "SECRET""#)
            .expect("compile");
        let ex = exchange_with_body("test");
        let err = expr.evaluate(&ex).await.expect_err("throw must fail");
        assert!(
            matches!(
                &err,
                LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Runtime,
                    ..
                }
            ),
            "expected Runtime class, got: {err:?}"
        );
        let dbg = format!("{err:?}");
        let disp = format!("{err}");
        let camel = format!("{}", to_expression_failed(err, &eval_meta()));
        for rendering in [&dbg, &disp, &camel] {
            assert!(!rendering.contains("SECRET"), "secret leaked: {rendering}");
        }
    }

    // ── language-value-boundary task 2.2: native body exposure (B1) ──
    //
    // Eager bodies bind natively (string / map / array / blob / unit);
    // streaming bodies bind as an access-aware refusal marker
    // (`StreamBodyRef`). Any materializing read of the marker fails with a
    // typed `ConversionError { source_type: "Body::Stream" }`; a script that
    // never materializes the stream succeeds and the stream handle stays
    // bit-identical and unconsumed. These tests pin the spike table so a
    // rhai upgrade that changes clone or optimization semantics surfaces.

    use bytes::Bytes;
    use camel_api::error::CamelError;
    use camel_api::{Body, StreamBody};
    use futures::stream::{BoxStream, StreamExt};
    use std::sync::Arc;

    /// Shared handle to a stream body's inner stream (identity + unconsumed
    /// assertions).
    type StreamHandle =
        Arc<tokio::sync::Mutex<Option<BoxStream<'static, Result<Bytes, CamelError>>>>>;

    /// An exchange with a streaming body plus its inner stream handle.
    fn stream_exchange() -> (Exchange, StreamHandle) {
        let stream: BoxStream<'static, Result<Bytes, CamelError>> =
            futures::stream::empty().boxed();
        let handle = Arc::new(tokio::sync::Mutex::new(Some(stream)));
        let msg = Message {
            body: Body::Stream(StreamBody {
                stream: handle.clone(),
                metadata: Default::default(),
            }),
            ..Default::default()
        };
        (Exchange::new(msg), handle)
    }

    fn assert_stream_body_conversion(err: &LanguageError) {
        assert!(
            matches!(
                err,
                LanguageError::ConversionError { source_type, .. }
                    if source_type == "Body::Stream"
            ),
            "expected Body::Stream ConversionError, got: {err:?}"
        );
    }

    fn assert_body_still_stream(ex: &Exchange, handle: &StreamHandle) {
        match &ex.input.body {
            Body::Stream(sb) => {
                assert!(
                    Arc::ptr_eq(&sb.stream, handle),
                    "stream identity must be unchanged"
                );
                assert!(
                    sb.stream.try_lock().expect("stream mutex").is_some(),
                    "stream must be unconsumed"
                );
            }
            other => panic!("body must remain Body::Stream, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn json_body_reads_as_map() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression(r#"type_of(body) == "map""#).unwrap();
        let obj: Value = r#"{"n":1}"#.parse().unwrap();
        let ex = Exchange::new(Message::new(obj));
        assert_eq!(expr.evaluate(&ex).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test]
    async fn xml_body_reads_as_string_and_keeps_variant() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression(r#"body == "<a/>""#).unwrap();
        let msg = Message {
            body: Body::Xml("<a/>".to_string()),
            ..Default::default()
        };
        let ex = Exchange::new(msg);
        assert_eq!(expr.evaluate(&ex).await.unwrap(), Value::Bool(true));
        assert!(
            matches!(ex.input.body, Body::Xml(_)),
            "read-only eval must keep the Xml variant"
        );
    }

    #[tokio::test]
    async fn bytes_body_reads_as_blob() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_expression(r#"type_of(body) == "blob""#)
            .unwrap();
        let msg = Message {
            body: Body::Bytes(Bytes::from(vec![1_u8, 2])),
            ..Default::default()
        };
        let ex = Exchange::new(msg);
        assert_eq!(expr.evaluate(&ex).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test]
    async fn empty_body_is_unit() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression(r#"type_of(body) == "()""#).unwrap();
        let ex = Exchange::new(Message::default());
        assert_eq!(expr.evaluate(&ex).await.unwrap(), Value::Bool(true));
    }

    #[tokio::test]
    async fn marker_named_missing_function_stays_function_not_found() {
        // A missing function whose CALLEE NAME contains the marker name must
        // not be reclassified as a stream conversion. Only the operand-type
        // list of the engine's `ErrorFunctionNotFound` signature may name the
        // marker, and only as an exact `StreamBodyRef` token.
        for script in ["StreamBodyRef()", "not_StreamBodyRef_fn()"] {
            let lang = RhaiLanguage::new();
            let expr = lang.create_expression(script).expect("compile");
            let ex = exchange_with_body("test");
            let err = expr.evaluate(&ex).await.expect_err("missing fn must fail");
            assert!(
                matches!(
                    &err,
                    LanguageError::EvalFailure {
                        class: ExpressionErrorClass::FunctionNotFound,
                        ..
                    }
                ),
                "{script}: expected FunctionNotFound, got: {err:?}"
            );
            assert!(
                !matches!(&err, LanguageError::ConversionError { .. }),
                "{script}: callee name must not map to conversion: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn stream_body_fails_loudly_when_read() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression("body.to_string()").unwrap();
        let (ex, _handle) = stream_exchange();
        let err = expr
            .evaluate(&ex)
            .await
            .expect_err("reading a stream body must refuse");
        assert_stream_body_conversion(&err);
    }

    #[tokio::test]
    async fn stream_body_untouched_script_succeeds() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"headers["h"] = "v""#)
            .unwrap();
        let (mut ex, handle) = stream_exchange();
        expr.evaluate(&mut ex)
            .await
            .expect("a script that never touches the stream body must succeed");
        assert_eq!(ex.input.headers.get("h").and_then(Value::as_str), Some("v"));
        assert_body_still_stream(&ex, &handle);
    }

    #[tokio::test]
    async fn stream_body_index_and_arithmetic_fail_loudly() {
        // `body[0]`: indexing raises ErrorIndexingType (type list names the
        // marker); `body + 1` and `body == body`: registered guard sentinel.
        for script in ["body[0]", "body + 1", "body == body"] {
            let lang = RhaiLanguage::new();
            let expr = lang.create_expression(script).unwrap();
            let (ex, _handle) = stream_exchange();
            let err = expr
                .evaluate(&ex)
                .await
                .expect_err("stream access must refuse with a conversion error");
            assert_stream_body_conversion(&err);
        }
    }

    #[tokio::test]
    async fn stream_body_bare_read_in_result_position_fails() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression("body").unwrap();
        let (ex, _handle) = stream_exchange();
        let err = expr
            .evaluate(&ex)
            .await
            .expect_err("marker result must be rejected by the result converter");
        assert_stream_body_conversion(&err);
    }

    #[tokio::test]
    async fn stream_body_alias_fails_on_use() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_expression("let x = body; x.to_string()")
            .unwrap();
        let (ex, _handle) = stream_exchange();
        let err = expr.evaluate(&ex).await.expect_err("alias use must refuse");
        assert_stream_body_conversion(&err);
    }

    #[tokio::test]
    async fn stream_body_type_of_returns_marker_name_documented() {
        // Documented exception 1: `type_of` reads the static type name
        // without cloning the marker (reads=0) — no stream data accessed.
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression("type_of(body)").unwrap();
        let (ex, _handle) = stream_exchange();
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(val, Value::String("StreamBodyRef".to_string()));
    }

    #[tokio::test]
    async fn stream_body_read_discarded_result_fails() {
        // The clone counter catches captures whose value is never used.
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression("let x = body; 42").unwrap();
        let (ex, _handle) = stream_exchange();
        let err = expr
            .evaluate(&ex)
            .await
            .expect_err("vanished capture must refuse");
        assert_stream_body_conversion(&err);
    }

    #[tokio::test]
    async fn stream_body_caught_method_read_is_suppressed_e7() {
        // Guard hits whose failures are handled in-script are forgiven
        // (E7 in-script error handling): the outer fallback variable wins.
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_expression(
                "let r = 0; try { body.to_string(); r = 1; } catch (err) { r = 42; } r",
            )
            .unwrap();
        let (ex, _handle) = stream_exchange();
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(val, Value::from(42));
    }

    #[tokio::test]
    async fn stream_body_caught_index_read_is_suppressed_e7() {
        // Indexing raises BEFORE cloning the marker (reads=0), so the
        // structured failure is an ordinary, fully suppressible script error.
        // rhai 1.26 discards the catch block's value: a try-catch whose
        // catch ran evaluates to unit (pinned with the spike row: Ok).
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_expression("try { let a = body[0]; 1 } catch (err) { 42 }")
            .unwrap();
        let (ex, _handle) = stream_exchange();
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(val, Value::Null);
    }

    #[tokio::test]
    async fn stream_body_caught_comparison_is_suppressed_e7() {
        // Binary guard counts both operand clones (2 reads / 2 hits),
        // forgiven when caught in-script.
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_expression("let r = 0; try { r = (body == body); } catch (err) { r = 42; } r")
            .unwrap();
        let (ex, _handle) = stream_exchange();
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(val, Value::from(42));
    }

    #[tokio::test]
    async fn stream_body_capture_after_caught_guard_still_fails() {
        // A forgiven guard hit does not license a later RAW capture
        // (reads=2 > hits=1): the post-eval rule still refuses.
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_expression(
                "let r = 0; try { body.to_string(); r = 1; } catch (err) { r = 2; }; let x = body; 99",
            )
            .unwrap();
        let (ex, _handle) = stream_exchange();
        let err = expr
            .evaluate(&ex)
            .await
            .expect_err("raw capture after a caught guard must refuse");
        assert_stream_body_conversion(&err);
    }

    #[tokio::test]
    async fn stream_body_caught_unsupported_op_still_fails() {
        // Documented residue: an operation OUTSIDE the registered guard
        // surface (`%`) that is CAUGHT in-script still fails post-eval —
        // rhai exposes no caught-error hook, so the boundary refusal cannot
        // see the catch. No partial mutation and the stream stays intact.
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(
                "let r = 0; try { let q = body % 1; r = 1; } catch (err) { r = 42; } r",
            )
            .unwrap();
        let (mut ex, handle) = stream_exchange();
        let err = expr
            .evaluate(&mut ex)
            .await
            .expect_err("unsupported-op residue must refuse");
        assert_stream_body_conversion(&err);
        assert_body_still_stream(&ex, &handle);
        assert!(ex.input.headers.is_empty(), "no partial mutation");
        assert!(ex.properties.is_empty(), "no partial mutation");
    }

    #[tokio::test]
    async fn stream_body_block_capture_fails() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression("{ let x = body; } 42").unwrap();
        let (ex, _handle) = stream_exchange();
        let err = expr
            .evaluate(&ex)
            .await
            .expect_err("block-scoped capture must refuse");
        assert_stream_body_conversion(&err);
    }

    #[tokio::test]
    async fn stream_body_alias_overwritten_fails() {
        // Overwritten aliases still counted as materializations.
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression("let x = body; x = 0; 42").unwrap();
        let (ex, _handle) = stream_exchange();
        let err = expr
            .evaluate(&ex)
            .await
            .expect_err("overwritten alias must refuse");
        assert_stream_body_conversion(&err);
    }

    #[tokio::test]
    async fn stream_body_nested_in_array_fails() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression("let x = [body]; 42").unwrap();
        let (ex, _handle) = stream_exchange();
        let err = expr
            .evaluate(&ex)
            .await
            .expect_err("array nesting must refuse");
        assert_stream_body_conversion(&err);
    }

    #[tokio::test]
    async fn stream_body_fn_arg_fails() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression("fn f(v) { 42 } f(body)").unwrap();
        let (ex, _handle) = stream_exchange();
        let err = expr
            .evaluate(&ex)
            .await
            .expect_err("marker as function argument must refuse");
        assert_stream_body_conversion(&err);
    }

    #[tokio::test]
    async fn stream_body_discarded_statement_is_noop() {
        // Documented exception 2: a bare discarded statement-expression is
        // folded away by the Simple exec-AST — nothing materializes.
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression("body; 42").unwrap();
        let (ex, _handle) = stream_exchange();
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(val, Value::from(42));
    }

    #[tokio::test]
    async fn stream_body_overwrite_assignment_allowed() {
        let lang = RhaiLanguage::new();

        // Plain assignment replaces the marker without materializing it.
        let expr = lang
            .create_mutating_expression(r#"body = "replacement""#)
            .unwrap();
        let (mut ex, _handle) = stream_exchange();
        expr.evaluate(&mut ex).await.unwrap();
        assert_eq!(ex.input.body.as_text(), Some("replacement"));

        // A read in the SAME script returns the replaced value.
        let expr = lang
            .create_mutating_expression(r#"body = "r"; body"#)
            .unwrap();
        let (mut ex, _handle) = stream_exchange();
        let val = expr.evaluate(&mut ex).await.unwrap();
        assert_eq!(val, Value::String("r".to_string()));
        assert_eq!(ex.input.body.as_text(), Some("r"));
    }

    #[tokio::test]
    async fn stream_body_read_after_replacement_succeeds() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"body = "r"; body.to_string()"#)
            .unwrap();
        let (mut ex, _handle) = stream_exchange();
        let val = expr.evaluate(&mut ex).await.unwrap();
        assert_eq!(val, Value::String("r".to_string()));
        assert_eq!(ex.input.body.as_text(), Some("r"));
    }

    // ── language-value-boundary task 2.3: mutating-script transaction (B2 + B3) ──
    //
    // The write-back is validate-all-then-commit: the evaluator compares the
    // pre-eval snapshots with the post-eval scope using a recursive,
    // TYPE-SENSITIVE comparison (an int-to-float change is a change, even
    // though rhai's `==` treats `1 == 1.0` as true). Only added/changed/
    // removed entries are converted (generic targets only) and committed;
    // untouched entries keep their original value and variant, and a
    // conversion failure leaves the exchange untouched.

    #[tokio::test]
    async fn header_only_script_preserves_json_body_bit_identical() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"headers["h"] = "v""#)
            .unwrap();
        let json: Value = r#"{"a":[1,2]}"#.parse().unwrap();
        let before = json.to_string();
        let mut ex = Exchange::new(Message::new(Body::Json(json)));
        expr.evaluate(&mut ex).await.unwrap();
        match &ex.input.body {
            Body::Json(v) => assert_eq!(v.to_string(), before, "body must be bit-identical"),
            other => panic!("body variant must stay Json, got {other:?}"),
        }
        assert_eq!(ex.input.headers.get("h"), Some(&Value::String("v".into())));
    }

    #[tokio::test]
    async fn header_only_script_preserves_xml_body_variant() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"headers["h"] = "v""#)
            .unwrap();
        let msg = Message {
            body: Body::Xml("<a/>".to_string()),
            ..Default::default()
        };
        let mut ex = Exchange::new(msg);
        expr.evaluate(&mut ex).await.unwrap();
        match &ex.input.body {
            Body::Xml(s) => assert_eq!(s, "<a/>"),
            other => panic!("body variant must stay Xml, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn empty_body_stays_empty() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"headers["h"] = "v""#)
            .unwrap();
        let mut ex = Exchange::new(Message::default());
        expr.evaluate(&mut ex).await.unwrap();
        assert!(
            matches!(ex.input.body, Body::Empty),
            "Empty must not become Text(\"\")"
        );
    }

    #[tokio::test]
    async fn untouched_map_property_stays_object() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"properties["other"] = 1"#)
            .unwrap();
        let map: Value = r#"{"a":[1,2]}"#.parse().unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.properties.insert("m".to_string(), map.clone());
        expr.evaluate(&mut ex).await.unwrap();
        let stored = ex.properties.get("m").expect("m must remain");
        assert!(
            stored.is_object(),
            "untouched map must stay an object, got {stored:?}"
        );
        assert_eq!(stored, &map);
    }

    #[tokio::test]
    async fn removed_header_is_removed() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"headers.remove("x")"#)
            .unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.input.headers.insert("x".to_string(), Value::from(1));
        expr.evaluate(&mut ex).await.unwrap();
        assert!(
            !ex.input.headers.contains_key("x"),
            "removed header must be gone"
        );
    }

    #[tokio::test]
    async fn body_assignment_writes_json() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"body = #{ "k": 1 };"#)
            .unwrap();
        let mut ex = Exchange::new(Message::default());
        expr.evaluate(&mut ex).await.unwrap();
        let expected: Value = r#"{"k":1}"#.parse().unwrap();
        assert_eq!(ex.input.body, Body::Json(expected));
    }

    #[tokio::test]
    async fn conversion_failure_commits_nothing() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(
                r#"headers["h"] = "changed"; properties["ok"] = 1; properties["bad"] = 0.0/0.0;"#,
            )
            .unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.input
            .headers
            .insert("h".to_string(), Value::String("orig".into()));
        ex.properties.insert("ok".to_string(), Value::from(0));
        ex.properties.insert("bad".to_string(), Value::from(0));
        let err = expr
            .evaluate(&mut ex)
            .await
            .expect_err("NaN conversion must refuse");
        assert!(
            matches!(err, LanguageError::ConversionError { .. }),
            "expected ConversionError, got {err:?}"
        );
        assert_eq!(
            ex.input.headers.get("h"),
            Some(&Value::String("orig".into())),
            "header diff must not be applied"
        );
        assert_eq!(ex.properties.get("ok"), Some(&Value::from(0)));
        assert_eq!(ex.properties.get("bad"), Some(&Value::from(0)));
    }

    #[tokio::test]
    async fn same_value_body_assignment_writes_nothing() {
        let lang = RhaiLanguage::new();
        let expr = lang.create_mutating_expression(r#"body = body;"#).unwrap();
        let json: Value = r#"{"a":[1,2]}"#.parse().unwrap();
        let before = json.to_string();
        let mut ex = Exchange::new(Message::new(Body::Json(json)));
        expr.evaluate(&mut ex).await.unwrap();
        match &ex.input.body {
            Body::Json(v) => assert_eq!(v.to_string(), before),
            other => panic!("body must stay Json, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unexecuted_assignment_is_untouched() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"properties["other"] = 1"#)
            .unwrap();
        let json: Value = r#"{"a":[1,2]}"#.parse().unwrap();
        let before = json.to_string();
        let mut ex = Exchange::new(Message::new(Body::Json(json)));
        expr.evaluate(&mut ex).await.unwrap();
        match &ex.input.body {
            Body::Json(v) => assert_eq!(v.to_string(), before),
            other => panic!("body must stay Json, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn nested_map_change_is_detected() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"properties["m"]["inner"] = 42"#)
            .unwrap();
        let map: Value = r#"{"a":[1,2]}"#.parse().unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.properties.insert("m".to_string(), map);
        expr.evaluate(&mut ex).await.unwrap();
        let m = ex.properties.get("m").expect("m must remain");
        assert_eq!(m["inner"], Value::from(42));
        assert_eq!(m["a"], r#"[1,2]"#.parse::<Value>().unwrap());
    }

    #[tokio::test]
    async fn numeric_type_change_is_a_change() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"properties["n"] = 1.0; body = 1.0;"#)
            .unwrap();
        let mut ex = Exchange::new(Message::new(Body::Json(Value::from(1))));
        ex.properties.insert("n".to_string(), Value::from(1));
        expr.evaluate(&mut ex).await.unwrap();

        let prop = ex.properties.get("n").expect("n must remain");
        assert!(
            prop.as_i64().is_none() && prop.as_f64() == Some(1.0),
            "int-to-float change must be written back as 1.0, got {prop:?}"
        );
        match &ex.input.body {
            Body::Json(v) => assert!(
                v.as_i64().is_none() && v.as_f64() == Some(1.0),
                "body int-to-float change must be written back as 1.0, got {v:?}"
            ),
            other => panic!("body must be Json, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn same_value_type_preserved_assignment_writes_nothing() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"properties["n"] = 1"#)
            .unwrap();
        let mut ex = Exchange::new(Message::default());
        let original = Value::from(1);
        ex.properties.insert("n".to_string(), original.clone());
        expr.evaluate(&mut ex).await.unwrap();
        let stored = ex.properties.get("n").expect("n must remain");
        assert_eq!(stored, &original, "same value/type must be preserved");
        assert!(stored.as_i64().is_some(), "must stay an integer");
    }

    #[tokio::test]
    async fn secret_key_never_enters_diagnostics() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(
                r#"properties["SECRET-KEY-" + headers.Authorization] = 0.0/0.0;"#,
            )
            .unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.input
            .headers
            .insert("Authorization".to_string(), Value::String("tok".into()));
        let err = expr
            .evaluate(&mut ex)
            .await
            .expect_err("NaN conversion must refuse");
        match &err {
            LanguageError::ConversionError { target, .. } => {
                assert_eq!(target, "property entry", "runtime key must not leak");
            }
            other => panic!("expected ConversionError, got {other:?}"),
        }
        let dbg = format!("{err:?}");
        let disp = format!("{err}");
        let camel = to_expression_failed(err, &eval_meta());
        let camel_dbg = format!("{camel:?}");
        let camel_disp = format!("{camel}");
        for rendering in [&dbg, &disp, &camel_dbg, &camel_disp] {
            assert!(
                !rendering.contains("SECRET-KEY"),
                "runtime key leaked: {rendering}"
            );
            assert!(!rendering.contains("tok"), "token leaked: {rendering}");
        }
    }

    // ── r_gpt finding: invalid whole-container assignment rolls back ──
    //
    // A script may reassign the whole `headers`/`properties` scope variable
    // to a non-map. The transaction must refuse with a typed, payload-blind
    // error naming the GENERIC container and commit NOTHING. The pre-fix
    // fallback silently reused the pre-eval snapshot, dropping the container's
    // own mutations while still committing unrelated body/other-map changes.

    #[tokio::test]
    async fn invalid_headers_container_rolls_back_all_changes() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(
                r#"body = "changed"; properties["p"] = 1; headers = "SECRET-PAYLOAD";"#,
            )
            .unwrap();
        let mut ex = Exchange::new(Message::new("original"));
        ex.input
            .headers
            .insert("h".to_string(), Value::String("orig".into()));
        ex.properties.insert("p".to_string(), Value::from(0));

        let err = expr
            .evaluate(&mut ex)
            .await
            .expect_err("invalid headers container must refuse");

        match &err {
            LanguageError::ConversionError { target, .. } => {
                assert_eq!(target, "headers", "generic container target only");
            }
            other => panic!("expected ConversionError, got {other:?}"),
        }

        // Exact pre-step state: no body write, no property write, no header
        // loss.
        assert_eq!(ex.input.body.as_text(), Some("original"));
        assert_eq!(
            ex.input.headers.get("h"),
            Some(&Value::String("orig".into())),
            "header entry must be untouched"
        );
        assert_eq!(ex.properties.get("p"), Some(&Value::from(0)));

        // Payload-blind: no rendering may leak the invalid assignment value.
        let dbg = format!("{err:?}");
        let disp = format!("{err}");
        let camel = to_expression_failed(err, &eval_meta());
        let camel_dbg = format!("{camel:?}");
        let camel_disp = format!("{camel}");
        for rendering in [&dbg, &disp, &camel_dbg, &camel_disp] {
            assert!(
                !rendering.contains("SECRET-PAYLOAD"),
                "assigned payload leaked: {rendering}"
            );
        }
    }

    #[tokio::test]
    async fn invalid_properties_container_rolls_back_all_changes() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(
                r#"body = "changed"; headers["h"] = "changed"; properties = 0.0/0.0;"#,
            )
            .unwrap();
        let mut ex = Exchange::new(Message::new("original"));
        ex.input
            .headers
            .insert("h".to_string(), Value::String("orig".into()));
        ex.properties
            .insert("p".to_string(), Value::String("orig".into()));

        let err = expr
            .evaluate(&mut ex)
            .await
            .expect_err("invalid properties container must refuse");

        match &err {
            LanguageError::ConversionError { target, .. } => {
                assert_eq!(target, "properties", "generic container target only");
            }
            other => panic!("expected ConversionError, got {other:?}"),
        }

        assert_eq!(ex.input.body.as_text(), Some("original"));
        assert_eq!(
            ex.input.headers.get("h"),
            Some(&Value::String("orig".into()))
        );
        assert_eq!(ex.properties.get("p"), Some(&Value::String("orig".into())));

        // The container misuse is reported by type name only; the assigned
        // NaN scalar can never appear as text in any rendering.
        let dbg = format!("{err:?}");
        let disp = format!("{err}");
        let camel = to_expression_failed(err, &eval_meta());
        let camel_dbg = format!("{camel:?}");
        let camel_disp = format!("{camel}");
        for rendering in [&dbg, &disp, &camel_dbg, &camel_disp] {
            assert!(
                !rendering.contains("NaN") && !rendering.contains("nan"),
                "assigned payload leaked: {rendering}"
            );
        }
    }

    // ── perf baseline: per-eval registration + eager scope-prep cost ──
    //
    // Task 1.1 records the pre-change median per-eval cost so the shared-module
    // cost introduced in Task 1.5 can be compared against it. The big phase
    // reproduces the `rc-m01r9` eager `make_scope`/`prepare_scope` deep
    // conversion: `json_to_dynamic` walks every property entry on every eval.

    /// Build an exchange whose single property `"buf"` holds an object with
    /// `property_entries` integer entries (`i` -> `i` for `0..property_entries`).
    fn bench_exchange(property_entries: usize) -> Exchange {
        let mut ex = Exchange::new(Message::new(""));
        let buf = Value::Object(
            (0..property_entries)
                .map(|i| (i.to_string(), Value::from(i as i64)))
                .collect(),
        );
        ex.set_property("buf", buf);
        ex
    }

    /// Conventional median (middle element for odd counts, mean of the two
    /// middle elements for even counts) over raw nanosecond samples.
    fn median_ns(samples: &mut [u128]) -> u128 {
        samples.sort_unstable();
        let n = samples.len();
        if n % 2 == 1 {
            samples[n / 2]
        } else {
            (samples[n / 2 - 1] + samples[n / 2]) / 2
        }
    }

    #[test]
    #[ignore = "slow test: perf baseline; run explicitly with --release"]
    fn bench_json_host_per_eval_cost() {
        use camel_language_api::RhaiLimitsConfig;

        let lang = RhaiLanguage::with_limits(RhaiLimitsConfig {
            execution_timeout_ms: Some(60_000),
            ..Default::default()
        });
        let expr = lang.create_expression("1 + 1").unwrap();

        let small_ex = bench_exchange(0);
        let big_ex = bench_exchange(4000);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let measure = |ex: &Exchange| -> u128 {
            for _ in 0..5 {
                let _ = rt.block_on(expr.evaluate(ex)).unwrap();
            }
            let mut samples = Vec::with_capacity(20);
            for _ in 0..20 {
                let start = std::time::Instant::now();
                let _ = rt.block_on(expr.evaluate(ex)).unwrap();
                samples.push(start.elapsed().as_nanos());
            }
            median_ns(&mut samples)
        };

        let small_ns = measure(&small_ex);
        let big_ns = measure(&big_ex);

        println!("BENCH json_host_per_eval small_ns={small_ns} big_ns={big_ns}");

        assert!(small_ns > 0, "small_ns median must be positive");
        assert!(big_ns > 0, "big_ns median must be positive");
        assert!(
            big_ns >= small_ns,
            "eager scope cost must be reproduced: big_ns={big_ns} < small_ns={small_ns}"
        );
    }

    // ── rhai-json-fidelity task 1.5: engine integration + outbound refusal ──
    //
    // The shared host module is registered on every base engine, so the
    // `json value`/`json number` wrapper surface shadows the stock JSON
    // helpers. Direct outbound wrapper conversion is refused with the stable
    // labels; every scenario below runs through `RhaiLanguage`.

    /// Evaluate a read-only script on a fresh default-limits language.
    async fn eval_script(script: &str, ex: &Exchange) -> Result<Value, LanguageError> {
        let lang = RhaiLanguage::new();
        let expr = lang.create_expression(script).expect("script compiles");
        expr.evaluate(ex).await
    }

    /// Typed class of a failed evaluation.
    fn eval_err_class(err: &LanguageError) -> ExpressionErrorClass {
        err.class()
            .expect("test error must carry an evaluation class")
    }

    /// Assert a failed evaluation carries the exact `ExpressionErrorClass`.
    fn assert_class(err: &LanguageError, expected: ExpressionErrorClass, context: &str) {
        assert_eq!(eval_err_class(err), expected, "{context}: {err:?}");
    }

    /// Evaluate a script against an empty exchange and return its string.
    async fn json_script_out(script: &str) -> String {
        let ex = Exchange::new(Message::default());
        match eval_script(script, &ex)
            .await
            .expect("json script evaluates")
        {
            Value::String(s) => s,
            other => panic!("expected Value::String, got {other:?}"),
        }
    }

    fn empty_exchange() -> Exchange {
        Exchange::new(Message::default())
    }

    #[tokio::test]
    async fn json_host_helpers_win_on_expression_engine() {
        let ex = empty_exchange();
        let val = eval_script(r#"type_of(parse_json("{}"))"#, &ex)
            .await
            .expect("host parse_json runs");
        assert_eq!(val, Value::String("json value".to_string()));
    }

    #[tokio::test]
    async fn json_host_helpers_win_on_mutating_engine() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"type_of(parse_json("[]"))"#)
            .unwrap();
        let mut ex = empty_exchange();
        let val = expr.evaluate(&mut ex).await.expect("host parse_json runs");
        assert_eq!(val, Value::String("json value".to_string()));
    }

    #[tokio::test]
    async fn json_host_helpers_win_on_predicate_engine() {
        let lang = RhaiLanguage::new();
        let pred = lang
            .create_predicate(r#"type_of(parse_json("{}")) == "json value""#)
            .unwrap();
        let ex = empty_exchange();
        assert!(pred.matches(&ex).await.expect("predicate evaluates"));
    }

    #[tokio::test]
    async fn json_escaped_solidus_accepted() {
        let out = json_script_out(r#"to_json(parse_json("{\"path\":\"a\\/b\"}"))"#).await;
        assert!(out.contains(r#""path":"a/b""#), "{out}");
    }

    #[tokio::test]
    async fn json_large_magnitude_round_trips_exact() {
        let out = json_script_out(
            r#"to_json(parse_json("[18446744073709551615,123456789012345678901234567890,1e400]"))"#,
        )
        .await;
        assert_eq!(
            out,
            "[18446744073709551615,123456789012345678901234567890,1e400]"
        );
    }

    #[tokio::test]
    async fn json_integer_forms_project_decimals_wrap() {
        let ex = empty_exchange();
        let script = r#"let j = parse_json("[9223372036854775807,-0,1.0,1e2,18446744073709551615]"); [type_of(j[0]),type_of(j[1]),type_of(j[2]),type_of(j[3]),type_of(j[4])]"#;
        let val = eval_script(script, &ex).await.expect("projection runs");
        assert_eq!(
            val,
            Value::Array(vec![
                Value::String("i64".to_string()),
                Value::String("i64".to_string()),
                Value::String("json number".to_string()),
                Value::String("json number".to_string()),
                Value::String("json number".to_string()),
            ])
        );
    }

    #[tokio::test]
    async fn json_to_float_1e400_is_arithmetic() {
        let ex = empty_exchange();
        let err = eval_script(r#"parse_json("1e400").to_float()"#, &ex)
            .await
            .expect_err("non-finite to_float must fail");
        assert_class(&err, ExpressionErrorClass::Arithmetic, "to_float 1e400");
    }

    #[tokio::test]
    async fn json_authored_order_retained() {
        let out = json_script_out(r#"to_json(parse_json("{\"z\":1,\"a\":2,\"m\":3}"))"#).await;
        assert_eq!(out, r#"{"z":1,"a":2,"m":3}"#);
    }

    #[tokio::test]
    async fn json_duplicate_key_keeps_first_position_last_value() {
        let out = json_script_out(r#"to_json(parse_json("{\"b\":1,\"a\":2,\"b\":3}"))"#).await;
        assert_eq!(out, r#"{"b":3,"a":2}"#);
    }

    #[tokio::test]
    async fn json_remove_a_keeps_order() {
        let out = json_script_out(
            r#"let j = parse_json("{\"a\":1,\"b\":2,\"c\":3}"); j.remove("a"); to_json(j)"#,
        )
        .await;
        assert_eq!(out, r#"{"b":2,"c":3}"#);
    }

    #[tokio::test]
    async fn json_round_trip_equivalent_not_byte_identical() {
        let out =
            json_script_out(r#"to_json(parse_json("{ \"a\" : 1 , \"b\" : \"x\\/y\" }"))"#).await;
        assert_eq!(out, r#"{"a":1,"b":"x/y"}"#);
    }

    #[tokio::test]
    async fn json_bare_read_isolated() {
        let out = json_script_out(
            r#"let j = parse_json("{\"a\":{\"b\":1}}"); let x = j["a"]; x["b"] = 9; to_json(j)"#,
        )
        .await;
        assert!(out.contains(r#""b":1"#), "source must be isolated: {out}");
    }

    #[tokio::test]
    async fn json_type_of_stable_labels() {
        let ex = empty_exchange();
        let val = eval_script(
            r#"[type_of(parse_json("{}")), type_of(parse_json("1.5"))]"#,
            &ex,
        )
        .await
        .expect("type_of runs");
        assert_eq!(
            val,
            Value::Array(vec![
                Value::String("json value".to_string()),
                Value::String("json number".to_string()),
            ])
        );
    }

    #[tokio::test]
    async fn json_chained_dot_negative_index() {
        let out = json_script_out(
            r#"let j = parse_json("{\"a\":[{\"b\":1}]}"); j["a"][0]["b"] = 2; j.a[-1].b = 3; to_json(j)"#,
        )
        .await;
        assert_eq!(out, r#"{"a":[{"b":3}]}"#);
    }

    #[tokio::test]
    async fn json_out_of_range_index_error_array_bounds() {
        let ex = empty_exchange();
        let err = eval_script(r#"let j = parse_json("[1]"); j[5]"#, &ex)
            .await
            .expect_err("out-of-range index must fail");
        // The integration boundary (`LanguageError::EvalFailure`) carries only
        // the class: `ErrorArrayBounds` classifies as Runtime here. The exact
        // inner variant is pinned at the host layer; see the test-design-gap
        // report for the plan's `inner ErrorArrayBounds` wording.
        assert_class(&err, ExpressionErrorClass::Runtime, "out-of-range index");
    }

    #[tokio::test]
    async fn json_missing_key_versus_null() {
        let ex = empty_exchange();
        let script = r#"let j = parse_json("{\"n\":null}"); [j["n"] == (), j["m"] == (), j.contains("n"), j.contains("m"), ("m" in j)]"#;
        let val = eval_script(script, &ex).await.expect("null semantics run");
        assert_eq!(
            val,
            Value::Array(vec![
                Value::Bool(true),
                Value::Bool(true),
                Value::Bool(true),
                Value::Bool(false),
                Value::Bool(false),
            ])
        );
    }

    #[tokio::test]
    async fn json_contains_on_array_type_mismatch() {
        let ex = empty_exchange();
        let err = eval_script(r#"parse_json("[1]").contains("k")"#, &ex)
            .await
            .expect_err("contains on an array must fail");
        assert_class(&err, ExpressionErrorClass::TypeMismatch, "array contains");
    }

    #[tokio::test]
    async fn json_wrong_container_getter_type_mismatch() {
        let ex = empty_exchange();
        for script in [r#"parse_json("[1]")["x"]"#, r#"parse_json("{}")[0]"#] {
            let err = eval_script(script, &ex)
                .await
                .expect_err("wrong-container read must fail");
            assert_class(&err, ExpressionErrorClass::TypeMismatch, script);
        }
    }

    #[tokio::test]
    async fn json_push_grows_remove_returns() {
        let ex = empty_exchange();
        let script = r#"let j = parse_json("[1,2]"); j.push(3); let r = j.remove(0); to_json(j) == "[2,3]" && r == 1 && j.remove(9) == ()"#;
        let val = eval_script(script, &ex).await.expect("push/remove runs");
        assert_eq!(val, Value::Bool(true));
    }

    #[tokio::test]
    async fn json_unit_assignment_stores_null() {
        let out = json_script_out(r#"let j = parse_json("{}"); j["k"] = (); to_json(j)"#).await;
        assert!(out.contains(r#""k":null"#), "{out}");
    }

    #[tokio::test]
    async fn json_nested_setter_failure_rolls_back() {
        let out = json_script_out(
            r#"let j = parse_json("{\"a\":{\"b\":1}}"); try { j["a"]["b"] = || 1; } catch (e) {} to_json(j)"#,
        )
        .await;
        assert!(out.contains(r#""b":1"#), "rollback must hold: {out}");
    }

    #[tokio::test]
    async fn json_all_six_comparisons_refuse_type_mismatch() {
        let ex = empty_exchange();
        let wrappers = [r#"parse_json("{}")"#, r#"parse_json("1.5")"#];
        let scalars = ["2", "2.5", r#""s""#, "true"];
        let mut cases = 0_usize;
        for op in ["==", "!=", "<", ">", "<=", ">="] {
            for wrapper in wrappers {
                for scalar in scalars {
                    for script in [
                        format!("{wrapper} {op} {scalar}"),
                        format!("{scalar} {op} {wrapper}"),
                    ] {
                        let err = eval_script(&script, &ex)
                            .await
                            .expect_err("registered comparison must refuse");
                        assert_class(&err, ExpressionErrorClass::TypeMismatch, &script);
                        cases += 1;
                    }
                }
            }
            for (a, b) in [(wrappers[0], wrappers[1]), (wrappers[1], wrappers[0])] {
                let script = format!("{a} {op} {b}");
                let err = eval_script(&script, &ex)
                    .await
                    .expect_err("cross-wrapper comparison must refuse");
                assert_class(&err, ExpressionErrorClass::TypeMismatch, &script);
                cases += 1;
            }
            for wrapper in wrappers {
                let script = format!("{wrapper} {op} {wrapper}");
                let err = eval_script(&script, &ex)
                    .await
                    .expect_err("same-wrapper comparison must refuse");
                assert_class(&err, ExpressionErrorClass::TypeMismatch, &script);
                cases += 1;
            }
        }
        assert_eq!(cases, 120, "the approved contract has 120 refusal cases");
    }

    #[tokio::test]
    async fn json_null_compares_with_unit() {
        let ex = empty_exchange();
        let val = eval_script(r#"parse_json("null") == ()"#, &ex)
            .await
            .expect("unit comparison runs");
        assert_eq!(val, Value::Bool(true));
    }

    #[tokio::test]
    async fn json_native_map_serializes_sorted_nested_inline() {
        let out = json_script_out(r#"let p = parse_json("{}"); to_json(#{"b":1,"a":p})"#).await;
        assert_eq!(out, r#"{"a":{},"b":1}"#);
    }

    #[tokio::test]
    async fn json_function_and_method_forms_agree() {
        let ex = empty_exchange();
        let val = eval_script(
            r#"let j = parse_json("{\"a\":1}"); to_json(j) == j.to_json()"#,
            &ex,
        )
        .await
        .expect("both forms run");
        assert_eq!(val, Value::Bool(true));
    }

    #[tokio::test]
    async fn json_raw_utf8_no_slash_escape() {
        let out = json_script_out(r#"to_json(parse_json("{\"s\":\"café\",\"p\":\"a/b\"}"))"#).await;
        assert!(out.contains("café"), "{out}");
        assert!(out.contains("a/b"), "{out}");
        assert!(!out.contains("\\/"), "{out}");
    }

    #[tokio::test]
    async fn json_nonfinite_float_refused() {
        let ex = empty_exchange();
        let err = eval_script(r#"to_json(#{"x": 0.0/0.0})"#, &ex)
            .await
            .expect_err("non-finite float must be refused");
        assert_class(&err, ExpressionErrorClass::TypeMismatch, "non-finite float");
        let rendered = format!("{err}");
        assert!(
            !rendered.contains("NaN") && !rendered.contains("nan"),
            "no debug fallback: {rendered}"
        );
    }

    #[tokio::test]
    async fn json_unsupported_value_refused_type_mismatch() {
        let ex = empty_exchange();
        let scripts = [
            r#"fn secret_fn_name() { 1 } to_json([Fn("secret_fn_name")])"#,
            r#"fn secret_fn_name() { 1 } to_json(#{"f": Fn("secret_fn_name")})"#,
            "to_json([timestamp()])",
            r#"to_json(#{"t": timestamp()})"#,
        ];
        for script in scripts {
            let err = eval_script(script, &ex)
                .await
                .expect_err("unsupported value must be refused");
            match &err {
                LanguageError::EvalFailure { class, detail, .. } => {
                    assert_eq!(*class, ExpressionErrorClass::TypeMismatch, "{script}");
                    assert!(detail.is_none(), "{script}: detail must be redacted");
                }
                other => panic!("{script}: expected EvalFailure, got {other:?}"),
            }
            for rendering in [format!("{err}"), format!("{err:?}")] {
                assert!(
                    !rendering.contains("secret_fn_name"),
                    "{script}: value leaked: {rendering}"
                );
            }
        }
    }

    #[tokio::test]
    async fn json_to_string_and_to_debug_are_compact_json() {
        let ex = empty_exchange();
        let val = eval_script(
            r#"let a = parse_json("{\"a\":1}").to_string(); let b = parse_json("{\"a\":1}").to_debug(); a == b && a == "{\"a\":1}""#,
            &ex,
        )
        .await
        .expect("string methods run");
        assert_eq!(val, Value::Bool(true));
    }

    #[tokio::test]
    async fn json_depth_boundary() {
        let ex = empty_exchange();
        let ok = format!(
            "to_json(parse_json(\"{}{}\"))",
            "[".repeat(128),
            "]".repeat(128)
        );
        let val = eval_script(&ok, &ex).await.expect("depth 128 parses");
        assert!(matches!(val, Value::String(_)), "depth 128 must parse");

        let bad = format!(
            "to_json(parse_json(\"{}{}\"))",
            "[".repeat(129),
            "]".repeat(129)
        );
        let err = eval_script(&bad, &ex)
            .await
            .expect_err("depth 129 must fail");
        assert_class(&err, ExpressionErrorClass::Limit, "depth 129");
    }

    #[tokio::test]
    async fn json_native_container_depth_limit() {
        let ex = empty_exchange();

        let map_ok =
            r#"let m = #{}; let n = 0; while n < 127 { m = #{ "x": m }; n += 1; } to_json(m)"#;
        let val = eval_script(map_ok, &ex)
            .await
            .expect("depth-128 map serializes");
        assert!(matches!(val, Value::String(_)));
        let map_bad =
            r#"let m = #{}; let n = 0; while n < 128 { m = #{ "x": m }; n += 1; } to_json(m)"#;
        let err = eval_script(map_bad, &ex)
            .await
            .expect_err("depth-129 map must fail");
        assert_class(&err, ExpressionErrorClass::Limit, "map depth 129");

        let arr_ok = r#"let a = []; let n = 0; while n < 127 { a = [a]; n += 1; } to_json(a)"#;
        let val = eval_script(arr_ok, &ex)
            .await
            .expect("depth-128 array serializes");
        assert!(matches!(val, Value::String(_)));
        let arr_bad = r#"let a = []; let n = 0; while n < 128 { a = [a]; n += 1; } to_json(a)"#;
        let err = eval_script(arr_bad, &ex)
            .await
            .expect_err("depth-129 array must fail");
        assert_class(&err, ExpressionErrorClass::Limit, "array depth 129");
    }

    #[tokio::test]
    async fn json_mutation_depth_and_cap_rechecked() {
        let lang = RhaiLanguage::with_limits(camel_language_api::RhaiLimitsConfig {
            max_array_size: Some(2),
            ..Default::default()
        });
        let expr = lang
            .create_expression(r#"let j = parse_json("[1,2]"); j.push(3)"#)
            .unwrap();
        let ex = empty_exchange();
        let err = expr
            .evaluate(&ex)
            .await
            .expect_err("push beyond the array cap must fail");
        assert_class(&err, ExpressionErrorClass::Limit, "push over cap");
    }

    #[tokio::test]
    async fn json_mutation_size_limit() {
        let lang = RhaiLanguage::with_limits(camel_language_api::RhaiLimitsConfig {
            max_string_size: Some(8),
            ..Default::default()
        });
        let expr = lang
            .create_expression(r#"let j = parse_json("{}"); j["key"] = "value";"#)
            .unwrap();
        let ex = empty_exchange();
        let err = expr
            .evaluate(&ex)
            .await
            .expect_err("mutation over the string cap must fail");
        assert_class(&err, ExpressionErrorClass::Limit, "mutation size cap");
    }

    #[tokio::test]
    async fn json_self_assignment_at_cap_refused() {
        let inner = format!("{}{}", "[".repeat(127), "]".repeat(127));
        let json = format!("{{\"a\":{inner}}}");
        let escaped = json.replace('"', "\\\"");
        let script = format!("let j = parse_json(\"{escaped}\"); j[\"a\"] = j");
        let ex = empty_exchange();
        let err = eval_script(&script, &ex)
            .await
            .expect_err("self-assignment at the cap must fail");
        assert_class(&err, ExpressionErrorClass::Limit, "self-assignment");
    }

    #[tokio::test]
    async fn json_limits_read_from_calling_engine() {
        let ex = empty_exchange();
        let limited = RhaiLanguage::with_limits(camel_language_api::RhaiLimitsConfig {
            max_array_size: Some(2),
            ..Default::default()
        });
        let expr = limited
            .create_expression(r#"to_json(parse_json("[1,2,3]"))"#)
            .unwrap();
        let err = expr
            .evaluate(&ex)
            .await
            .expect_err("engine cap must refuse");
        assert_class(&err, ExpressionErrorClass::Limit, "engine array cap");

        let default = RhaiLanguage::new();
        let expr = default
            .create_expression(r#"to_json(parse_json("[1,2,3]"))"#)
            .unwrap();
        assert_eq!(
            expr.evaluate(&ex).await.unwrap(),
            Value::String("[1,2,3]".to_string())
        );
    }

    #[tokio::test]
    async fn json_zero_limit_is_unlimited() {
        let lang = RhaiLanguage::with_limits(camel_language_api::RhaiLimitsConfig {
            max_array_size: Some(0),
            ..Default::default()
        });
        let ex = empty_exchange();
        let expr = lang
            .create_expression(r#"to_json(parse_json("[1,2,3]"))"#)
            .unwrap();
        assert_eq!(
            expr.evaluate(&ex).await.unwrap(),
            Value::String("[1,2,3]".to_string())
        );

        let ok = format!(
            "to_json(parse_json(\"{}{}\"))",
            "[".repeat(128),
            "]".repeat(128)
        );
        let expr = lang.create_expression(&ok).unwrap();
        assert!(matches!(
            expr.evaluate(&ex).await.unwrap(),
            Value::String(_)
        ));

        let bad = format!(
            "to_json(parse_json(\"{}{}\"))",
            "[".repeat(129),
            "]".repeat(129)
        );
        let expr = lang.create_expression(&bad).unwrap();
        let err = expr
            .evaluate(&ex)
            .await
            .expect_err("depth 129 must still fail");
        assert_class(&err, ExpressionErrorClass::Limit, "zero cap depth 129");
    }

    #[tokio::test]
    async fn json_arithmetic_and_helpers_function_not_found() {
        let ex = empty_exchange();
        let scripts = [
            r#"parse_json("1.5") + 1"#,
            r#"let j = parse_json("1.5"); j += 1"#,
            r#"parse_json("{}").values()"#,
            r#"parse_json("{}").merge(#{})"#,
        ];
        for script in scripts {
            let err = eval_script(script, &ex)
                .await
                .expect_err("unavailable operation must fail");
            assert_class(&err, ExpressionErrorClass::FunctionNotFound, script);
        }
    }

    #[tokio::test]
    async fn json_iteration_runtime_error() {
        let ex = empty_exchange();
        let err = eval_script(r#"for k in parse_json("{}") {}"#, &ex)
            .await
            .expect_err("iteration must fail");
        assert_class(&err, ExpressionErrorClass::Runtime, "for iteration");
    }

    #[tokio::test]
    async fn json_setter_failure_propagates() {
        let ex = empty_exchange();
        let err = eval_script(r#"let j = parse_json("{}"); j["k"] = || 1;"#, &ex)
            .await
            .expect_err("setter failure must propagate");
        // Class discrimination matters here: an `ErrorIndexingType` (which Rhai
        // discards during index-chain write-back) would classify as Runtime,
        // so TypeMismatch proves the typed setter failure propagated.
        assert_class(&err, ExpressionErrorClass::TypeMismatch, "setter failure");
    }

    #[tokio::test]
    async fn json_parse_error_is_parse_class_with_position() {
        let canary = "SECRET_TOKEN_XYZ";
        let script = format!(r#"parse_json("{canary}")"#);
        let ex = empty_exchange();
        let err = eval_script(&script, &ex)
            .await
            .expect_err("malformed input must fail");
        match &err {
            LanguageError::EvalFailure {
                class,
                position,
                detail,
            } => {
                assert_eq!(*class, ExpressionErrorClass::Parse);
                assert!(position.is_some(), "script call position must be set");
                assert!(detail.is_none(), "detail must be redacted");
            }
            other => panic!("expected EvalFailure, got {other:?}"),
        }
        let rendered = format!("{err}");
        assert!(!rendered.contains(canary), "secret leaked: {rendered}");
        assert!(
            !rendered.contains("line "),
            "parser line leaked: {rendered}"
        );
        assert!(
            !rendered.contains("column"),
            "parser column leaked: {rendered}"
        );
    }

    #[tokio::test]
    async fn json_inbound_u64_property_still_refused() {
        let mut ex = empty_exchange();
        ex.properties
            .insert("big".to_string(), Value::from(u64::MAX));
        let err = eval_script(r#"property("big")"#, &ex)
            .await
            .expect_err("u64 > i64::MAX must be refused");
        // Spec: the existing typed conversion error is unchanged. The task
        // wording says `type-mismatch`; observationally the inbound refusal is
        // the pre-existing `ConversionError` (class `conversion`). Reported as
        // a test-design-gap.
        assert_class(&err, ExpressionErrorClass::Conversion, "inbound u64");
        assert!(
            matches!(err, LanguageError::ConversionError { .. }),
            "inbound u64 must stay a conversion refusal: {err:?}"
        );
    }

    #[tokio::test]
    async fn json_explicit_serialization_yields_text_body() {
        let lang = RhaiLanguage::new();
        let expr = lang
            .create_mutating_expression(r#"body = to_json(parse_json("{\"a\":1}"))"#)
            .unwrap();
        let mut ex = empty_exchange();
        expr.evaluate(&mut ex).await.unwrap();
        assert_eq!(ex.input.body.as_text(), Some(r#"{"a":1}"#));
    }
}
