use crate::ResolverFn;
use crate::parser::{CallKind, Expr, InterpolatedPart, LogicalOp, Op, PathSegment};
use camel_api::ExpressionErrorClass;
use camel_language_api::{Body, Exchange, LanguageError, Value};
use serde_json::Number;
use std::future::Future;
use std::pin::Pin;

/// One shared work budget per source evaluation.
const MAX_WORK_STEPS: u32 = 4096;
/// One shared output budget per source evaluation.
const MAX_OUTPUT_BYTES: u32 = 1_048_576;

pub fn evaluate<'a>(
    expr: &'a Expr,
    exchange: &'a Exchange,
    resolver: &'a Option<ResolverFn>,
) -> Pin<Box<dyn Future<Output = Result<Value, LanguageError>> + Send + 'a>> {
    // Two distinct futures: the delegate-free future has no await points, so
    // it compiles to a plain coroutine instead of a resumable state machine.
    if contains_delegate(expr) {
        Box::pin(async move { evaluate_with_delegation(expr, exchange, resolver).await })
    } else {
        Box::pin(async move { evaluate_without_delegation(expr, exchange, resolver) })
    }
}

/// Single accounting point for produced output: any string RESULT leaving
/// evaluation (whole-slot native value, delegation, `string()`) is charged
/// once here. Interpolated results are already charged incrementally.
fn charge_native_output(
    expr: &Expr,
    value: &Value,
    ctx: &mut EvalCtx<'_>,
) -> Result<(), LanguageError> {
    if !matches!(expr, Expr::Interpolated(_))
        && let Value::String(rendered) = value
    {
        ctx.charge_output(rendered.len())?;
    }
    Ok(())
}

fn evaluate_without_delegation(
    expr: &Expr,
    exchange: &Exchange,
    resolver: &Option<ResolverFn>,
) -> Result<Value, LanguageError> {
    let mut ctx = EvalCtx {
        exchange,
        resolver,
        work: 0,
        output: 0,
    };
    let value = match eval_leaf(expr, ctx.exchange) {
        Some(value) => {
            ctx.charge_work()?;
            value
        }
        None => eval_sync_expr(expr, &mut ctx)?,
    };
    charge_native_output(expr, &value, &mut ctx)?;
    Ok(value)
}

async fn evaluate_with_delegation(
    expr: &Expr,
    exchange: &Exchange,
    resolver: &Option<ResolverFn>,
) -> Result<Value, LanguageError> {
    let mut ctx = EvalCtx {
        exchange,
        resolver,
        work: 0,
        output: 0,
    };
    let value = eval_async_expr(expr, &mut ctx).await?;
    charge_native_output(expr, &value, &mut ctx)?;
    Ok(value)
}

struct EvalCtx<'a> {
    exchange: &'a Exchange,
    resolver: &'a Option<ResolverFn>,
    work: u32,
    output: u32,
}

impl EvalCtx<'_> {
    fn charge_work(&mut self) -> Result<(), LanguageError> {
        // Bounded: the walk stops as soon as it exceeds MAX_WORK_STEPS.
        self.work += 1;
        if self.work > MAX_WORK_STEPS {
            Err(limit_error("work step limit exceeded"))
        } else {
            Ok(())
        }
    }

    fn charge_output(&mut self, bytes: usize) -> Result<(), LanguageError> {
        let bytes = bytes.min(u32::MAX as usize) as u32;
        self.output = self.output.saturating_add(bytes);
        if self.output > MAX_OUTPUT_BYTES {
            Err(limit_error("output size limit exceeded"))
        } else {
            Ok(())
        }
    }
}

fn limit_error(detail: &str) -> LanguageError {
    LanguageError::EvalFailure {
        class: ExpressionErrorClass::Limit,
        position: None,
        detail: Some(detail.to_string()),
    }
}

fn arithmetic_error(detail: &str) -> LanguageError {
    LanguageError::EvalFailure {
        class: ExpressionErrorClass::Arithmetic,
        position: None,
        detail: Some(detail.to_string()),
    }
}

fn type_error(expected: &str, actual: &str) -> LanguageError {
    LanguageError::TypeMismatch {
        expected: expected.to_string(),
        actual: actual.to_string(),
        position: None,
    }
}

fn delegate_failure(language: &str) -> LanguageError {
    LanguageError::EvalFailure {
        class: ExpressionErrorClass::Runtime,
        position: None,
        detail: Some(format!("delegate language `{language}` failed")),
    }
}

fn conversion_error(source_type: &str, target: &str) -> LanguageError {
    LanguageError::ConversionError {
        source_type: source_type.to_string(),
        target: target.to_string(),
    }
}

