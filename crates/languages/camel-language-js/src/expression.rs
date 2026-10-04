//! Expression, MutatingExpression, and Predicate implementations for JS.

use std::{ops::ControlFlow, sync::Arc, time::Duration};

use async_trait::async_trait;
use camel_language_api::{Body, Exchange, ExpressionErrorClass, Value};
use camel_language_api::{Expression, LanguageError, MutatingExpression, Predicate};

use boa_engine::ast::expression::access::{
    PropertyAccess, PropertyAccessField, SimplePropertyAccess,
};
use boa_engine::ast::expression::operator::assign::{Assign, AssignTarget};
use boa_engine::ast::expression::operator::unary::{Unary, UnaryOp};
use boa_engine::ast::expression::operator::update::{Update, UpdateTarget};
use boa_engine::ast::expression::{Call, Expression as BoaExpression};
use boa_engine::ast::pattern::{ArrayPatternElement, ObjectPatternElement, Pattern};
use boa_engine::ast::visitor::{VisitWith, Visitor};
use boa_engine::interner::{Interner, Sym};
use boa_engine::parser::{Parser, Source};

use crate::{
    engine::{JsEngine, JsEvalResult, JsExchange},
    error::JsLanguageError,
};

/// Convert a [`JsLanguageError`] into a fully redacted [`LanguageError`].
///
/// This is the only bridge from engine diagnostics to the language contract,
/// and therefore the redaction boundary: Boa 0.22 exposes no public
/// line/column accessor on `JsError`, and the engine `Display` text may embed
/// exchange data (a `throw new Error('LEAKED-' + camel.body)` message). No
/// engine message text is ever forwarded; classes and static details only.
fn js_err_to_lang_err(_source: &str, e: JsLanguageError) -> LanguageError {
    match e {
        // Compile-time syntax errors surface through `validate_to_parse_error`
        // as `ParseError`; an eval-time parse classification keeps class
        // `parse` without carrying engine text.
        JsLanguageError::Parse { .. } => LanguageError::EvalFailure {
            class: ExpressionErrorClass::Parse,
            position: None,
            detail: None,
        },
        JsLanguageError::Timeout => LanguageError::EvalFailure {
            class: ExpressionErrorClass::Timeout,
            position: None,
            detail: None,
        },
        // Native `TypeError` from the engine, classified by Boa's structured
        // `JsNativeErrorKind` (never by rendered message).
        JsLanguageError::TypeMismatch => LanguageError::EvalFailure {
            class: ExpressionErrorClass::TypeMismatch,
            position: None,
            detail: None,
        },
        // A configured `RuntimeLimits` quota tripped (loop iteration,
        // recursion, or stack size), classified by Boa's structured
        // `EngineError::RuntimeLimit`.
        JsLanguageError::Limit => LanguageError::EvalFailure {
            class: ExpressionErrorClass::Limit,
            position: None,
            detail: None,
        },
        // Engine conversion refusals (unsafe integers, non-finite floats,
        // symbols/BigInt, depth) — typed and operand-free. `to_expression_failed`
        // rewrites the generic target from trusted `EvalMeta`.
        JsLanguageError::TypeConversion { .. } => LanguageError::ConversionError {
            source_type: "js value".to_string(),
            target: "value".to_string(),
        },
        // Runtime scripting and exchange-access errors: class only. The
        // engine message is discarded here, never rendered.
        JsLanguageError::Execution { .. } | JsLanguageError::ExchangeAccess { .. } => {
            LanguageError::EvalFailure {
                class: ExpressionErrorClass::Runtime,
                position: None,
                detail: None,
            }
        }
        // A read-only evaluation refused an exchange mutation the compile-time
        // walk could not decide. The detail is a static operator hint (it
        // names no exchange data) and points at `script:`; the class is
        // `runtime` because the refusal happens during evaluation.
        JsLanguageError::ReadOnlyMutation => LanguageError::EvalFailure {
            class: ExpressionErrorClass::Runtime,
            position: None,
            detail: Some(READ_ONLY_MUTATION_REASON.to_string()),
        },
        // The configured engine does not implement read-only evaluation. The
        // default `JsEngine::eval_read_only` fails closed with this variant
        // instead of silently running the writable path. The detail is a
        // static hint (it names no exchange data) and points at `script:`.
        JsLanguageError::ReadOnlyUnsupported => LanguageError::EvalFailure {
            class: ExpressionErrorClass::Runtime,
            position: None,
            detail: Some(READ_ONLY_UNSUPPORTED_REASON.to_string()),
        },
    }
}

/// Validate `script` and convert any error to a `LanguageError::ParseError`.
pub(crate) fn validate_to_parse_error(
    engine: &Arc<dyn JsEngine>,
    script: &str,
) -> Result<(), LanguageError> {
    engine
        .validate(script)
        .map_err(|e| LanguageError::ParseError {
            expr: script.to_string(),
            reason: e.to_string(),
        })
}

/// Reason returned when a read-only expression attempts an Exchange mutation.
const READ_ONLY_MUTATION_REASON: &str =
    "exchange mutation is not allowed in a read-only expression; use a script: step instead";

/// Reason returned when the configured engine does not implement read-only
/// evaluation (the fail-closed default of [`JsEngine::eval_read_only`]).
const READ_ONLY_UNSUPPORTED_REASON: &str =
    "read-only evaluation is not supported by the configured JS engine; use a script: step instead";

/// Reject Exchange mutations in read-only JS expression/predicate sources
/// (change `language-value-boundary`, B4).
///
/// JS mutations go through the `camel.*` surface (`camel.headers.set(...)`,
/// `camel.properties.set(...)`, `camel.set_property(...)`, and assignment to
/// `camel.body`/`camel.headers`/`camel.properties`). Static computed member
/// names (`camel["set_property"](...)`, `camel.headers["set"](...)`,
/// `camel["body"] = ...`) are resolved to the same surface, and the mutating
/// update (`camel.body++`) and `delete camel.body` nodes are recognized too.
/// The crate's validate hook returns unit and cannot see them, so the raw
/// source is parsed with Boa's public parser and walked for those shapes.
///
/// Tolerant by design: a source the parser cannot parse is reported by
/// `validate` already, so this walk returns `Ok(())` and lets the normal
/// syntax-error path surface. `camel.*` reads (`camel.body`, `camel.headers.get`,
/// ...) are legitimate and never rejected.
///
/// Destructuring-pattern assignment targets (`[camel.body] = [...]`,
/// `({ body: camel.body } = ...)`) are resolved by this walk and rejected at
/// compile time like their direct-assignment twins.
///
/// Dynamic dispatch (`camel[expr]`, alias capture such as
/// `const m = camel.headers.set; m(...)`, or host-style
/// `Object.defineProperty(camel, ...)`) is NOT decidable statically. Those
/// writes are refused at runtime by the read-only snapshot's throwing traps
/// (see `readonly.rs`); they are never silently discarded.
pub(crate) fn reject_read_only_mutation(source: &str) -> Result<(), LanguageError> {
    let mut interner = Interner::new();
    let camel = interner.get_or_intern("camel");
    let headers = interner.get_or_intern("headers");
    let properties = interner.get_or_intern("properties");
    let set = interner.get_or_intern("set");
    let remove = interner.get_or_intern("remove");
    let set_property = interner.get_or_intern("set_property");
    let set_header = interner.get_or_intern("set_header");

    let scope = boa_engine::ast::scope::Scope::new_global();
    let mut parser = Parser::new(Source::from_bytes(source.as_bytes()));
    // Parse failures are diagnosed by `validate_to_parse_error`; skip the walk.
    let Ok(script) = parser.parse_script(&scope, &mut interner) else {
        return Ok(());
    };

    let mut walker = ReadOnlyMutationWalker {
        camel,
        headers,
        properties,
        set,
        remove,
        set_property,
        set_header,
        found: false,
    };
    let _ = script.visit_with(&mut walker);

    if walker.found {
        Err(LanguageError::ParseError {
            expr: source.to_string(),
            reason: READ_ONLY_MUTATION_REASON.to_string(),
        })
    } else {
        Ok(())
    }
}

