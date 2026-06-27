use std::cmp::Ordering;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::{CamelError, Exchange};

/// Total order over a JSON value used as a sort key.
/// Tier: Null < Bool < Number < String.
/// Array/Object keys are REJECTED at extraction.
#[derive(Debug, Clone)]
pub struct SortKey(pub serde_json::Value);

impl Ord for SortKey {
    fn cmp(&self, other: &Self) -> Ordering {
        use serde_json::Value::*;
        match (&self.0, &other.0) {
            (Null, Null) => Ordering::Equal,
            (Null, _) => Ordering::Less,
            (_, Null) => Ordering::Greater,
            (Bool(a), Bool(b)) => a.cmp(b),
            (Bool(_), Number(_)) | (Bool(_), String(_)) => Ordering::Less,
            (Number(_), Bool(_)) | (String(_), Bool(_)) => Ordering::Greater,
            // ponytail: serde_json::Number cannot represent NaN/Infinity; this branch
            // is defensive and will never fire for valid JSON input.
            (Number(a), Number(b)) => {
                let af = a.as_f64().unwrap_or(f64::INFINITY);
                let bf = b.as_f64().unwrap_or(f64::INFINITY);
                af.partial_cmp(&bf)
                    .unwrap_or_else(|| af.is_nan().cmp(&bf.is_nan()))
            }
            (Number(_), String(_)) => Ordering::Less,
            (String(_), Number(_)) => Ordering::Greater,
            (String(a), String(b)) => a.cmp(b),
            _ => Ordering::Equal, // defensive fallback for Array/Object (shouldn't reach here)
        }
    }
}

impl PartialOrd for SortKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for SortKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for SortKey {}

/// Extracts a sort key from each element of the body array.
/// MUST return a scalar (Null/Bool/Number/String); returning Array/Object is a user error.
pub type SortExpression =
    Arc<dyn Fn(&serde_json::Value) -> Result<SortKey, CamelError> + Send + Sync>;

/// SortService: order body collection by expression.
///
/// Process-mode leaf processor (no child pipeline). Requires `Body::Json(Value::Array(_))`.
/// Non-array/non-Json body → Err. Array/Object keys → Err.
#[derive(Clone)]
pub struct SortService {
    expression: SortExpression,
    reverse: bool,
}

impl SortService {
    pub fn new(expression: SortExpression, reverse: bool) -> Self {
        Self {
            expression,
            reverse,
        }
    }
}

impl Service<Exchange> for SortService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let expression = Arc::clone(&self.expression);
        let reverse = self.reverse;
        Box::pin(async move {
            let array = match std::mem::take(&mut exchange.input.body) {
                camel_api::body::Body::Json(serde_json::Value::Array(arr)) => arr,
                _ => {
                    return Err(CamelError::ProcessorError(
                        "sort requires an array body (Body::Json(Value::Array))".into(),
                    ));
                }
            };

            let mut indexed: Vec<(SortKey, serde_json::Value)> = Vec::with_capacity(array.len());
            for element in array {
                let key = expression(&element)?;
                indexed.push((key, element));
            }

            if reverse {
                indexed.sort_by(|a, b| b.0.cmp(&a.0));
            } else {
                indexed.sort_by(|a, b| a.0.cmp(&b.0));
            }

            let sorted: Vec<serde_json::Value> = indexed.into_iter().map(|(_, v)| v).collect();
            exchange.input.body = camel_api::body::Body::Json(serde_json::Value::Array(sorted));
            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{Exchange, Message, body::Body};
    use serde_json::json;
    use tower::ServiceExt;

    #[tokio::test]
    async fn ascending_numeric_sort() {
        let exchange = Exchange::new(Message::new(Body::Json(json!([3, 1, 2]))));
        let expr: SortExpression = Arc::new(|v| Ok(SortKey(v.clone())));
        let svc = SortService::new(expr, false);
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body, Body::Json(json!([1, 2, 3])));
    }

    #[tokio::test]
    async fn descending_numeric_sort() {
        let exchange = Exchange::new(Message::new(Body::Json(json!([3, 1, 2]))));
        let expr: SortExpression = Arc::new(|v| Ok(SortKey(v.clone())));
        let svc = SortService::new(expr, true);
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body, Body::Json(json!([3, 2, 1])));
    }

    #[tokio::test]
    async fn string_sort() {
        let exchange = Exchange::new(Message::new(Body::Json(json!([
            "banana", "apple", "cherry"
        ]))));
        let expr: SortExpression = Arc::new(|v| Ok(SortKey(v.clone())));
        let svc = SortService::new(expr, false);
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.input.body,
            Body::Json(json!(["apple", "banana", "cherry"]))
        );
    }

    #[tokio::test]
    async fn empty_array_passthrough() {
        let exchange = Exchange::new(Message::new(Body::Json(json!([]))));
        let expr: SortExpression = Arc::new(|v| Ok(SortKey(v.clone())));
        let svc = SortService::new(expr, false);
        let result = svc.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body, Body::Json(json!([])));
    }

    #[tokio::test]
    async fn non_array_body_errors() {
        let exchange = Exchange::new(Message::new(Body::Text("hello".to_string())));
        let expr: SortExpression = Arc::new(|v| Ok(SortKey(v.clone())));
        let svc = SortService::new(expr, false);
        let result = svc.oneshot(exchange).await;
        assert!(matches!(result, Err(CamelError::ProcessorError(_))));
    }

    #[tokio::test]
    async fn array_key_errors() {
        let exchange = Exchange::new(Message::new(Body::Json(json!([[1, 2], 3]))));
        let expr: SortExpression = Arc::new(|v| {
            if v.is_array() {
                Err(CamelError::ProcessorError("array key rejected".into()))
            } else {
                Ok(SortKey(v.clone()))
            }
        });
        let svc = SortService::new(expr, false);
        let result = svc.oneshot(exchange).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn mixed_type_sort_key_order() {
        // Null < Bool < Number < String
        let exchange = Exchange::new(Message::new(Body::Json(json!([
            "str", null, false, 42, true, 1
        ]))));
        let expr: SortExpression = Arc::new(|v| Ok(SortKey(v.clone())));
        let svc = SortService::new(expr, false);
        let result = svc.oneshot(exchange).await.unwrap();
        // Expected: null, false, true, 1, 42, "str"
        assert_eq!(
            result.input.body,
            Body::Json(json!([null, false, true, 1, 42, "str"]))
        );
    }
}