/// Evaluate atom/leaf variants. The caller is responsible for charging one
/// work unit. Returns `None` for compound nodes.
#[inline]
fn eval_leaf(expr: &Expr, exchange: &Exchange) -> Option<Value> {
    let value = match expr {
        Expr::Header(key) => exchange.input.header(key).cloned().unwrap_or(Value::Null),
        Expr::Body => match &exchange.input.body {
            Body::Text(s) => Value::String(s.clone()),
            Body::Bytes(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
            Body::Json(v) => Value::String(v.to_string()),
            Body::Xml(s) => Value::String(s.clone()),
            _ => Value::Null,
        },
        Expr::BodyField(segments) => {
            let Body::Json(root) = &exchange.input.body else {
                return Some(Value::Null);
            };
            let mut current: &serde_json::Value = root;
            for seg in segments {
                let next = match seg {
                    PathSegment::Key(k) => current.get(k.as_str()),
                    PathSegment::Index(i) => current.get(*i),
                    _ => None,
                };
                match next {
                    Some(v) => current = v,
                    None => return Some(Value::Null),
                }
            }
            current.clone()
        }
        Expr::ExchangeProperty(key) => exchange.property(key).cloned().unwrap_or(Value::Null),
        Expr::ExceptionMessage => match &exchange.error {
            Some(err) => Value::String(err.to_string()),
            None => Value::Null,
        },
        Expr::StringLit(s) => Value::String(s.clone()),
        Expr::EscapedString(s) => Value::String(s.clone()),
        Expr::IntLit(value) => Value::Number((*value).into()),
        Expr::Null => Value::Null,
        Expr::Bool(b) => Value::Bool(*b),
        _ => return None,
    };
    Some(value)
}

/// True when the AST contains a language delegation. Only those sources need
/// the boxed async walk; everything else is evaluated synchronously.
#[inline]
fn contains_delegate(expr: &Expr) -> bool {
    match expr {
        Expr::LanguageDelegate { .. } => true,
        Expr::BinOp { left, right, .. } | Expr::LogicalOp { left, right, .. } => {
            contains_delegate(left) || contains_delegate(right)
        }
        Expr::Neg(inner) => contains_delegate(inner),
        Expr::Call { args, .. } => args.iter().any(contains_delegate),
        Expr::Interpolated(parts) => parts.iter().any(|part| match part {
            InterpolatedPart::Literal(_) => false,
            InterpolatedPart::Expr(sub) => contains_delegate(sub),
        }),
        _ => false,
    }
}

/// Delegate-free synchronous evaluator. Mirrors [`eval_async_expr`] exactly;
/// the `LanguageDelegate` arm is unreachable because callers route such ASTs
/// through the async path.
#[inline]
fn eval_sync_expr(expr: &Expr, ctx: &mut EvalCtx<'_>) -> Result<Value, LanguageError> {
    ctx.charge_work()?;
    if let Some(value) = eval_leaf(expr, ctx.exchange) {
        return Ok(value);
    }
    match expr {
        Expr::LanguageDelegate { .. } => Err(LanguageError::EvalError(
            "language delegation requires the async evaluator".into(),
        )),

        Expr::BinOp { left, op, right } => {
            let lv = eval_sync_expr(left, ctx)?;
            let rv = eval_sync_expr(right, ctx)?;
            apply_binop(&lv, op, &rv)
        }

        Expr::Interpolated(parts) => {
            let mut result = String::new();
            for part in parts {
                match part {
                    InterpolatedPart::Literal(text) => {
                        ctx.charge_output(text.len())?;
                        result.push_str(text);
                    }
                    InterpolatedPart::Expr(sub_expr) => {
                        ctx.charge_work()?;
                        let val = eval_sync_expr(sub_expr, ctx)?;
                        let rendered = match val {
                            Value::Null => String::new(),
                            Value::String(s) => s,
                            other => other.to_string(),
                        };
                        ctx.charge_output(rendered.len())?;
                        result.push_str(&rendered);
                    }
                }
            }
            Ok(Value::String(result))
        }

        Expr::LogicalOp { left, op, right } => {
            let lv = eval_sync_expr(left, ctx)?;
            match op {
                LogicalOp::And => {
                    if !is_truthy(&lv) {
                        return Ok(Value::Bool(false));
                    }
                    let rv = eval_sync_expr(right, ctx)?;
                    Ok(Value::Bool(is_truthy(&rv)))
                }
                LogicalOp::Or => {
                    if is_truthy(&lv) {
                        return Ok(Value::Bool(true));
                    }
                    let rv = eval_sync_expr(right, ctx)?;
                    Ok(Value::Bool(is_truthy(&rv)))
                }
            }
        }

        Expr::Neg(inner) => {
            let value = eval_sync_expr(inner, ctx)?;
            let operand = to_i64(&value)?;
            let negated = operand
                .checked_neg()
                .ok_or_else(|| arithmetic_error("i64 overflow"))?;
            Ok(Value::Number(negated.into()))
        }

        Expr::Call { kind, args } => match kind {
            CallKind::Int => {
                let value = eval_sync_expr(call_arg(args, 0)?, ctx)?;
                Ok(Value::Number(to_i64_conversion(&value)?.into()))
            }
            CallKind::String => {
                let value = eval_sync_expr(call_arg(args, 0)?, ctx)?;
                Ok(Value::String(string_value(&value)?))
            }
            CallKind::Default => {
                let first = eval_sync_expr(call_arg(args, 0)?, ctx)?;
                if is_missing(&first) {
                    eval_sync_expr(call_arg(args, 1)?, ctx)
                } else {
                    Ok(first)
                }
            }
            CallKind::Min | CallKind::Max => {
                let a = eval_sync_expr(call_arg(args, 0)?, ctx)?;
                let b = eval_sync_expr(call_arg(args, 1)?, ctx)?;
                let x = to_i64(&a)?;
                let y = to_i64(&b)?;
                let selected = if matches!(kind, CallKind::Min) {
                    i64::min(x, y)
                } else {
                    i64::max(x, y)
                };
                Ok(Value::Number(selected.into()))
            }
        },

        // Leaves were handled by `eval_leaf` before the match.
        _ => Ok(eval_leaf(expr, ctx.exchange).unwrap_or(Value::Null)),
    }
}

fn eval_async_expr<'e, 'c>(
    expr: &'e Expr,
    ctx: &'c mut EvalCtx<'_>,
) -> Pin<Box<dyn Future<Output = Result<Value, LanguageError>> + Send + 'c>>
where
    'e: 'c,
{
    Box::pin(async move {
        ctx.charge_work()?;
        match expr {
            Expr::Header(key) => Ok(ctx
                .exchange
                .input
                .header(key)
                .cloned()
                .unwrap_or(Value::Null)),

            Expr::Body => match &ctx.exchange.input.body {
                Body::Text(s) => Ok(Value::String(s.clone())),
                Body::Bytes(b) => Ok(Value::String(String::from_utf8_lossy(b).into_owned())),
                Body::Json(v) => Ok(Value::String(v.to_string())),
                Body::Xml(s) => Ok(Value::String(s.clone())),
                // Empty, Stream, and any future variant render as JSON null.
                _ => Ok(Value::Null),
            },

            Expr::BodyField(segments) => {
                if let Body::Json(root) = &ctx.exchange.input.body {
                    let mut current: &serde_json::Value = root;
                    for seg in segments {
                        let next = match seg {
                            PathSegment::Key(k) => current.get(k.as_str()),
                            PathSegment::Index(i) => current.get(*i),
                            // Future PathSegment variants can't index JSON → absent.
                            _ => None,
                        };
                        match next {
                            Some(v) => current = v,
                            None => return Ok(Value::Null),
                        }
                    }
                    Ok(current.clone())
                } else {
                    Ok(Value::Null)
                }
            }

            Expr::ExchangeProperty(key) => {
                Ok(ctx.exchange.property(key).cloned().unwrap_or(Value::Null))
            }

            Expr::ExceptionMessage => match &ctx.exchange.error {
                Some(err) => Ok(Value::String(err.to_string())),
                None => Ok(Value::Null),
            },

            Expr::LanguageDelegate {
                language,
                expression,
            } => {
                ctx.charge_work()?;
                let resolver = ctx.resolver.as_ref().ok_or_else(|| {
                    LanguageError::EvalError(
                        "No language resolver configured. Register languages via CamelContext to use ${lang:expr} syntax.".into(),
                    )
                })?;
                let lang = resolver(language.as_str()).ok_or_else(|| {
                    LanguageError::EvalError(format!(
                        "Language '{}' not found in registry. Available languages must be registered via CamelContext.",
                        language
                    ))
                })?;
                // Provider errors may embed exchange data. Simple's
                // diagnostics are payload-free, so surface a fixed message
                // naming only the trusted language identifier.
                let expr = lang
                    .create_expression(expression)
                    .map_err(|_| delegate_failure(language))?;
                expr.evaluate(ctx.exchange)
                    .await
                    .map_err(|_| delegate_failure(language))
            }

            Expr::StringLit(s) => Ok(Value::String(s.clone())),
            Expr::EscapedString(s) => Ok(Value::String(s.clone())),
            Expr::IntLit(value) => Ok(Value::Number((*value).into())),
            Expr::Null => Ok(Value::Null),

            Expr::BinOp { left, op, right } => {
                let lv = eval_async_expr(left, &mut *ctx).await?;
                let rv = eval_async_expr(right, &mut *ctx).await?;
                apply_binop(&lv, op, &rv)
            }

            Expr::Interpolated(parts) => {
                let mut result = String::new();
                for part in parts {
                    match part {
                        InterpolatedPart::Literal(text) => {
                            ctx.charge_output(text.len())?;
                            result.push_str(text);
                        }
                        InterpolatedPart::Expr(sub_expr) => {
                            ctx.charge_work()?;
                            let val = eval_async_expr(sub_expr, &mut *ctx).await?;
                            let rendered = match val {
                                Value::Null => String::new(),
                                Value::String(s) => s,
                                other => other.to_string(),
                            };
                            // Fail fast: check the running total before this
                            // piece is appended, so an oversized template never
                            // materializes the accumulated result.
                            ctx.charge_output(rendered.len())?;
                            result.push_str(&rendered);
                        }
                    }
                }
                Ok(Value::String(result))
            }

            Expr::LogicalOp { left, op, right } => {
                let lv = eval_async_expr(left, &mut *ctx).await?;
                match op {
                    LogicalOp::And => {
                        if !is_truthy(&lv) {
                            return Ok(Value::Bool(false));
                        }
                        let rv = eval_async_expr(right, &mut *ctx).await?;
                        Ok(Value::Bool(is_truthy(&rv)))
                    }
                    LogicalOp::Or => {
                        if is_truthy(&lv) {
                            return Ok(Value::Bool(true));
                        }
                        let rv = eval_async_expr(right, &mut *ctx).await?;
                        Ok(Value::Bool(is_truthy(&rv)))
                    }
                }
            }
            Expr::Bool(b) => Ok(Value::Bool(*b)),

            Expr::Neg(inner) => {
                let value = eval_async_expr(inner, &mut *ctx).await?;
                let operand = to_i64(&value)?;
                let negated = operand
                    .checked_neg()
                    .ok_or_else(|| arithmetic_error("i64 overflow"))?;
                Ok(Value::Number(negated.into()))
            }

            Expr::Call { kind, args } => match kind {
                CallKind::Int => {
                    let value = eval_async_expr(call_arg(args, 0)?, &mut *ctx).await?;
                    Ok(Value::Number(to_i64_conversion(&value)?.into()))
                }
                CallKind::String => {
                    let value = eval_async_expr(call_arg(args, 0)?, &mut *ctx).await?;
                    Ok(Value::String(string_value(&value)?))
                }
                CallKind::Default => {
                    let first = eval_async_expr(call_arg(args, 0)?, &mut *ctx).await?;
                    if is_missing(&first) {
                        eval_async_expr(call_arg(args, 1)?, &mut *ctx).await
                    } else {
                        Ok(first)
                    }
                }
                CallKind::Min | CallKind::Max => {
                    let a = eval_async_expr(call_arg(args, 0)?, &mut *ctx).await?;
                    let b = eval_async_expr(call_arg(args, 1)?, &mut *ctx).await?;
                    let x = to_i64(&a)?;
                    let y = to_i64(&b)?;
                    let selected = if matches!(kind, CallKind::Min) {
                        i64::min(x, y)
                    } else {
                        i64::max(x, y)
                    };
                    Ok(Value::Number(selected.into()))
                }
            },
        }
    })
}

fn call_arg(args: &[Expr], index: usize) -> Result<&Expr, LanguageError> {
    args.get(index)
        .ok_or_else(|| LanguageError::EvalError("invalid call arity".into()))
}

fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Number(_) => true,
        _ => true,
    }
}

