use crate::ResolverFn;
use crate::parser::{Expr, InterpolatedPart, LogicalOp, Op, PathSegment};
use camel_language_api::{Body, Exchange, LanguageError, Value};

pub fn evaluate(
    expr: &Expr,
    exchange: &Exchange,
    resolver: &Option<ResolverFn>,
) -> Result<Value, LanguageError> {
    match expr {
        Expr::Header(key) => Ok(exchange.input.header(key).cloned().unwrap_or(Value::Null)),

        Expr::Body => match &exchange.input.body {
            Body::Empty => Ok(Value::Null),
            Body::Stream(_) => Ok(Value::Null),
            Body::Text(s) => Ok(Value::String(s.clone())),
            Body::Bytes(b) => Ok(Value::String(String::from_utf8_lossy(b).into_owned())),
            Body::Json(v) => Ok(Value::String(v.to_string())),
            Body::Xml(s) => Ok(Value::String(s.clone())),
        },

        Expr::BodyField(segments) => {
            if let Body::Json(root) = &exchange.input.body {
                let mut current: &serde_json::Value = root;
                for seg in segments {
                    let next = match seg {
                        PathSegment::Key(k) => current.get(k.as_str()),
                        PathSegment::Index(i) => current.get(*i),
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

        Expr::ExchangeProperty(key) => Ok(exchange.property(key).cloned().unwrap_or(Value::Null)),

        Expr::LanguageDelegate {
            language,
            expression,
        } => {
            let resolver = resolver.as_ref().ok_or_else(|| {
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
            let expr = lang.create_expression(expression)?;
            expr.evaluate(exchange)
        }

        Expr::StringLit(s) => Ok(Value::String(s.clone())),
        Expr::EscapedString(s) => Ok(Value::String(s.clone())),
        Expr::NumberLit(n) => serde_json::Number::from_f64(*n)
            .map(Value::Number)
            .ok_or_else(|| LanguageError::EvalError(format!("non-finite number: {n}"))),
        Expr::Null => Ok(Value::Null),

        Expr::BinOp { left, op, right } => {
            let lv = evaluate(left, exchange, resolver)?;
            let rv = evaluate(right, exchange, resolver)?;
            let result = apply_op(&lv, op, &rv)?;
            // Design note: BinOp always produces a Bool result. The Simple Language
            // only supports comparison/predicate expressions — arithmetic or
            // string-concatenation operators are out of scope. If non-boolean BinOp
            // results are needed in the future, this arm should return Value directly
            // and callers (Predicate::matches) should coerce the result to bool.
            Ok(Value::Bool(result))
        }

        Expr::Interpolated(parts) => {
            let mut result = String::new();
            for part in parts {
                match part {
                    InterpolatedPart::Literal(text) => result.push_str(text),
                    InterpolatedPart::Expr(sub_expr) => {
                        let val = evaluate(sub_expr, exchange, resolver)?;
                        match val {
                            Value::Null => {} // missing value → empty string in interpolation
                            Value::String(s) => result.push_str(&s),
                            other => result.push_str(&other.to_string()),
                        }
                    }
                }
            }
            Ok(Value::String(result))
        }

        Expr::LogicalOp { left, op, right } => {
            let lv = evaluate(left, exchange, resolver)?;
            match op {
                LogicalOp::And => {
                    if !is_truthy(&lv) {
                        return Ok(Value::Bool(false));
                    }
                    let rv = evaluate(right, exchange, resolver)?;
                    Ok(Value::Bool(is_truthy(&rv)))
                }
                LogicalOp::Or => {
                    if is_truthy(&lv) {
                        return Ok(Value::Bool(true));
                    }
                    let rv = evaluate(right, exchange, resolver)?;
                    Ok(Value::Bool(is_truthy(&rv)))
                }
            }
        }
        Expr::Bool(b) => Ok(Value::Bool(*b)),
    }
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

fn apply_op(left: &Value, op: &Op, right: &Value) -> Result<bool, LanguageError> {
    match op {
        Op::Eq => Ok(left == right),
        Op::Ne => Ok(left != right),
        Op::Contains => {
            if matches!(left, Value::Null) || matches!(right, Value::Null) {
                return Ok(false);
            }
            let ls = left
                .as_str()
                .ok_or_else(|| LanguageError::EvalError("contains requires string left".into()))?;
            let rs = right
                .as_str()
                .ok_or_else(|| LanguageError::EvalError("contains requires string right".into()))?;
            Ok(ls.contains(rs))
        }
        Op::Gt | Op::Lt | Op::Gte | Op::Lte => {
            if matches!(left, Value::Null) || matches!(right, Value::Null) {
                return Ok(false);
            }
            let ln = to_f64(left)?;
            let rn = to_f64(right)?;
            Ok(match op {
                Op::Gt => ln > rn,
                Op::Lt => ln < rn,
                Op::Gte => ln >= rn,
                Op::Lte => ln <= rn,
                _ => unreachable!(),
            })
        }
    }
}

fn to_f64(v: &Value) -> Result<f64, LanguageError> {
    v.as_f64()
        .ok_or_else(|| LanguageError::EvalError(format!("expected number, got {v}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_language_api::{Expression, Language, Message};
    use serde_json::json;
    use std::sync::Arc;

    struct MockExpression {
        out: Value,
    }

    impl Expression for MockExpression {
        fn evaluate(&self, _exchange: &Exchange) -> Result<Value, LanguageError> {
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

    #[test]
    fn evaluate_body_variants_and_header_property() {
        let mut msg = Message::new("txt");
        msg.set_header("k", Value::String("v".to_string()));
        let mut ex = Exchange::new(msg);
        ex.set_property("p".to_string(), Value::String("pv".to_string()));

        assert_eq!(
            evaluate(&Expr::Header("k".to_string()), &ex, &None).unwrap(),
            Value::String("v".to_string())
        );
        assert_eq!(
            evaluate(&Expr::ExchangeProperty("p".to_string()), &ex, &None).unwrap(),
            Value::String("pv".to_string())
        );
        assert_eq!(
            evaluate(&Expr::Body, &exchange_with_body(Body::Empty), &None).unwrap(),
            Value::Null
        );
        assert_eq!(
            evaluate(
                &Expr::Body,
                &exchange_with_body(Body::from(b"abc".to_vec())),
                &None
            )
            .unwrap(),
            Value::String("abc".to_string())
        );
        assert_eq!(
            evaluate(
                &Expr::Body,
                &exchange_with_body(Body::Json(json!({"a": 1}))),
                &None
            )
            .unwrap(),
            Value::String("{\"a\":1}".to_string())
        );
    }

    #[test]
    fn evaluate_body_field_success_and_missing() {
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
        .unwrap();
        assert_eq!(found, json!("Ana"));

        let missing = evaluate(
            &Expr::BodyField(vec![PathSegment::Key("missing".to_string())]),
            &ex,
            &None,
        )
        .unwrap();
        assert_eq!(missing, Value::Null);
    }

    #[test]
    fn evaluate_delegate_and_delegate_errors() {
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
            evaluate(&expr, &ex, &resolver).unwrap(),
            Value::String("mock:x".to_string())
        );
        assert!(evaluate(&expr, &ex, &None).is_err());

        let missing_resolver: Option<ResolverFn> = Some(Arc::new(|_| None));
        assert!(evaluate(&expr, &ex, &missing_resolver).is_err());
    }

    #[test]
    fn evaluate_binop_and_contains_and_type_errors() {
        let ex = Exchange::default();
        assert_eq!(
            evaluate(
                &Expr::BinOp {
                    left: Box::new(Expr::NumberLit(7.0)),
                    op: Op::Gt,
                    right: Box::new(Expr::NumberLit(3.0)),
                },
                &ex,
                &None,
            )
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
            .is_err()
        );
    }

    #[test]
    fn evaluate_interpolated_and_logical_truthiness() {
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
        .unwrap();
        assert_eq!(out, Value::String("id=7 body=hello miss=".to_string()));

        let and_short = evaluate(
            &Expr::LogicalOp {
                left: Box::new(Expr::Bool(false)),
                op: LogicalOp::And,
                right: Box::new(Expr::BinOp {
                    left: Box::new(Expr::StringLit("x".to_string())),
                    op: Op::Gt,
                    right: Box::new(Expr::NumberLit(1.0)),
                }),
            },
            &ex,
            &None,
        )
        .unwrap();
        assert_eq!(and_short, Value::Bool(false));

        let or_short = evaluate(
            &Expr::LogicalOp {
                left: Box::new(Expr::StringLit("non-empty".to_string())),
                op: LogicalOp::Or,
                right: Box::new(Expr::BinOp {
                    left: Box::new(Expr::StringLit("x".to_string())),
                    op: Op::Gt,
                    right: Box::new(Expr::NumberLit(1.0)),
                }),
            },
            &ex,
            &None,
        )
        .unwrap();
        assert_eq!(or_short, Value::Bool(true));
    }
}