/// AST walker finding the first Exchange-mutating call or assignment rooted at
/// the `camel` binding.
struct ReadOnlyMutationWalker {
    camel: Sym,
    headers: Sym,
    properties: Sym,
    set: Sym,
    remove: Sym,
    set_property: Sym,
    set_header: Sym,
    found: bool,
}

/// Flatten a `x.y.z` member chain into its root identifier and field symbols
/// (innermost-first order). Returns `None` for dynamic or non-property chains.
fn member_chain(expr: &BoaExpression, fields: &mut Vec<Sym>) -> Option<Sym> {
    match expr {
        BoaExpression::Identifier(id) => Some(id.sym()),
        BoaExpression::PropertyAccess(PropertyAccess::Simple(access)) => {
            member_chain_access(access, fields)
        }
        _ => None,
    }
}

/// Static property name of a `x.y` field or a `x["y"]`/`x['y']` computed field
/// whose index is a string literal.
///
/// The engine cannot distinguish a dynamic lookup such as `x[key]` from a
/// static one at compile time, so only literal string indexes are resolved
/// here; everything else (`x[0]`, `x[key]`, a template literal) stays dynamic
/// and is left to the runtime.
fn static_field(field: &PropertyAccessField) -> Option<Sym> {
    match field {
        PropertyAccessField::Const(id) => Some(id.sym()),
        PropertyAccessField::Expr(expr) => match expr.as_ref() {
            BoaExpression::Literal(lit) => lit.as_string(),
            _ => None,
        },
    }
}

fn member_chain_access(access: &SimplePropertyAccess, fields: &mut Vec<Sym>) -> Option<Sym> {
    let root = member_chain(access.target(), fields)?;
    let field = static_field(access.field())?;
    fields.push(field);
    Some(root)
}

impl ReadOnlyMutationWalker {
    /// True when `access` is a member chain rooted at the `camel` binding.
    fn access_targets_camel(&self, access: &PropertyAccess) -> bool {
        match access {
            PropertyAccess::Simple(access) => {
                let mut fields = Vec::new();
                member_chain_access(access, &mut fields) == Some(self.camel)
            }
            // Private (`x.#p`) and `super.x` accesses never root at `camel`.
            _ => false,
        }
    }

    fn call_is_mutation(&self, call: &Call) -> bool {
        let mut fields = Vec::new();
        let Some(root) = member_chain(call.function(), &mut fields) else {
            return false;
        };
        if root != self.camel {
            return false;
        }
        match fields.as_slice() {
            [name] => *name == self.set_property || *name == self.set_header,
            [namespace, method] => {
                (*namespace == self.headers || *namespace == self.properties)
                    && (*method == self.set || *method == self.remove)
            }
            _ => false,
        }
    }

    fn assign_targets_camel(&self, target: &AssignTarget) -> bool {
        match target {
            AssignTarget::Access(access) => self.access_targets_camel(access),
            // `[camel.body] = [...]` / `({ body: camel.body } = ...)`: the
            // pattern's element accesses are assignment targets too.
            AssignTarget::Pattern(pattern) => self.pattern_targets_camel(pattern),
            AssignTarget::Identifier(_) => false,
        }
    }

    /// True when any assignment-target access inside a destructuring pattern
    /// roots at `camel`. Default initializers are expressions, not targets,
    /// and are deliberately not inspected.
    fn pattern_targets_camel(&self, pattern: &Pattern) -> bool {
        match pattern {
            Pattern::Array(array) => array
                .bindings()
                .iter()
                .any(|element| self.array_element_targets_camel(element)),
            Pattern::Object(object) => object
                .bindings()
                .iter()
                .any(|element| self.object_element_targets_camel(element)),
        }
    }

    fn array_element_targets_camel(&self, element: &ArrayPatternElement) -> bool {
        match element {
            ArrayPatternElement::PropertyAccess { access, .. } => self.access_targets_camel(access),
            ArrayPatternElement::Pattern { pattern, .. } => self.pattern_targets_camel(pattern),
            ArrayPatternElement::PropertyAccessRest { access } => self.access_targets_camel(access),
            ArrayPatternElement::PatternRest { pattern } => self.pattern_targets_camel(pattern),
            ArrayPatternElement::Elision
            | ArrayPatternElement::SingleName { .. }
            | ArrayPatternElement::SingleNameRest { .. } => false,
        }
    }

    fn object_element_targets_camel(&self, element: &ObjectPatternElement) -> bool {
        match element {
            ObjectPatternElement::AssignmentPropertyAccess { access, .. } => {
                self.access_targets_camel(access)
            }
            ObjectPatternElement::AssignmentRestPropertyAccess { access } => {
                self.access_targets_camel(access)
            }
            ObjectPatternElement::Pattern { pattern, .. } => self.pattern_targets_camel(pattern),
            ObjectPatternElement::SingleName { .. } | ObjectPatternElement::RestProperty { .. } => {
                false
            }
        }
    }

    /// `camel.x++` / `++camel.x` mutates the exchange surface.
    fn update_targets_camel(&self, target: &UpdateTarget) -> bool {
        match target {
            UpdateTarget::PropertyAccess(access) => self.access_targets_camel(access),
            UpdateTarget::Identifier(_) => false,
        }
    }

    /// `delete camel.x` removes an exchange-surface member.
    fn unary_is_delete_of_camel(&self, unary: &Unary) -> bool {
        if unary.op() != UnaryOp::Delete {
            return false;
        }
        match unary.target() {
            BoaExpression::PropertyAccess(access) => self.access_targets_camel(access),
            _ => false,
        }
    }
}

impl<'ast> Visitor<'ast> for ReadOnlyMutationWalker {
    type BreakTy = ();

    fn visit_call(&mut self, node: &'ast Call) -> ControlFlow<()> {
        if self.call_is_mutation(node) {
            self.found = true;
            return ControlFlow::Break(());
        }
        node.visit_with(self)
    }

    fn visit_assign(&mut self, node: &'ast Assign) -> ControlFlow<()> {
        if self.assign_targets_camel(node.lhs()) {
            self.found = true;
            return ControlFlow::Break(());
        }
        node.visit_with(self)
    }

    fn visit_update(&mut self, node: &'ast Update) -> ControlFlow<()> {
        if self.update_targets_camel(node.target()) {
            self.found = true;
            return ControlFlow::Break(());
        }
        node.visit_with(self)
    }

    fn visit_unary(&mut self, node: &'ast Unary) -> ControlFlow<()> {
        if self.unary_is_delete_of_camel(node) {
            self.found = true;
            return ControlFlow::Break(());
        }
        node.visit_with(self)
    }
}