fn is_missing(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty() || s == "null",
        _ => false,
    }
}

fn string_value(v: &Value) -> Result<String, LanguageError> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(if *b { "true" } else { "false" }.to_string()),
        Value::Null => Ok(String::new()),
        other => Err(conversion_error(value_type_name(other), "string")),
    }
}

fn to_i64_conversion(v: &Value) -> Result<i64, LanguageError> {
    match v {
        Value::Number(n) => number_to_i64(n).ok_or_else(|| conversion_error("number", "i64")),
        Value::String(s) => parse_strict_i64(s).ok_or_else(|| conversion_error("string", "i64")),
        other => Err(conversion_error(value_type_name(other), "i64")),
    }
}

fn to_i64(v: &Value) -> Result<i64, LanguageError> {
    match v {
        Value::Number(n) => number_to_i64(n).ok_or_else(|| type_error("integer", "number")),
        Value::String(s) => parse_strict_i64(s).ok_or_else(|| type_error("integer", "string")),
        other => Err(type_error("integer", value_type_name(other))),
    }
}

/// Exact numeric value for equality. Both i64 and u64 fit i128. Float-valued
/// numbers are exact only when they are integral; non-integral floats never
/// equal an integer (no lossy f64 fallback).
fn number_to_i128(n: &Number) -> Option<i128> {
    if let Some(value) = n.as_i64() {
        return Some(value as i128);
    }
    if let Some(value) = n.as_u64() {
        return Some(value as i128);
    }
    let value = n.as_f64()?;
    if !value.is_finite() || value.fract() != 0.0 {
        return None;
    }
    if value >= i128::MIN as f64 && value <= i128::MAX as f64 {
        Some(value as i128)
    } else {
        None
    }
}

