use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::exchange::Exchange;
use camel_api::{CamelError, Headers, PROPERTY_HTTP_CONTENT_TYPE_INHERITED, Value};
use camel_language_api::{EvalMeta, MutatingExpression, to_expression_failed};

/// Collect the exact Content-Type header entries (key spelling and value) as a
/// deterministically ordered list.
///
/// Both language surfaces commit changed-only per exact key, so a before/after
/// comparison of this list detects every landed Content-Type mutation while
/// ignoring same-value assignments.
fn content_type_entries(headers: &Headers) -> Vec<(String, Value)> {
    let mut entries: Vec<(String, Value)> = headers
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case("content-type"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

/// Processor that executes a mutating expression, allowing scripts to modify the Exchange.
/// Uses `Arc<dyn MutatingExpression>` to enable `Clone` (required by `BoxProcessor`).
///
/// Evaluation failures are mapped to [`CamelError::ExpressionFailed`] with
/// the trusted route metadata supplied via [`ScriptMutator::with_meta`]
/// (ParseError routes as class `Parse` via `LanguageError::class()`).
#[derive(Clone)]
pub struct ScriptMutator {
    expression: Arc<dyn MutatingExpression>,
    meta: EvalMeta,
}

impl ScriptMutator {
    /// Create without route metadata. The default metadata reports language
    /// `unknown` — tests only; production code should use
    /// [`ScriptMutator::with_meta`].
    pub fn new(expression: Box<dyn MutatingExpression>) -> Self {
        Self::with_meta(
            expression,
            EvalMeta {
                language: "unknown".to_string(),
                route_id: String::new(),
                step_id: String::new(),
                verb: String::new(),
                target: None,
            },
        )
    }

    /// Create with trusted route metadata used to enrich evaluation
    /// failures as [`CamelError::ExpressionFailed`].
    pub fn with_meta(expression: Box<dyn MutatingExpression>, meta: EvalMeta) -> Self {
        Self {
            expression: expression.into(),
            meta,
        }
    }
}

impl Service<Exchange> for ScriptMutator {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let expression = self.expression.clone();
        let meta = self.meta.clone();
        let content_type_before = content_type_entries(&exchange.input.headers);
        Box::pin(async move {
            let result = expression.evaluate(&mut exchange).await;
            match result {
                Ok(_) => {
                    if content_type_entries(&exchange.input.headers) != content_type_before {
                        exchange.remove_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED);
                    }
                    Ok(exchange)
                }
                Err(e) => Err(to_expression_failed(e, &meta)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use camel_api::{CamelError, Exchange, ExpressionErrorClass, Message, Value};
    use camel_language_api::{EvalMeta, LanguageError};
    use tower::ServiceExt;

    use super::*;

    /// A mutating expression that always fails with a structured failure.
    struct EvalFailureMutatingExpression;

    #[async_trait::async_trait]
    impl MutatingExpression for EvalFailureMutatingExpression {
        async fn evaluate(&self, _exchange: &mut Exchange) -> Result<Value, LanguageError> {
            Err(LanguageError::EvalFailure {
                class: ExpressionErrorClass::Runtime,
                position: None,
                detail: None,
            })
        }
    }

    /// A simple test mutating expression that sets a header
    struct TestMutatingExpression;

    struct ParseErrorMutatingExpression;
    struct NotSupportedMutatingExpression;
    struct UnknownVariableMutatingExpression;

    #[async_trait::async_trait]
    impl MutatingExpression for TestMutatingExpression {
        async fn evaluate(&self, exchange: &mut Exchange) -> Result<Value, LanguageError> {
            exchange
                .input
                .headers
                .insert("mutated".into(), Value::Bool(true));
            Ok(Value::Null)
        }
    }

    #[async_trait::async_trait]
    impl MutatingExpression for ParseErrorMutatingExpression {
        async fn evaluate(&self, _exchange: &mut Exchange) -> Result<Value, LanguageError> {
            Err(LanguageError::ParseError {
                expr: "x".to_string(),
                reason: "bad".to_string(),
            })
        }
    }

    #[async_trait::async_trait]
    impl MutatingExpression for NotSupportedMutatingExpression {
        async fn evaluate(&self, _exchange: &mut Exchange) -> Result<Value, LanguageError> {
            Err(LanguageError::NotSupported {
                feature: "f".to_string(),
                language: "l".to_string(),
            })
        }
    }

    #[async_trait::async_trait]
    impl MutatingExpression for UnknownVariableMutatingExpression {
        async fn evaluate(&self, _exchange: &mut Exchange) -> Result<Value, LanguageError> {
            Err(LanguageError::UnknownVariable("foo".to_string()))
        }
    }

    #[tokio::test]
    async fn test_script_mutator_modifies_exchange() {
        let exchange = Exchange::new(Message::new("test"));

        let mutator = ScriptMutator::new(Box::new(TestMutatingExpression));

        let result = mutator.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.header("mutated"), Some(&Value::Bool(true)));
    }

    #[tokio::test]
    async fn test_script_mutator_preserves_body() {
        let exchange = Exchange::new(Message::new("original body"));

        let mutator = ScriptMutator::new(Box::new(TestMutatingExpression));

        let result = mutator.oneshot(exchange).await.unwrap();
        assert_eq!(result.input.body.as_text(), Some("original body"));
    }

    #[tokio::test]
    async fn test_script_mutator_is_clone() {
        let mutator = ScriptMutator::new(Box::new(TestMutatingExpression));
        let _cloned = mutator.clone();
    }

    #[tokio::test]
    async fn script_mutator_error_is_expression_failed() {
        let meta = EvalMeta {
            language: "rhai".to_string(),
            route_id: "r1".to_string(),
            step_id: "script#0".to_string(),
            verb: "script".to_string(),
            target: None,
        };
        let mutator = ScriptMutator::with_meta(Box::new(EvalFailureMutatingExpression), meta);

        let result = mutator.oneshot(Exchange::new(Message::new("test"))).await;

        assert!(
            matches!(
                result,
                Err(CamelError::ExpressionFailed {
                    ref language,
                    ref verb,
                    class: ExpressionErrorClass::Runtime,
                    ..
                }) if language == "rhai" && verb == "script"
            ),
            "mutating-eval failure must map to ExpressionFailed with the bound meta"
        );
    }

    #[tokio::test]
    async fn test_script_mutator_maps_parse_error() {
        let exchange = Exchange::new(Message::new("test"));
        let mutator = ScriptMutator::new(Box::new(ParseErrorMutatingExpression));
        let result = mutator.oneshot(exchange).await;
        assert!(matches!(
            result,
            Err(CamelError::ExpressionFailed {
                class: ExpressionErrorClass::Parse,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn test_script_mutator_maps_not_supported_error() {
        let exchange = Exchange::new(Message::new("test"));
        let mutator = ScriptMutator::new(Box::new(NotSupportedMutatingExpression));
        let result = mutator.oneshot(exchange).await;
        // NotSupported carries no class: to_expression_failed defaults it to
        // Runtime under the default "unknown" language.
        assert!(matches!(
            result,
            Err(CamelError::ExpressionFailed {
                ref language,
                class: ExpressionErrorClass::Runtime,
                ..
            }) if language == "unknown"
        ));
    }

    #[tokio::test]
    async fn test_script_mutator_maps_other_language_error() {
        let exchange = Exchange::new(Message::new("test"));
        let mutator = ScriptMutator::new(Box::new(UnknownVariableMutatingExpression));
        let result = mutator.oneshot(exchange).await;
        assert!(matches!(
            result,
            Err(CamelError::ExpressionFailed {
                class: ExpressionErrorClass::Runtime,
                ..
            })
        ));
    }

    // ── Content-Type provenance marker ──

    /// A script that assigns a new value to the `Content-Type` header.
    struct SetContentTypeMutatingExpression(&'static str);

    #[async_trait::async_trait]
    impl MutatingExpression for SetContentTypeMutatingExpression {
        async fn evaluate(&self, exchange: &mut Exchange) -> Result<Value, LanguageError> {
            exchange
                .input
                .headers
                .insert("Content-Type".into(), Value::String(self.0.to_string()));
            Ok(Value::Null)
        }
    }

    /// A script that renames the `Content-Type` header key casing, keeping the value.
    struct RenameContentTypeCaseMutatingExpression;

    #[async_trait::async_trait]
    impl MutatingExpression for RenameContentTypeCaseMutatingExpression {
        async fn evaluate(&self, exchange: &mut Exchange) -> Result<Value, LanguageError> {
            let value = exchange
                .input
                .headers
                .remove("Content-Type")
                .expect("test double expects a Content-Type header");
            exchange.input.headers.insert("content-type".into(), value);
            Ok(Value::Null)
        }
    }

    /// A script that removes the `Content-Type` header.
    struct RemoveContentTypeMutatingExpression;

    #[async_trait::async_trait]
    impl MutatingExpression for RemoveContentTypeMutatingExpression {
        async fn evaluate(&self, exchange: &mut Exchange) -> Result<Value, LanguageError> {
            exchange.input.headers.remove("Content-Type");
            Ok(Value::Null)
        }
    }

    /// A script that only touches the body.
    struct BodyOnlyMutatingExpression;

    #[async_trait::async_trait]
    impl MutatingExpression for BodyOnlyMutatingExpression {
        async fn evaluate(&self, exchange: &mut Exchange) -> Result<Value, LanguageError> {
            exchange.input.body = "mutated body".into();
            Ok(Value::Null)
        }
    }

    fn exchange_with_content_type(value: &str) -> Exchange {
        let mut exchange = Exchange::new(Message::new("test"));
        exchange
            .input
            .headers
            .insert("Content-Type".into(), Value::String(value.into()));
        exchange.set_property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED, Value::Bool(true));
        exchange
    }

    #[tokio::test]
    async fn script_mutator_clears_marker_on_ct_update() {
        let exchange = exchange_with_content_type("application/json");
        let mutator = ScriptMutator::new(Box::new(SetContentTypeMutatingExpression("text/html")));

        let result = mutator.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED),
            None,
            "a script write to Content-Type must clear the inherited marker"
        );
        assert_eq!(
            result.input.header("Content-Type"),
            Some(&Value::String("text/html".into()))
        );
    }

    #[tokio::test]
    async fn script_mutator_clears_marker_on_case_rename() {
        let exchange = exchange_with_content_type("application/json");
        let mutator = ScriptMutator::new(Box::new(RenameContentTypeCaseMutatingExpression));

        let result = mutator.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED),
            None,
            "renaming the Content-Type key casing lands a mutation and must clear the marker"
        );
        assert_eq!(
            result.input.header("content-type"),
            Some(&Value::String("application/json".into()))
        );
    }

    #[tokio::test]
    async fn script_mutator_clears_marker_on_ct_removal() {
        let exchange = exchange_with_content_type("application/json");
        let mutator = ScriptMutator::new(Box::new(RemoveContentTypeMutatingExpression));

        let result = mutator.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED),
            None,
            "removing Content-Type must clear the inherited marker"
        );
        assert_eq!(result.input.header("Content-Type"), None);
    }

    #[tokio::test]
    async fn script_mutator_keeps_marker_on_same_value_assignment() {
        let exchange = exchange_with_content_type("application/json");
        let mutator = ScriptMutator::new(Box::new(SetContentTypeMutatingExpression(
            "application/json",
        )));

        let result = mutator.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED),
            Some(&Value::Bool(true)),
            "a same-value assignment does not land a change and must keep the marker"
        );
    }

    #[tokio::test]
    async fn script_mutator_keeps_marker_on_body_only_script() {
        let exchange = exchange_with_content_type("application/json");
        let mutator = ScriptMutator::new(Box::new(BodyOnlyMutatingExpression));

        let result = mutator.oneshot(exchange).await.unwrap();
        assert_eq!(
            result.property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED),
            Some(&Value::Bool(true)),
            "a body-only script must keep the marker"
        );
        assert_eq!(result.input.body.as_text(), Some("mutated body"));
    }

    /// Marker value and headers captured by the snapshotting test double.
    type CapturedHeaderState = Option<(Option<Value>, camel_api::Headers)>;

    /// A failing script that snapshots the marker and headers it is handed. The
    /// `Err` path consumes the exchange, so recording the state inside `evaluate`
    /// is the only way to assert the caller cleared nothing on the error path.
    struct SnapshottingParseErrorMutatingExpression(Arc<std::sync::Mutex<CapturedHeaderState>>);

    #[async_trait::async_trait]
    impl MutatingExpression for SnapshottingParseErrorMutatingExpression {
        async fn evaluate(&self, exchange: &mut Exchange) -> Result<Value, LanguageError> {
            *self
                .0
                .lock()
                .expect("test double snapshot mutex is not poisoned") = Some((
                exchange
                    .property(PROPERTY_HTTP_CONTENT_TYPE_INHERITED)
                    .cloned(),
                exchange.input.headers.clone(),
            ));
            Err(LanguageError::ParseError {
                expr: "x".to_string(),
                reason: "bad".to_string(),
            })
        }
    }

    #[tokio::test]
    async fn script_mutator_keeps_marker_on_eval_error() {
        let snapshot = Arc::new(std::sync::Mutex::new(None));
        let mutator = ScriptMutator::new(Box::new(SnapshottingParseErrorMutatingExpression(
            snapshot.clone(),
        )));

        let result = mutator
            .oneshot(exchange_with_content_type("application/json"))
            .await;
        assert!(
            matches!(result, Err(CamelError::ExpressionFailed { .. })),
            "the parse error must propagate"
        );

        let (marker, headers) = snapshot
            .lock()
            .expect("test double snapshot mutex is not poisoned")
            .take()
            .expect("the expression must have run");
        assert_eq!(
            marker,
            Some(Value::Bool(true)),
            "a failed script must not clear the marker"
        );
        assert_eq!(
            headers.get("Content-Type"),
            Some(&Value::String("application/json".into())),
            "a failed script must not touch the headers"
        );
    }
}