/// Build a [`JsExchange`] snapshot from an [`Exchange`] reference.
fn snapshot_exchange(exchange: &Exchange) -> Result<JsExchange, LanguageError> {
    let body = match &exchange.input.body {
        Body::Json(v) => v.clone(),
        Body::Text(s) | Body::Xml(s) => Value::String(s.clone()),
        Body::Stream(_) => {
            return Err(LanguageError::EvalError(
                "Body::Stream cannot be used in JS — add 'stream_cache' or 'convert_body_to' before this step".to_string(),
            ));
        }
        // Empty, Bytes, and any future variant expose no value to JS → null.
        _ => Value::Null,
    };

    Ok(JsExchange::from_headers_body_properties(
        exchange.input.headers.clone(),
        body,
        exchange.properties.clone(),
    ))
}

/// Apply [`JsEvalResult`] mutations back onto a mutable [`Exchange`].
fn apply_result_to_exchange(result: &JsEvalResult, exchange: &mut Exchange) {
    exchange.input.headers = result.headers.clone();
    exchange.properties = result.properties.clone();

    let original_was_representable = matches!(
        &exchange.input.body,
        Body::Json(_) | Body::Text(_) | Body::Xml(_)
    );
    if result.body != Value::Null || original_was_representable {
        exchange.input.body = value_to_body(&result.body);
    }
}

/// Convert a `Value` to a `Body`.
fn value_to_body(v: &Value) -> Body {
    match v {
        Value::String(s) => Body::from(s.as_str()),
        other => Body::from(other.clone()),
    }
}

/// A non-mutating JS expression.
///
/// Evaluates the script and returns its result. No mutations are applied to the exchange.
pub struct JsExpression {
    script: String,
    engine: Arc<dyn JsEngine>,
    execution_timeout_ms: u64,
}

impl JsExpression {
    pub fn new(script: String, engine: Arc<dyn JsEngine>, execution_timeout_ms: u64) -> Self {
        Self {
            script,
            engine,
            execution_timeout_ms,
        }
    }
}

#[async_trait]
impl Expression for JsExpression {
    async fn evaluate(&self, exchange: &Exchange) -> Result<Value, LanguageError> {
        let js_exchange = snapshot_exchange(exchange)?;
        let result = eval_async(
            Arc::clone(&self.engine),
            self.script.clone(),
            js_exchange,
            self.execution_timeout_ms,
            true,
        )
        .await
        .map_err(|e| js_err_to_lang_err(&self.script, e))?;
        Ok(result.return_value)
    }
}

/// A mutating JS expression.
///
/// Evaluates the script and propagates any mutations to `headers`, `properties`,
/// and `body` back to the exchange. On failure, changes are rolled back atomically.
pub struct JsMutatingExpression {
    script: String,
    engine: Arc<dyn JsEngine>,
    execution_timeout_ms: u64,
}

impl JsMutatingExpression {
    pub fn new(script: String, engine: Arc<dyn JsEngine>, execution_timeout_ms: u64) -> Self {
        Self {
            script,
            engine,
            execution_timeout_ms,
        }
    }
}

#[async_trait]
impl MutatingExpression for JsMutatingExpression {
    async fn evaluate(&self, exchange: &mut Exchange) -> Result<Value, LanguageError> {
        let js_exchange = snapshot_exchange(exchange)?;
        let original_headers = exchange.input.headers.clone();
        let original_properties = exchange.properties.clone();
        let original_body = exchange.input.body.clone();

        match eval_async(
            Arc::clone(&self.engine),
            self.script.clone(),
            js_exchange,
            self.execution_timeout_ms,
            false,
        )
        .await
        {
            Ok(result) => {
                apply_result_to_exchange(&result, exchange);
                Ok(result.return_value)
            }
            Err(e) => {
                exchange.input.headers = original_headers;
                exchange.properties = original_properties;
                exchange.input.body = original_body;
                Err(js_err_to_lang_err(&self.script, e))
            }
        }
    }
}

/// A JS predicate.
///
/// Evaluates the script and interprets the return value as a boolean.
pub struct JsPredicate {
    script: String,
    engine: Arc<dyn JsEngine>,
    execution_timeout_ms: u64,
}

impl JsPredicate {
    pub fn new(script: String, engine: Arc<dyn JsEngine>, execution_timeout_ms: u64) -> Self {
        Self {
            script,
            engine,
            execution_timeout_ms,
        }
    }
}

#[async_trait]
impl Predicate for JsPredicate {
    async fn matches(&self, exchange: &Exchange) -> Result<bool, LanguageError> {
        let js_exchange = snapshot_exchange(exchange)?;
        let result = eval_async(
            Arc::clone(&self.engine),
            self.script.clone(),
            js_exchange,
            self.execution_timeout_ms,
            true,
        )
        .await
        .map_err(|e| js_err_to_lang_err(&self.script, e))?;
        // Strict bool: predicates must return a real boolean. No JS
        // truthiness coercion — any other value is a type error (change
        // `language-value-boundary`, sealed Q3).
        match &result.return_value {
            Value::Bool(b) => Ok(*b),
            other => Err(LanguageError::TypeMismatch {
                expected: "bool".to_string(),
                actual: value_type_name(other).to_string(),
                position: None,
            }),
        }
    }
}