fn number_to_i64(n: &Number) -> Option<i64> {
    if let Some(value) = n.as_i64() {
        return Some(value);
    }
    let value = n.as_u64()?;
    if value <= i64::MAX as u64 {
        Some(value as i64)
    } else {
        None
    }
}

/// Strict integer syntax: `[+-]?[0-9]+`, no whitespace, must fit i64.
fn parse_strict_i64(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    let (negative, digits) = match bytes.first()? {
        b'+' => (false, &s[1..]),
        b'-' => (true, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let magnitude: u128 = digits.parse().ok()?;
    if negative {
        if magnitude == (i64::MAX as u128) + 1 {
            Some(i64::MIN)
        } else if magnitude <= i64::MAX as u128 {
            Some(-(magnitude as i64))
        } else {
            None
        }
    } else if magnitude <= i64::MAX as u128 {
        Some(magnitude as i64)
    } else {
        None
    }
}

fn values_equal_with_coercion(left: &Value, right: &Value) -> bool {
    if left == right {
        return true;
    }
    match (left, right) {
        (Value::String(s), Value::Number(n)) | (Value::Number(n), Value::String(s)) => {
            match (parse_strict_i64(s), number_to_i64(n)) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            }
        }
        (Value::Number(a), Value::Number(b)) => match (number_to_i128(a), number_to_i128(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
        _ => false,
    }
}

fn apply_binop(left: &Value, op: &Op, right: &Value) -> Result<Value, LanguageError> {
    match op {
        Op::Eq => Ok(Value::Bool(values_equal_with_coercion(left, right))),
        Op::Ne => Ok(Value::Bool(!values_equal_with_coercion(left, right))),
        Op::Contains => {
            if matches!(left, Value::Null) || matches!(right, Value::Null) {
                return Ok(Value::Bool(false));
            }
            let ls = left
                .as_str()
                .ok_or_else(|| LanguageError::EvalError("contains requires string left".into()))?;
            let rs = right
                .as_str()
                .ok_or_else(|| LanguageError::EvalError("contains requires string right".into()))?;
            Ok(Value::Bool(ls.contains(rs)))
        }
        Op::Gt | Op::Lt | Op::Gte | Op::Lte => Ok(Value::Bool(ordering_compare(left, op, right)?)),
        Op::Add | Op::Sub | Op::Mul => {
            let a = to_i64(left)?;
            let b = to_i64(right)?;
            let result = match op {
                Op::Add => a.checked_add(b),
                Op::Sub => a.checked_sub(b),
                Op::Mul => a.checked_mul(b),
                _ => None,
            }
            .ok_or_else(|| arithmetic_error("i64 overflow"))?;
            Ok(Value::Number(result.into()))
        }
    }
}

fn ordering_compare(left: &Value, op: &Op, right: &Value) -> Result<bool, LanguageError> {
    use std::cmp::Ordering;

    if matches!(left, Value::Null) || matches!(right, Value::Null) {
        return Ok(false);
    }

    let ordering = match (left, right) {
        (Value::String(a), Value::String(b)) => match (parse_strict_i64(a), parse_strict_i64(b)) {
            (Some(x), Some(y)) => x.cmp(&y),
            _ => a.cmp(b),
        },
        (Value::String(a), Value::Number(n)) => {
            let x = parse_strict_i64(a).ok_or_else(|| type_error("integer", "string"))?;
            let y = number_to_i64(n).ok_or_else(|| type_error("integer", "number"))?;
            x.cmp(&y)
        }
        (Value::Number(n), Value::String(b)) => {
            let x = number_to_i64(n).ok_or_else(|| type_error("integer", "number"))?;
            let y = parse_strict_i64(b).ok_or_else(|| type_error("integer", "string"))?;
            x.cmp(&y)
        }
        (Value::Number(a), Value::Number(b)) => {
            let x = number_to_i64(a).ok_or_else(|| type_error("integer", "number"))?;
            let y = number_to_i64(b).ok_or_else(|| type_error("integer", "number"))?;
            x.cmp(&y)
        }
        (a, b) => {
            let actual = if matches!(a, Value::Bool(_) | Value::Array(_) | Value::Object(_)) {
                value_type_name(a)
            } else {
                value_type_name(b)
            };
            return Err(type_error("integer", actual));
        }
    };

    let result = match op {
        Op::Gt => ordering == Ordering::Greater,
        Op::Lt => ordering == Ordering::Less,
        Op::Gte => ordering != Ordering::Less,
        Op::Lte => ordering != Ordering::Greater,
        _ => false,
    };
    Ok(result)
}

/// Type name of a [`Value`] for `TypeMismatch` diagnostics. Type names only —
/// never runtime values.
pub(crate) fn value_type_name(v: &Value) -> &'static str {
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
    use super::*;
    use camel_api::error::CamelError;
    use camel_language_api::{Expression, Language, Message};
    use serde_json::json;
    use std::sync::Arc;

    struct MockExpression {
        out: Value,
    }

    #[async_trait::async_trait]
    impl Expression for MockExpression {
        async fn evaluate(&self, _exchange: &Exchange) -> Result<Value, LanguageError> {
            Ok(self.out.clone())
        }
    }

    struct MockLanguage;

    impl Language for MockLanguage {
        fn name(&self) -> &'static str {
            "mock"
        }

        fn create_expression(&self, script: &str) -> Result<Box<dyn Expression>, LanguageError> {
            Ok(Box::new(MockExpression {
                out: Value::String(format!("mock:{script}")),
            }))
        }

        fn create_predicate(
            &self,
            _script: &str,
        ) -> Result<Box<dyn camel_language_api::Predicate>, LanguageError> {
            Err(LanguageError::NotSupported {
                feature: "predicate".to_string(),
                language: "mock".to_string(),
            })
        }
    }

    fn exchange_with_body(body: Body) -> Exchange {
        let mut ex = Exchange::default();
        ex.input.body = body;
        ex
    }

    #[tokio::test]
    async fn evaluate_body_variants_and_header_property() {
        let mut msg = Message::new("txt");
        msg.set_header("k", Value::String("v".to_string()));
        let mut ex = Exchange::new(msg);
        ex.set_property("p".to_string(), Value::String("pv".to_string()));

        assert_eq!(
            evaluate(&Expr::Header("k".to_string()), &ex, &None)
                .await
                .unwrap(),
            Value::String("v".to_string())
        );
        assert_eq!(
            evaluate(&Expr::ExchangeProperty("p".to_string()), &ex, &None)
                .await
                .unwrap(),
            Value::String("pv".to_string())
        );
        assert_eq!(
            evaluate(&Expr::Body, &exchange_with_body(Body::Empty), &None)
                .await
                .unwrap(),
            Value::Null
        );
        assert_eq!(
            evaluate(
                &Expr::Body,
                &exchange_with_body(Body::from(b"abc".to_vec())),
                &None
            )
            .await
            .unwrap(),
            Value::String("abc".to_string())
        );
        assert_eq!(
            evaluate(
                &Expr::Body,
                &exchange_with_body(Body::Json(json!({"a": 1}))),
                &None
            )
            .await
            .unwrap(),
            Value::String("{\"a\":1}".to_string())
        );
    }

    #[tokio::test]
    async fn evaluate_body_field_success_and_missing() {
        let ex = exchange_with_body(Body::Json(json!({"users":[{"name":"Ana"}]})));
        let found = evaluate(
            &Expr::BodyField(vec![
                PathSegment::Key("users".to_string()),
                PathSegment::Index(0),
                PathSegment::Key("name".to_string()),
            ]),
            &ex,
            &None,
        )
        .await
        .unwrap();
        assert_eq!(found, json!("Ana"));

        let missing = evaluate(
            &Expr::BodyField(vec![PathSegment::Key("missing".to_string())]),
            &ex,
            &None,
        )
        .await
        .unwrap();
        assert_eq!(missing, Value::Null);
    }

    #[tokio::test]
    async fn evaluate_delegate_and_delegate_errors() {
        let expr = Expr::LanguageDelegate {
            language: "mock".to_string(),
            expression: "x".to_string(),
        };
        let ex = Exchange::default();
        let resolver: Option<ResolverFn> = Some(Arc::new(|n| {
            if n == "mock" {
                Some(Arc::new(MockLanguage) as Arc<dyn Language>)
            } else {
                None
            }
        }));
        assert_eq!(
            evaluate(&expr, &ex, &resolver).await.unwrap(),
            Value::String("mock:x".to_string())
        );
        assert!(evaluate(&expr, &ex, &None).await.is_err());

        let missing_resolver: Option<ResolverFn> = Some(Arc::new(|_| None));
        assert!(evaluate(&expr, &ex, &missing_resolver).await.is_err());
    }

    #[tokio::test]
    async fn evaluate_binop_and_contains_and_type_errors() {
        let ex = Exchange::default();
        assert_eq!(
            evaluate(
                &Expr::BinOp {
                    left: Box::new(Expr::IntLit(7)),
                    op: Op::Gt,
                    right: Box::new(Expr::IntLit(3)),
                },
                &ex,
                &None,
            )
            .await
            .unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            evaluate(
                &Expr::BinOp {
                    left: Box::new(Expr::StringLit("abcdef".to_string())),
                    op: Op::Contains,
                    right: Box::new(Expr::StringLit("bcd".to_string())),
                },
                &ex,
                &None,
            )
            .await
            .unwrap(),
            Value::Bool(true)
        );
        assert!(
            evaluate(
                &Expr::BinOp {
                    left: Box::new(Expr::Bool(true)),
                    op: Op::Contains,
                    right: Box::new(Expr::StringLit("x".to_string())),
                },
                &ex,
                &None,
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn evaluate_interpolated_and_logical_truthiness() {
        let mut msg = Message::new("hello");
        msg.set_header("id", Value::Number(7.into()));
        let ex = Exchange::new(msg);

        let out = evaluate(
            &Expr::Interpolated(vec![
                InterpolatedPart::Literal("id=".to_string()),
                InterpolatedPart::Expr(Box::new(Expr::Header("id".to_string()))),
                InterpolatedPart::Literal(" body=".to_string()),
                InterpolatedPart::Expr(Box::new(Expr::Body)),
                InterpolatedPart::Literal(" miss=".to_string()),
                InterpolatedPart::Expr(Box::new(Expr::Header("missing".to_string()))),
            ]),
            &ex,
            &None,
        )
        .await
        .unwrap();
        assert_eq!(out, Value::String("id=7 body=hello miss=".to_string()));

        let and_short = evaluate(
            &Expr::LogicalOp {
                left: Box::new(Expr::Bool(false)),
                op: LogicalOp::And,
                right: Box::new(Expr::BinOp {
                    left: Box::new(Expr::StringLit("x".to_string())),
                    op: Op::Gt,
                    right: Box::new(Expr::IntLit(1)),
                }),
            },
            &ex,
            &None,
        )
        .await
        .unwrap();
        assert_eq!(and_short, Value::Bool(false));

        let or_short = evaluate(
            &Expr::LogicalOp {
                left: Box::new(Expr::StringLit("non-empty".to_string())),
                op: LogicalOp::Or,
                right: Box::new(Expr::BinOp {
                    left: Box::new(Expr::StringLit("x".to_string())),
                    op: Op::Gt,
                    right: Box::new(Expr::IntLit(1)),
                }),
            },
            &ex,
            &None,
        )
        .await
        .unwrap();
        assert_eq!(or_short, Value::Bool(true));
    }

    #[tokio::test]
    async fn evaluate_exception_message_with_error() {
        let mut ex = Exchange::default();
        ex.set_error(CamelError::RouteError("Unauthorized".to_string()));
        let result = evaluate(&Expr::ExceptionMessage, &ex, &None).await.unwrap();
        assert_eq!(
            result,
            Value::String("Route error: Unauthorized".to_string())
        );
    }

    #[tokio::test]
    async fn evaluate_exception_message_without_error() {
        let ex = Exchange::default();
        let result = evaluate(&Expr::ExceptionMessage, &ex, &None).await.unwrap();
        assert_eq!(result, Value::Null);
    }

    #[tokio::test]
    async fn evaluate_exception_message_interpolation() {
        let mut ex = Exchange::default();
        ex.set_error(CamelError::RouteError("Unauthorized".to_string()));
        let result = evaluate(
            &Expr::Interpolated(vec![
                InterpolatedPart::Literal("Error: ".to_string()),
                InterpolatedPart::Expr(Box::new(Expr::ExceptionMessage)),
            ]),
            &ex,
            &None,
        )
        .await
        .unwrap();
        assert_eq!(
            result,
            Value::String("Error: Route error: Unauthorized".to_string())
        );
    }

    #[tokio::test]
    async fn evaluate_exception_message_interpolation_no_error() {
        let ex = Exchange::default();
        let result = evaluate(
            &Expr::Interpolated(vec![
                InterpolatedPart::Literal("Error: ".to_string()),
                InterpolatedPart::Expr(Box::new(Expr::ExceptionMessage)),
            ]),
            &ex,
            &None,
        )
        .await
        .unwrap();
        assert_eq!(result, Value::String("Error: ".to_string()));
    }

    fn ex_with_headers(pairs: &[(&str, Value)]) -> Exchange {
        let mut msg = Message::default();
        for (key, value) in pairs {
            msg.set_header(key.to_string(), value.clone());
        }
        Exchange::new(msg)
    }

    fn ex_with_props(pairs: &[(&str, Value)]) -> Exchange {
        let mut ex = Exchange::default();
        for (key, value) in pairs {
            ex.set_property(key.to_string(), value.clone());
        }
        ex
    }

    async fn eval(src: &str, ex: &Exchange) -> Result<Value, LanguageError> {
        let ast = crate::parser::parse(src).unwrap();
        evaluate(&ast, ex, &None).await
    }

    fn number(value: i64) -> Value {
        Value::Number(value.into())
    }

    #[tokio::test]
    async fn int_from_numeric_strings() {
        let ex = ex_with_headers(&[("count", Value::String("42".to_string()))]);
        assert_eq!(eval("int(header.count)", &ex).await.unwrap(), number(42));

        let ex = ex_with_props(&[("nowMs", Value::String("1750000000000".to_string()))]);
        assert_eq!(
            eval("int(property.nowMs)", &ex).await.unwrap(),
            number(1_750_000_000_000)
        );
    }

    #[tokio::test]
    async fn int_conversion_errors() {
        let string_cases = ["abc", " 7", "1.5", "18446744073709551615"];
        for value in string_cases {
            let ex = ex_with_headers(&[("v", Value::String(value.to_string()))]);
            let err = eval("int(header.v)", &ex).await.unwrap_err();
            assert!(
                matches!(err, LanguageError::ConversionError { .. }),
                "case {value:?}: {err:?}"
            );
        }

        let ex = ex_with_headers(&[("v", Value::Bool(true))]);
        let err = eval("int(header.v)", &ex).await.unwrap_err();
        assert!(matches!(err, LanguageError::ConversionError { .. }));

        let ex = ex_with_props(&[("u64val", Value::Number(serde_json::Number::from(u64::MAX)))]);
        let err = eval("int(property.u64val)", &ex).await.unwrap_err();
        assert!(matches!(err, LanguageError::ConversionError { .. }));
    }

    #[tokio::test]
    async fn checked_arithmetic_overflow() {
        let ex = ex_with_props(&[("n", Value::String(i64::MAX.to_string()))]);
        let err = eval("int(property.n) + 1", &ex).await.unwrap_err();
        assert!(
            matches!(
                err,
                LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Arithmetic,
                    ..
                }
            ),
            "{err:?}"
        );

        let ex = Exchange::default();
        let err = eval("-9223372036854775808 - 1", &ex).await.unwrap_err();
        assert!(
            matches!(
                err,
                LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Arithmetic,
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn arithmetic_rejects_non_integers() {
        let ex = ex_with_headers(&[("f", Value::String("1.5".to_string()))]);
        let err = eval("header.f * 2", &ex).await.unwrap_err();
        assert!(matches!(err, LanguageError::TypeMismatch { .. }), "{err:?}");

        let ex = ex_with_props(&[("flag", Value::Bool(true))]);
        let err = eval("property.flag + 1", &ex).await.unwrap_err();
        assert!(matches!(err, LanguageError::TypeMismatch { .. }), "{err:?}");

        let ex = ex_with_headers(&[
            ("a", Value::String("10".to_string())),
            ("b", Value::String("9".to_string())),
        ]);
        assert_eq!(eval("header.a + header.b", &ex).await.unwrap(), number(19));
    }

    #[tokio::test]
    async fn string_conversion_rules() {
        let ex = ex_with_headers(&[("h", Value::String("x".to_string())), ("n", number(42))]);
        assert_eq!(
            eval("string(header.h)", &ex).await.unwrap(),
            Value::String("x".to_string())
        );
        assert_eq!(
            eval("string(header.n)", &ex).await.unwrap(),
            Value::String("42".to_string())
        );

        let ex = ex_with_props(&[("flag", Value::Bool(true))]);
        assert_eq!(
            eval("string(property.flag)", &ex).await.unwrap(),
            Value::String("true".to_string())
        );

        let ex = Exchange::default();
        assert_eq!(
            eval("string(header.missing)", &ex).await.unwrap(),
            Value::String(String::new())
        );

        let ex = ex_with_props(&[("arr", Value::Array(vec![json!(1)]))]);
        let err = eval("string(property.arr)", &ex).await.unwrap_err();
        assert!(
            matches!(err, LanguageError::ConversionError { .. }),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn default_lazy_and_missing_rules() {
        let ex = Exchange::default();
        assert_eq!(
            eval("default(property.giveUpAt, '100')", &ex)
                .await
                .unwrap(),
            Value::String("100".to_string())
        );

        for value in ["", "null"] {
            let ex = ex_with_headers(&[("flag", Value::String(value.to_string()))]);
            assert_eq!(
                eval("default(header.flag, 'x')", &ex).await.unwrap(),
                Value::String("x".to_string())
            );
        }

        let ex = ex_with_headers(&[("flag", Value::String("0".to_string()))]);
        assert_eq!(
            eval("default(header.flag, 'x')", &ex).await.unwrap(),
            Value::String("0".to_string())
        );

        let ex = ex_with_headers(&[("flag", Value::Bool(false))]);
        assert_eq!(
            eval("default(header.flag, 'x')", &ex).await.unwrap(),
            Value::Bool(false)
        );

        // Present first operand: the erroring fallback is not evaluated.
        let ex = ex_with_headers(&[
            ("flag", Value::String("present".to_string())),
            ("bad", Value::String("abc".to_string())),
        ]);
        assert_eq!(
            eval("default(header.flag, int(header.bad))", &ex)
                .await
                .unwrap(),
            Value::String("present".to_string())
        );

        // Missing first operand: the fallback is evaluated and fails.
        let ex = ex_with_headers(&[("bad", Value::String("abc".to_string()))]);
        let err = eval("default(header.flag, int(header.bad))", &ex)
            .await
            .unwrap_err();
        assert!(matches!(err, LanguageError::ConversionError { .. }));

        // Erroring first operand propagates.
        let err = eval("default(int(header.bad), 'x')", &ex)
            .await
            .unwrap_err();
        assert!(matches!(err, LanguageError::ConversionError { .. }));
    }

    #[tokio::test]
    async fn min_max_select() {
        let ex = ex_with_headers(&[
            ("base", Value::String("500".to_string())),
            ("cap", Value::String("3000".to_string())),
        ]);
        assert_eq!(
            eval("min(int(header.base) * 2, int(header.cap))", &ex)
                .await
                .unwrap(),
            number(1000)
        );
        assert_eq!(
            eval("max(int(header.base), int(header.cap))", &ex)
                .await
                .unwrap(),
            number(3000)
        );
    }

    #[tokio::test]
    async fn equality_no_throw_type_mismatch() {
        let ex = ex_with_headers(&[("ok", Value::String("true".to_string()))]);
        let mut ex = ex;
        ex.set_property("flag".to_string(), Value::Bool(true));
        assert_eq!(
            eval("property.flag == header.ok", &ex).await.unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            eval("property.flag != header.ok", &ex).await.unwrap(),
            Value::Bool(true)
        );

        let ex = Exchange::default();
        assert_eq!(
            eval("header.a == header.a", &ex).await.unwrap(),
            Value::Bool(true)
        );

        let ex = ex_with_headers(&[("b", Value::String(String::new()))]);
        assert_eq!(
            eval("header.a == header.b", &ex).await.unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            eval("header.a != header.b", &ex).await.unwrap(),
            Value::Bool(true)
        );
    }

    #[tokio::test]
    async fn numeric_string_exact_beyond_f64() {
        let ex = ex_with_headers(&[("big", Value::String("9007199254740993".to_string()))]);
        assert_eq!(
            eval("header.big == 9007199254740993", &ex).await.unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            eval("header.big == 9007199254740992", &ex).await.unwrap(),
            Value::Bool(false)
        );
    }

    #[tokio::test]
    async fn ordering_rules() {
        let ex = Exchange::default();
        assert_eq!(eval("header.n > 1", &ex).await.unwrap(), Value::Bool(false));

        let ex = ex_with_headers(&[
            ("a", Value::String("10".to_string())),
            ("b", Value::String("9".to_string())),
        ]);
        assert_eq!(
            eval("header.a > header.b", &ex).await.unwrap(),
            Value::Bool(true)
        );

        let ex = ex_with_headers(&[
            ("w", Value::String("apple".to_string())),
            ("v", Value::String("9".to_string())),
        ]);
        assert_eq!(
            eval("header.w > header.v", &ex).await.unwrap(),
            Value::Bool(true)
        );

        let ex = ex_with_headers(&[("dec", Value::String("1.5".to_string()))]);
        let err = eval("header.dec > 1", &ex).await.unwrap_err();
        assert!(matches!(err, LanguageError::TypeMismatch { .. }), "{err:?}");

        let ex = ex_with_headers(&[("flag", Value::Bool(true))]);
        let err = eval("header.flag > 0", &ex).await.unwrap_err();
        assert!(matches!(err, LanguageError::TypeMismatch { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn numeric_equality_is_exact_beyond_i64() {
        let big_u64 = Value::Number(serde_json::Number::from(9_223_372_036_854_775_808u64));
        let i64_max = i64::MAX;

        let ex = ex_with_headers(&[("big", big_u64.clone())]);
        assert_eq!(
            eval(&format!("header.big == {i64_max}"), &ex)
                .await
                .unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            eval(&format!("{i64_max} == header.big"), &ex)
                .await
                .unwrap(),
            Value::Bool(false)
        );

        let ex = ex_with_headers(&[("big", big_u64.clone()), ("same", big_u64)]);
        assert_eq!(
            eval("header.big == header.same", &ex).await.unwrap(),
            Value::Bool(true)
        );
    }

    #[tokio::test]
    async fn nested_unary_minus_overflow_is_evaluated() {
        let ex = Exchange::default();
        let ast = crate::parser::parse("-(-9223372036854775808)").unwrap();
        let err = evaluate(&ast, &ex, &None).await.unwrap_err();
        assert!(
            matches!(
                err,
                LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Arithmetic,
                    ..
                }
            ),
            "{err:?}"
        );

        let ast = crate::parser::parse("-5").unwrap();
        assert_eq!(
            evaluate(&ast, &ex, &None).await.unwrap(),
            Value::Number((-5i64).into())
        );
    }
}
