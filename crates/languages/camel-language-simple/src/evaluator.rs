use crate::parser::{Expr, InterpolatedPart, Op, PathSegment};
use camel_language_api::{Body, Exchange, LanguageError, Value};

pub fn evaluate(expr: &Expr, exchange: &Exchange) -> Result<Value, LanguageError> {
    match expr {
        Expr::Header(key) => Ok(exchange.input.header(key).cloned().unwrap_or(Value::Null)),

        Expr::Body => {
            let s = match &exchange.input.body {
                Body::Text(s) => s.clone(),
                Body::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                Body::Json(v) => v.to_string(),
                Body::Xml(s) => s.clone(),
                Body::Empty => String::new(),
                Body::Stream(_) => String::new(),
            };
            Ok(Value::String(s))
        }

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

        Expr::StringLit(s) => Ok(Value::String(s.clone())),
        Expr::NumberLit(n) => serde_json::Number::from_f64(*n)
            .map(Value::Number)
            .ok_or_else(|| LanguageError::EvalError(format!("non-finite number: {n}"))),
        Expr::Null => Ok(Value::Null),

        Expr::BinOp { left, op, right } => {
            let lv = evaluate(left, exchange)?;
            let rv = evaluate(right, exchange)?;
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
                        let val = evaluate(sub_expr, exchange)?;
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
    }
}

fn apply_op(left: &Value, op: &Op, right: &Value) -> Result<bool, LanguageError> {
    match op {
        Op::Eq => Ok(left == right),
        Op::Ne => Ok(left != right),
        Op::Contains => {
            let ls = left
                .as_str()
                .ok_or_else(|| LanguageError::EvalError("contains requires string left".into()))?;
            let rs = right
                .as_str()
                .ok_or_else(|| LanguageError::EvalError("contains requires string right".into()))?;
            Ok(ls.contains(rs))
        }
        Op::Gt | Op::Lt | Op::Gte | Op::Lte => {
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