/// Evaluate a JS expression asynchronously with a timeout.
///
/// Uses `tokio::task::spawn_blocking` to run the synchronous JS engine on a
/// blocking thread, and `tokio::time::timeout` to enforce the execution deadline.
async fn eval_async(
    engine: Arc<dyn JsEngine>,
    script: String,
    exchange: JsExchange,
    execution_timeout_ms: u64,
    read_only: bool,
) -> Result<JsEvalResult, JsLanguageError> {
    let timeout = Duration::from_millis(execution_timeout_ms);
    tokio::time::timeout(timeout, async {
        let join = tokio::task::spawn_blocking(move || {
            if read_only {
                engine.eval_read_only(&script, exchange)
            } else {
                engine.eval(&script, exchange)
            }
        });
        join.await.map_err(|e| JsLanguageError::Execution {
            message: format!("JS execution task join error: {e}"),
        })?
    })
    .await
    .map_err(|_| JsLanguageError::Timeout)?
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

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Instant};

    use camel_language_api::{JsLimitsConfig, Language, Message};
    use serde_json::json;

    use super::*;
    use crate::engines::boa::BoaEngine;
    use crate::error::JsLanguageError;

    fn make_exchange() -> Exchange {
        let mut msg = Message::default();
        msg.headers.insert("foo".to_string(), json!("bar"));
        msg.body = Body::Text("hello".to_string());
        let mut ex = Exchange::new(msg);
        ex.properties.insert("key".to_string(), json!("val"));
        ex
    }

    fn engine() -> Arc<dyn JsEngine> {
        Arc::new(BoaEngine::default())
    }

    #[derive(Debug)]
    struct SlowEngine;

    impl JsEngine for SlowEngine {
        fn eval(
            &self,
            _source: &str,
            exchange: JsExchange,
        ) -> Result<JsEvalResult, JsLanguageError> {
            std::thread::sleep(Duration::from_millis(500));
            Ok(JsEvalResult {
                return_value: json!("done"),
                headers: exchange.headers,
                body: exchange.body,
                properties: exchange.properties,
            })
        }

        fn eval_read_only(
            &self,
            source: &str,
            exchange: JsExchange,
        ) -> Result<JsEvalResult, JsLanguageError> {
            // Opt in to read-only evaluation: this engine is a timing stub,
            // not a real read-only implementation.
            self.eval(source, exchange)
        }

        fn validate(&self, _source: &str) -> Result<(), JsLanguageError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_expression_timeout_returns_error_before_eval_completes() {
        let expr = JsExpression::new("sleep500()".to_string(), Arc::new(SlowEngine), 100);
        let ex = make_exchange();

        let start = Instant::now();
        let result = expr.evaluate(&ex).await;
        let elapsed = start.elapsed();

        match result {
            Err(LanguageError::EvalFailure {
                class: ExpressionErrorClass::Timeout,
                position: None,
                detail: None,
            }) => {}
            other => panic!("expected redacted Timeout EvalFailure, got: {other:?}"),
        }
        assert!(
            elapsed < Duration::from_millis(500),
            "timeout should happen before 500ms, elapsed: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn test_expression_arithmetic() {
        let expr = JsExpression::new("1 + 2".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result.as_i64().unwrap(), 3);
    }

    #[tokio::test]
    async fn test_expression_reads_header() {
        let expr = JsExpression::new("camel.headers.get('foo')".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result.as_str().unwrap(), "bar");
    }

    #[tokio::test]
    async fn test_expression_reads_body() {
        let expr = JsExpression::new("camel.body".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result.as_str().unwrap(), "hello");
    }

    #[tokio::test]
    async fn test_expression_does_not_mutate() {
        // Read-only evaluation refuses the mutation loudly (B4) instead of
        // evaluating against a snapshot and discarding the write.
        let expr = JsExpression::new(
            "camel.headers.set('x', 'y'); camel.body = 'changed'".to_string(),
            engine(),
            5_000,
        );
        let ex = make_exchange();
        let err = expr
            .evaluate(&ex)
            .await
            .expect_err("read-only mutation must be refused");
        assert!(
            matches!(
                err,
                LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Runtime,
                    detail: Some(_),
                    ..
                }
            ),
            "expected typed runtime refusal, got {err:?}"
        );
        assert!(!ex.input.headers.contains_key("x"));
        assert_eq!(ex.input.body.as_text(), Some("hello"));
    }

    #[tokio::test]
    async fn test_mutating_expression_propagates_header() {
        let expr = JsMutatingExpression::new(
            "camel.headers.set('newkey', 'newval'); 'done'".to_string(),
            engine(),
            5_000,
        );
        let mut ex = make_exchange();
        let result = expr.evaluate(&mut ex).await.unwrap();
        assert_eq!(result.as_str().unwrap(), "done");
        assert_eq!(
            ex.input.headers.get("newkey").unwrap().as_str().unwrap(),
            "newval"
        );
    }

    #[tokio::test]
    async fn test_mutating_expression_propagates_property() {
        let expr = JsMutatingExpression::new(
            "camel.set_property('newkey', 'newval'); 'done'".to_string(),
            engine(),
            5_000,
        );
        let mut ex = make_exchange();
        let result = expr.evaluate(&mut ex).await.unwrap();
        assert_eq!(result.as_str().unwrap(), "done");
        assert_eq!(
            ex.properties.get("newkey").unwrap().as_str().unwrap(),
            "newval"
        );
    }

    #[tokio::test]
    async fn test_expression_reads_property_function() {
        let expr = JsExpression::new("camel.property('key')".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result.as_str().unwrap(), "val");
    }

    #[tokio::test]
    async fn test_mutating_expression_propagates_body() {
        let expr = JsMutatingExpression::new(
            "camel.body = 'modified'; camel.body".to_string(),
            engine(),
            5_000,
        );
        let mut ex = make_exchange();
        let _ = expr.evaluate(&mut ex).await.unwrap();
        assert_eq!(ex.input.body.as_text(), Some("modified"));
    }

    #[tokio::test]
    async fn test_mutating_expression_preserves_bytes_body() {
        let expr = JsMutatingExpression::new(
            "camel.headers.set('x', '1'); 'done'".to_string(),
            engine(),
            5_000,
        );
        let mut ex = Exchange::new(Message::default());
        ex.input.body = Body::from(b"binary data".to_vec());

        let _ = expr.evaluate(&mut ex).await.unwrap();

        assert!(
            matches!(ex.input.body, Body::Bytes(_)),
            "Bytes body should be preserved"
        );
    }

    #[tokio::test]
    async fn test_mutating_expression_preserves_empty_body() {
        let expr = JsMutatingExpression::new(
            "camel.headers.set('x', '1'); 'done'".to_string(),
            engine(),
            5_000,
        );
        let mut ex = Exchange::new(Message::default());
        ex.input.body = Body::Empty;

        let _ = expr.evaluate(&mut ex).await.unwrap();

        assert!(
            matches!(ex.input.body, Body::Empty),
            "Empty body should be preserved when JS does not set camel.body"
        );
    }

    #[tokio::test]
    async fn test_mutating_expression_rejects_stream_body() {
        let expr = JsMutatingExpression::new(
            "camel.headers.set('x', '1'); 'done'".to_string(),
            engine(),
            5_000,
        );
        let mut ex = Exchange::new(Message::default());
        ex.input.body = Body::Stream(camel_api::StreamBody {
            stream: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            metadata: camel_api::StreamMetadata::default(),
        });

        let result = expr.evaluate(&mut ex).await;
        assert!(result.is_err(), "JS should reject Body::Stream with error");
    }

    #[tokio::test]
    async fn test_mutating_expression_can_override_bytes_body() {
        let expr = JsMutatingExpression::new(
            "camel.body = 'replaced'; 'done'".to_string(),
            engine(),
            5_000,
        );
        let mut ex = Exchange::new(Message::default());
        ex.input.body = Body::from(b"binary".to_vec());

        let _ = expr.evaluate(&mut ex).await.unwrap();

        assert!(
            !matches!(ex.input.body, Body::Bytes(_)),
            "Body should be updated when JS explicitly sets camel.body"
        );
        assert_eq!(ex.input.body.as_text(), Some("replaced"));
    }

    #[tokio::test]
    async fn test_mutating_expression_atomic_rollback_on_error() {
        let expr = JsMutatingExpression::new(
            "camel.headers.set('x', 'y'); throw new Error('fail')".to_string(),
            engine(),
            5_000,
        );
        let mut ex = make_exchange();
        let result = expr.evaluate(&mut ex).await;
        assert!(result.is_err());
        assert!(!ex.input.headers.contains_key("x"));
        assert_eq!(ex.input.body.as_text(), Some("hello"));
    }

    #[tokio::test]
    async fn test_predicate_true() {
        let pred = JsPredicate::new("true".to_string(), engine(), 5_000);
        let ex = make_exchange();
        assert!(pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn test_predicate_false() {
        let pred = JsPredicate::new("false".to_string(), engine(), 5_000);
        let ex = make_exchange();
        assert!(!pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn test_predicate_header_condition() {
        let pred = JsPredicate::new(
            "camel.headers.get('foo') === 'bar'".to_string(),
            engine(),
            5_000,
        );
        let ex = make_exchange();
        assert!(pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn js_predicate_non_bool_is_type_mismatch() {
        // Sealed Q3: no JS truthiness coercion. `"false"`, `0`, and `[]` are
        // all non-bool and must yield a `TypeMismatch`.
        for (script, actual) in [
            ("\"false\"", "string"),
            ("0", "number"),
            ("[]", "array"),
            ("null", "null"),
        ] {
            let pred = JsPredicate::new(script.to_string(), engine(), 5_000);
            let ex = make_exchange();
            match pred.matches(&ex).await {
                Err(LanguageError::TypeMismatch {
                    expected,
                    actual: got,
                    position: None,
                }) => {
                    assert_eq!(expected, "bool");
                    assert_eq!(got, actual, "script `{script}` type name");
                }
                other => panic!("expected TypeMismatch for `{script}`, got: {other:?}"),
            }
        }

        // Real booleans still work.
        let pred = JsPredicate::new("true".to_string(), engine(), 5_000);
        assert!(pred.matches(&make_exchange()).await.unwrap());
        let pred = JsPredicate::new("false".to_string(), engine(), 5_000);
        assert!(!pred.matches(&make_exchange()).await.unwrap());
    }

    #[tokio::test]
    async fn js_thrown_error_is_class_only() {
        // Repro R5: a thrown message built from exchange data must never
        // reach the error chain. `JsMutatingExpression` is the `script:`
        // path; the read-only predicate/expression path is covered by the
        // compile-time mutation walk.
        let secret = "SECRETVAL";
        let expr = JsMutatingExpression::new(
            "throw new Error('LEAKED-' + camel.body)".to_string(),
            engine(),
            5_000,
        );
        let mut ex = Exchange::new(Message::new(secret));
        let err = expr.evaluate(&mut ex).await.expect_err("must fail");

        let meta = camel_language_api::EvalMeta {
            language: "js".to_string(),
            route_id: "r1".to_string(),
            step_id: "script#0".to_string(),
            verb: "script".to_string(),
            target: None,
        };
        let display = err.to_string();
        let debug = format!("{err:?}");
        let camel_display = camel_language_api::to_expression_failed(err, &meta).to_string();
        let rendered = format!("{display} | {camel_display} | {debug}");
        assert!(!rendered.contains("LEAKED-"), "leak: {rendered}");
        assert!(
            !rendered.contains(secret),
            "exchange data leaked: {rendered}"
        );
        assert!(debug.contains("Runtime"), "expected class Runtime: {debug}");
    }

    #[tokio::test]
    async fn js_conversion_refusal_maps_to_conversion_class() {
        // A header beyond ±2^53 cannot convert into JS; the engine refusal is
        // a typed `conversion` error, not a runtime one.
        let mut ex = Exchange::new(Message::default());
        ex.input
            .headers
            .insert("big".to_string(), json!(9_007_199_254_740_993i64));
        let expr =
            JsMutatingExpression::new("camel.headers.get('big')".to_string(), engine(), 5_000);
        let err = expr.evaluate(&mut ex).await.expect_err("must refuse");
        assert!(
            matches!(err, LanguageError::ConversionError { .. }),
            "expected ConversionError, got: {err:?}"
        );
        assert_eq!(err.class(), Some(ExpressionErrorClass::Conversion));
        // No operand leaks into the rendering.
        assert!(!err.to_string().contains("9007199254740993"));
    }

    #[tokio::test]
    async fn js_revoked_proxy_map_value_is_conversion_and_rolls_back() {
        // A script parks a revoked `Proxy` in a map value and returns `true`.
        // Extracting the map back into the exchange must refuse the value as a
        // typed conversion refusal (class `Conversion`), NOT a runtime /
        // exchange-access failure, and the mutating transaction must roll back
        // with the original exchange intact. Covers the `headers` and
        // `properties` extract_map call sites.
        let secret = "SECRETVAL";
        for map in ["headers", "properties"] {
            let script = format!(
                "const r=Proxy.revocable([],{{}});camel.{map}.set(\"k\",r.proxy);r.revoke();true"
            );
            let expr = JsMutatingExpression::new(script, engine(), 5_000);
            let mut ex = Exchange::new(Message::new(secret));
            ex.input.headers.insert("foo".to_string(), json!("bar"));
            ex.properties.insert("key".to_string(), json!("val"));

            let err = expr
                .evaluate(&mut ex)
                .await
                .expect_err("a revoked proxy map value must refuse conversion");

            assert!(
                matches!(err, LanguageError::ConversionError { .. }),
                "expected ConversionError for {map}, got: {err:?}"
            );
            assert_eq!(
                err.class(),
                Some(ExpressionErrorClass::Conversion),
                "revoked proxy value must classify as conversion, not runtime: {err:?}"
            );
            // Display and Debug must not carry exchange data.
            let rendered = format!("{err} | {err:?}");
            assert!(
                !rendered.contains(secret),
                "exchange data leaked into {map} error: {rendered}"
            );

            // Atomic rollback: header/property/body stay at their originals.
            assert!(
                !ex.input.headers.contains_key("k"),
                "{map}: header rolled back"
            );
            assert_eq!(
                ex.input.headers.get("foo").and_then(|v| v.as_str()),
                Some("bar"),
                "{map}: unrelated header preserved"
            );
            assert!(
                !ex.properties.contains_key("k"),
                "{map}: property rolled back"
            );
            assert_eq!(
                ex.properties.get("key").and_then(|v| v.as_str()),
                Some("val"),
                "{map}: unrelated property preserved"
            );
            assert_eq!(
                ex.input.body.as_text(),
                Some(secret),
                "{map}: body preserved"
            );
        }
    }

    #[tokio::test]
    async fn js_native_type_error_is_typed_and_redacted() {
        // `null.nope` is an engine-owned TypeError (structured
        // `JsNativeErrorKind::Type`), not a user `throw`. It must classify as
        // `type-mismatch` and forward no rendered engine message.
        let secret = "SECRETVAL";
        let mut ex = Exchange::new(Message::new(secret));
        let expr =
            JsMutatingExpression::new("let v = camel.body; null.nope".to_string(), engine(), 5_000);
        let err = expr
            .evaluate(&mut ex)
            .await
            .expect_err("native TypeError must fail");
        assert_eq!(
            err.class(),
            Some(ExpressionErrorClass::TypeMismatch),
            "native TypeError must be typed, got: {err:?}"
        );
        let rendered = format!("{err} | {err:?}");
        assert!(
            !rendered.contains(secret),
            "exchange data leaked: {rendered}"
        );
        assert!(
            !rendered.contains("nope"),
            "engine message leaked into language error: {rendered}"
        );
    }

    #[tokio::test]
    async fn js_native_quota_error_is_limit() {
        // Loop quota — direct and nested inside a function call.
        for source in ["while (true) {}", "(function f() { while (true) {} })()"] {
            let lang = crate::language::JsLanguage::with_limits(JsLimitsConfig {
                max_loop_iterations: Some(1_000),
                ..Default::default()
            });
            let expr = lang
                .create_mutating_expression(source)
                .expect("parse must succeed");
            let mut ex = Exchange::new(Message::new("payload"));
            let err = expr
                .evaluate(&mut ex)
                .await
                .expect_err("loop quota must fail");
            assert_eq!(
                err.class(),
                Some(ExpressionErrorClass::Limit),
                "`{source}` must classify as limit, got: {err:?}"
            );
            assert!(
                !err.to_string().contains("reached the maximum"),
                "engine limit message must not leak: {err}"
            );
        }

        // Recursion quota, nested one frame deep.
        let lang = crate::language::JsLanguage::with_limits(JsLimitsConfig {
            max_recursion_depth: Some(10),
            ..Default::default()
        });
        let expr = lang
            .create_mutating_expression("(function f() { return f(); })()")
            .expect("parse must succeed");
        let mut ex = Exchange::new(Message::new("payload"));
        let err = expr
            .evaluate(&mut ex)
            .await
            .expect_err("recursion quota must fail");
        assert_eq!(
            err.class(),
            Some(ExpressionErrorClass::Limit),
            "recursion quota must classify as limit, got: {err:?}"
        );
    }

    // --- JS-003: Edge case tests ---
    #[tokio::test]
    async fn test_expression_empty_script_returns_undefined() {
        // An empty script should evaluate to undefined (maps to Null)
        let expr = JsExpression::new("".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await.unwrap();
        assert!(
            result.is_null(),
            "empty script should return null/undefined"
        );
    }

    #[tokio::test]
    async fn test_expression_syntax_error_returns_err() {
        let expr = JsExpression::new("let x = {{{".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await;
        assert!(result.is_err(), "syntax error should return Err");
    }

    #[tokio::test]
    async fn test_expression_non_string_return_value() {
        // A script returning a number (not string) should work correctly
        let expr = JsExpression::new("42".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result.as_i64().unwrap(), 42);
    }

    #[tokio::test]
    async fn test_expression_object_return_value() {
        // A script returning an object should produce a JSON object
        let expr = JsExpression::new("({a: 1, b: 'two'})".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await.unwrap();
        assert!(result.is_object(), "object result should be a JSON object");
        let obj = result.as_object().unwrap();
        assert_eq!(obj.get("a").unwrap().as_i64().unwrap(), 1);
        assert_eq!(obj.get("b").unwrap().as_str().unwrap(), "two");
    }

    #[tokio::test]
    async fn test_expression_array_return_value() {
        let expr = JsExpression::new("[1, 2, 3]".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await.unwrap();
        assert!(result.is_array());
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 3);
    }

    #[tokio::test]
    async fn test_expression_boolean_return_value() {
        let expr = JsExpression::new("true".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await.unwrap();
        assert!(result.is_boolean());
        assert!(result.as_bool().unwrap());
    }

    #[tokio::test]
    async fn concurrent_expressions_no_cross_talk() {
        // Two stateful scripts on the one shared worker, evaluated
        // concurrently via `tokio::join!`: each must observe only its own
        // exchange data (worker-thread confinement, fresh camel per eval).
        // These scripts mutate `camel.*`, so they use the mutating path (the
        // read-only walk correctly rejects them on `create_expression`).
        let lang = crate::language::JsLanguage::new();
        let expr_a = lang
            .create_mutating_expression("camel.headers.set('a', '1'); camel.headers.get('a')")
            .unwrap();
        let expr_b = lang
            .create_mutating_expression("camel.headers.set('b', '2'); camel.headers.get('b')")
            .unwrap();

        let mut ex_a = Exchange::new(Message::default());
        let mut ex_b = Exchange::new(Message::default());

        let (ra, rb) = tokio::join!(expr_a.evaluate(&mut ex_a), expr_b.evaluate(&mut ex_b));

        assert_eq!(ra.unwrap().as_str().unwrap(), "1");
        assert_eq!(rb.unwrap().as_str().unwrap(), "2");
    }

    #[tokio::test]
    async fn js_read_only_mutation_is_compile_error() {
        let lang = crate::language::JsLanguage::new();
        for source in [
            "camel.headers.set(\"k\", 1)",
            "camel.properties.set(\"k\", 1)",
            "camel.headers.remove(\"k\")",
            "camel.properties.remove(\"k\")",
            "camel.set_property(\"k\", 1)",
            "camel.set_header(\"k\", 1)",
            "camel.body = 1",
            "if (true) { camel.headers.set(\"k\", 1); }",
            "if (true) { camel.headers.remove(\"k\"); }",
            "if (true) { camel.properties.remove(\"k\"); }",
        ] {
            match lang.create_expression(source) {
                Err(LanguageError::ParseError { reason, .. }) => {
                    assert!(
                        reason.contains("script:"),
                        "reason must point to `script:`: {reason}"
                    );
                }
                Err(other) => panic!("expected ParseError for `{source}`, got: {other:?}"),
                Ok(_) => panic!("expected ParseError for `{source}`, got Ok"),
            }
            // The mutating path skips the walk and still accepts the source.
            assert!(
                lang.create_mutating_expression(source).is_ok(),
                "mutating path must accept `{source}`"
            );
        }

        // Reads and unrelated chains are legitimate read-only sources.
        for source in [
            "camel.body",
            "camel.headers.get(\"k\")",
            "camel.property(\"k\")",
            "other.headers.set(\"k\", 1)",
        ] {
            assert!(
                lang.create_expression(source).is_ok(),
                "`{source}` must not be rejected"
            );
        }
    }

    /// A static computed member name (`camel["set_property"]`,
    /// `camel.headers["set"]`) reaches the same mutation surface as the dot
    /// form and must be compile-rejected on the read-only paths.
    #[tokio::test]
    async fn js_static_computed_mutations_rejected() {
        let lang = crate::language::JsLanguage::new();
        let mutations = [
            "camel[\"set_property\"](\"k\", 1)",
            "camel[\"set_header\"](\"k\", 1)",
            "camel[\"headers\"][\"set\"](\"k\", 1)",
            "camel.headers[\"set\"](\"k\", 1)",
            "camel[\"headers\"].remove(\"k\")",
            "camel.properties[\"remove\"](\"k\")",
            "camel[\"body\"] = 1",
            "camel[\"headers\"] = {}",
        ];
        for source in mutations {
            for (mode, result) in [
                ("expression", lang.create_expression(source).map(|_| ())),
                ("predicate", lang.create_predicate(source).map(|_| ())),
            ] {
                match result {
                    Err(LanguageError::ParseError { reason, .. }) => assert!(
                        reason.contains("script:"),
                        "[{mode}] reason must point to `script:` for `{source}`: {reason}"
                    ),
                    Err(other) => {
                        panic!("[{mode}] expected ParseError for `{source}`, got: {other:?}")
                    }
                    Ok(_) => panic!("[{mode}] expected ParseError for `{source}`, got Ok"),
                }
            }
            assert!(
                lang.create_mutating_expression(source).is_ok(),
                "mutating path must accept `{source}`"
            );
        }

        // Static computed READS stay legitimate on every path.
        for source in [
            "camel[\"headers\"][\"get\"](\"k\")",
            "camel[\"body\"]",
            "other[\"headers\"][\"set\"](\"k\", 1)",
        ] {
            assert!(
                lang.create_expression(source).is_ok(),
                "`{source}` must not be rejected"
            );
            assert!(
                lang.create_predicate(source).is_ok(),
                "`{source}` must not be rejected as a predicate"
            );
        }
    }

    /// Update (`camel.body++`) and delete (`delete camel.body`) nodes mutate
    /// the exchange surface even though they are neither a call nor a plain
    /// assignment.
    #[tokio::test]
    async fn js_update_delete_mutations_rejected() {
        let lang = crate::language::JsLanguage::new();
        let mutations = [
            "camel.body++",
            "++camel.body",
            "camel[\"body\"]++",
            "camel.headers.foo--",
            "delete camel.body",
            "delete camel.headers.foo",
            "delete camel[\"body\"]",
        ];
        for source in mutations {
            for (mode, result) in [
                ("expression", lang.create_expression(source).map(|_| ())),
                ("predicate", lang.create_predicate(source).map(|_| ())),
            ] {
                match result {
                    Err(LanguageError::ParseError { reason, .. }) => assert!(
                        reason.contains("script:"),
                        "[{mode}] reason must point to `script:` for `{source}`: {reason}"
                    ),
                    Err(other) => {
                        panic!("[{mode}] expected ParseError for `{source}`, got: {other:?}")
                    }
                    Ok(_) => panic!("[{mode}] expected ParseError for `{source}`, got Ok"),
                }
            }
            assert!(
                lang.create_mutating_expression(source).is_ok(),
                "mutating path must accept `{source}`"
            );
        }

        // Unrelated update/delete operations are not exchange mutations.
        for source in ["other.body++", "delete other.body", "const x = 1; x"] {
            assert!(
                lang.create_expression(source).is_ok(),
                "`{source}` must not be rejected"
            );
        }
    }

    // --- B4 runtime read-only refusal (undecidable statics) ---

    /// Every mutation the compile-time walk cannot decide must be refused at
    /// runtime with a typed, redacted error — never silently discarded with the
    /// snapshot. Covers dynamic dispatch, alias capture, method destructuring,
    /// compound update, `delete`, `Object.defineProperty`, and `Reflect.set`.
    #[tokio::test]
    async fn js_dynamic_mutations_fail_loudly() {
        let lang = crate::language::JsLanguage::new();
        let cases = [
            "const k=\"body\";camel[k]=1;true",
            "const m=camel.set_property;m(\"k\",1);true",
            "const {set}=camel.headers;set(\"k\",1);true",
            "const c=camel;c.body+=1;true",
            "const c=camel;c.body++",
            "const c=camel;delete c.body",
            "Object.defineProperty(camel,\"body\",{value:1});true",
            "Reflect.set(camel,\"body\",1);true",
        ];

        for source in cases {
            // The dynamic forms are not statically decidable, so they must
            // compile on both read-only paths.
            let expr = lang
                .create_expression(source)
                .unwrap_or_else(|e| panic!("`{source}` must compile: {e:?}"));
            let ex = make_exchange();
            let err = expr
                .evaluate(&ex)
                .await
                .expect_err("dynamic read-only mutation must fail loudly");
            match &err {
                LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Runtime,
                    position: None,
                    detail: Some(detail),
                } => assert!(
                    detail.contains("script:"),
                    "detail must point at `script:`: {detail}"
                ),
                other => panic!("expected typed runtime refusal for `{source}`, got {other:?}"),
            }
            let rendered = format!("{err} | {err:?}");
            assert!(
                !rendered.contains("hello"),
                "refusal leaked exchange body for `{source}`: {rendered}"
            );
            // The exchange snapshot is untouched (nothing was applied).
            assert_eq!(ex.input.body.as_text(), Some("hello"));
            assert!(!ex.input.headers.contains_key("k"));
            assert_eq!(ex.properties.get("key").unwrap().as_str(), Some("val"));

            // Predicate path: same source, same refusal.
            let pred = lang
                .create_predicate(source)
                .unwrap_or_else(|e| panic!("predicate `{source}` must compile: {e:?}"));
            let ex = make_exchange();
            let perr = pred
                .matches(&ex)
                .await
                .expect_err("dynamic read-only mutation must fail the predicate");
            assert!(
                matches!(
                    perr,
                    LanguageError::EvalFailure {
                        class: ExpressionErrorClass::Runtime,
                        position: None,
                        detail: Some(_),
                    }
                ),
                "expected typed runtime refusal for predicate `{source}`, got {perr:?}"
            );
        }
    }

    /// Same-value and write-restore writes are refused too: a refusal cannot
    /// depend on comparing the final snapshot (the write may leave the value
    /// unchanged), and it cannot be repaired by restoring the old value.
    #[tokio::test]
    async fn js_read_only_same_value_and_restore_writes_rejected() {
        let lang = crate::language::JsLanguage::new();
        let cases = [
            // `foo` already equals `bar`; a same-value write must still refuse.
            "const h=camel.headers;h.set(\"foo\",\"bar\");true",
            // Write then restore the previous body value.
            "const c=camel;const old=c.body;c.body=1;c.body=old;true",
        ];
        for source in cases {
            let expr = lang
                .create_expression(source)
                .unwrap_or_else(|e| panic!("`{source}` must compile: {e:?}"));
            let ex = make_exchange();
            let err = expr
                .evaluate(&ex)
                .await
                .expect_err("same-value/restore write must refuse");
            assert!(
                matches!(
                    err,
                    LanguageError::EvalFailure {
                        class: ExpressionErrorClass::Runtime,
                        detail: Some(_),
                        ..
                    }
                ),
                "expected typed runtime refusal for `{source}`, got {err:?}"
            );
            assert_eq!(ex.input.body.as_text(), Some("hello"));
            assert_eq!(ex.input.headers.get("foo").unwrap().as_str(), Some("bar"));
        }
    }

    /// An in-script `try`/`catch` around a dynamic mutation is ordinary script
    /// error handling: the refusal is suppressed and evaluation continues with
    /// the fallback value.
    #[tokio::test]
    async fn js_read_only_caught_mutation_is_suppressed() {
        let lang = crate::language::JsLanguage::new();
        let source = "try { const k=\"body\"; camel[k]=1; false } catch (e) { true }";

        let expr = lang.create_expression(source).expect("must compile");
        let ex = make_exchange();
        let value = expr
            .evaluate(&ex)
            .await
            .expect("caught mutation must be suppressed");
        assert_eq!(value.as_bool(), Some(true));
        assert_eq!(ex.input.body.as_text(), Some("hello"));

        let pred = lang.create_predicate(source).expect("must compile");
        let ex = make_exchange();
        assert!(
            pred.matches(&ex)
                .await
                .expect("caught mutation must be suppressed")
        );
        assert_eq!(ex.input.body.as_text(), Some("hello"));
    }

    /// Independently created local objects stay fully mutable, and ordinary
    /// `camel` reads keep working in read-only mode.
    #[tokio::test]
    async fn js_local_mutation_and_reads_still_work() {
        let lang = crate::language::JsLanguage::new();
        let source = "const local = { a: 1 }; local.a = 2; local.b = 3; \
                      const arr = [1]; arr.push(2); \
                      ({ a: local.a, b: local.b, len: arr.length, \
                         body: camel.body, header: camel.headers.get(\"foo\") })";
        let expr = lang.create_expression(source).expect("must compile");
        let ex = make_exchange();
        let value = expr.evaluate(&ex).await.expect("local mutation must work");
        let obj = value.as_object().expect("object result");
        assert_eq!(obj.get("a").unwrap().as_i64(), Some(2));
        assert_eq!(obj.get("b").unwrap().as_i64(), Some(3));
        assert_eq!(obj.get("len").unwrap().as_i64(), Some(2));
        assert_eq!(obj.get("body").unwrap().as_str(), Some("hello"));
        assert_eq!(obj.get("header").unwrap().as_str(), Some("bar"));
    }

    /// Read-only snapshot objects/arrays convert back to JSON intact (proxy
    /// wrapping must not degrade an array into an object), nested reads work,
    /// and nested writes are refused.
    #[tokio::test]
    async fn js_read_only_body_objects_read_as_objects() {
        let lang = crate::language::JsLanguage::new();
        let mut ex = make_exchange();
        ex.input.body = Body::Json(serde_json::json!({
            "a": [1, 2, { "b": 3 }],
            "s": "x"
        }));

        let expr = lang.create_expression("camel.body").unwrap();
        let value = expr.evaluate(&ex).await.unwrap();
        assert_eq!(value["a"][1].as_i64(), Some(2));
        assert_eq!(value["a"][2]["b"].as_i64(), Some(3));
        assert_eq!(value["s"].as_str(), Some("x"));

        let expr = lang.create_expression("camel.body.a[2].b").unwrap();
        assert_eq!(expr.evaluate(&ex).await.unwrap().as_i64(), Some(3));

        // A nested write is not statically decidable (numeric index) and must
        // be refused at runtime.
        let expr = lang
            .create_expression("camel.body.a[2].b = 9; true")
            .unwrap();
        let err = expr
            .evaluate(&ex)
            .await
            .expect_err("nested write must be refused");
        assert!(
            matches!(
                err,
                LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Runtime,
                    ..
                }
            ),
            "expected typed runtime refusal, got {err:?}"
        );
        // The exchange body is untouched.
        match &ex.input.body {
            Body::Json(v) => assert_eq!(v["a"][2]["b"], 3),
            other => panic!("expected Json body, got {other:?}"),
        }
    }

    /// The mutating path still accepts every dynamic form the read-only path
    /// refuses (the walk is read-only-only; runtime traps are not installed).
    #[tokio::test]
    async fn js_dynamic_mutations_accepted_in_mutating_mode() {
        let lang = crate::language::JsLanguage::new();
        let cases = [
            "const k=\"body\";camel[k]=1;true",
            "const m=camel.set_property;m(\"k\",1);true",
            "const {set}=camel.headers;set(\"k\",1);true",
            "const c=camel;c.body+=1;true",
            "const c=camel;c.body++",
            "const c=camel;delete c.body",
            "Object.defineProperty(camel,\"body\",{value:1});true",
            "Reflect.set(camel,\"body\",1);true",
        ];
        for source in cases {
            assert!(
                lang.create_mutating_expression(source).is_ok(),
                "mutating path must accept `{source}`"
            );
        }
    }

    /// Destructuring assignment targets that name a `camel` member are
    /// statically rejected on both read-only paths.
    #[tokio::test]
    async fn js_read_only_destructuring_is_compile_error() {
        let lang = crate::language::JsLanguage::new();
        let cases = [
            "[camel.body]=[1]",
            "[camel.body, x]=[1, 2]",
            "({ body: camel.body } = {})",
            "[...camel.body] = [1]",
        ];
        for source in cases {
            for (mode, result) in [
                ("expression", lang.create_expression(source).map(|_| ())),
                ("predicate", lang.create_predicate(source).map(|_| ())),
            ] {
                match result {
                    Err(LanguageError::ParseError { reason, .. }) => assert!(
                        reason.contains("script:"),
                        "[{mode}] reason must point at `script:` for `{source}`: {reason}"
                    ),
                    Err(other) => {
                        panic!("[{mode}] expected ParseError for `{source}`, got: {other:?}")
                    }
                    Ok(_) => panic!("[{mode}] expected ParseError for `{source}`, got Ok"),
                }
            }
            assert!(
                lang.create_mutating_expression(source).is_ok(),
                "mutating path must accept `{source}`"
            );
        }

        // Unrelated destructuring (local identifiers) stays legitimate.
        for source in ["[x] = [1]", "({ a } = {})", "[x, y] = [1, 2]"] {
            assert!(
                lang.create_expression(source).is_ok(),
                "`{source}` must not be rejected"
            );
        }
    }

    /// A script that replaces the writable global `Array.isArray` must not
    /// affect conversion of a read-only proxy array: the detector is a
    /// pristine builtin retained privately before the eval. A throwing
    /// override must not leak into the conversion path either.
    #[tokio::test]
    async fn js_proxy_array_shape_survives_is_array_override() {
        let lang = crate::language::JsLanguage::new();
        let mut ex = make_exchange();
        ex.input.body = Body::Json(json!([1, 2]));

        for source in [
            "Array.isArray=()=>false; camel.body",
            "Array.isArray=()=>{throw new Error('LEAKED');}; camel.body",
        ] {
            let expr = lang
                .create_expression(source)
                .unwrap_or_else(|e| panic!("`{source}` must compile: {e:?}"));
            let value = expr
                .evaluate(&ex)
                .await
                .unwrap_or_else(|e| panic!("`{source}` must evaluate: {e:?}"));
            assert_eq!(
                value,
                json!([1, 2]),
                "proxy array must survive an Array.isArray override: `{source}`"
            );
            let rendered = format!("{value:?}");
            assert!(
                !rendered.contains("LEAKED"),
                "override error must not leak into conversion: {rendered}"
            );
        }
    }

    /// Extensibility mutations on a read-only snapshot must fail loudly:
    /// `Object.preventExtensions`, `Object.seal`, `Object.freeze`, and an
    /// aliased `Object.preventExtensions` all hit the proxy's throwing
    /// `preventExtensions` trap. Locals stay freezable and the mutating path
    /// accepts the same calls.
    #[tokio::test]
    async fn js_read_only_extensibility_mutations_fail_loudly() {
        let lang = crate::language::JsLanguage::new();

        // Non-empty nested body for the alias case.
        let mut ex = make_exchange();
        ex.input.body = Body::Json(json!({ "a": [1, 2], "b": { "c": 3 } }));

        let cases = [
            "Reflect.preventExtensions(camel.body); true",
            "Object.preventExtensions(camel.body); true",
            "const p = Object.preventExtensions; p(camel.body); true",
            "Object.freeze(camel.body); true",
            "Object.seal(camel.body); true",
        ];
        for source in cases {
            let expr = lang
                .create_expression(source)
                .unwrap_or_else(|e| panic!("`{source}` must compile: {e:?}"));
            let err = expr
                .evaluate(&ex)
                .await
                .expect_err("extensibility mutation must fail loudly");
            match &err {
                LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Runtime,
                    position: None,
                    detail: Some(detail),
                } => assert!(
                    detail.contains("script:"),
                    "detail must point at `script:`: {detail}"
                ),
                other => panic!("expected typed runtime refusal for `{source}`, got {other:?}"),
            }
            let rendered = format!("{err} | {err:?}");
            assert!(
                !rendered.contains("hello"),
                "refusal leaked exchange body for `{source}`: {rendered}"
            );

            // Predicate path: same source, same refusal.
            let pred = lang
                .create_predicate(source)
                .unwrap_or_else(|e| panic!("predicate `{source}` must compile: {e:?}"));
            let perr = pred
                .matches(&ex)
                .await
                .expect_err("extensibility mutation must fail the predicate");
            assert!(
                matches!(
                    perr,
                    LanguageError::EvalFailure {
                        class: ExpressionErrorClass::Runtime,
                        position: None,
                        detail: Some(_),
                    }
                ),
                "expected typed runtime refusal for predicate `{source}`, got {perr:?}"
            );
        }

        // Empty-body freeze: the snapshot object is still a proxy.
        let mut empty = make_exchange();
        empty.input.body = Body::Json(json!({}));
        let expr = lang
            .create_expression("Object.freeze(camel.body); true")
            .unwrap();
        let err = expr
            .evaluate(&empty)
            .await
            .expect_err("freeze of an empty snapshot must fail loudly");
        assert!(
            matches!(
                err,
                LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Runtime,
                    detail: Some(_),
                    ..
                }
            ),
            "expected typed runtime refusal, got {err:?}"
        );

        // Locals stay freezable in read-only mode.
        let expr = lang
            .create_expression(
                "const local = { a: 1 }; Object.freeze(local); Object.isFrozen(local)",
            )
            .unwrap();
        let value = expr
            .evaluate(&ex)
            .await
            .expect("freezing a local must work");
        assert_eq!(value.as_bool(), Some(true));

        // The mutating path accepts the same extensibility calls.
        for source in cases {
            assert!(
                lang.create_mutating_expression(source).is_ok(),
                "mutating path must accept `{source}`"
            );
        }
    }
}
