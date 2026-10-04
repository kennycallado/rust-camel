use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::CamelError;
use camel_api::exchange::Exchange;
use camel_language_api::{EvalMeta, MutatingExpression, to_expression_failed};

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
        Box::pin(async move {
            let result = expression.evaluate(&mut exchange).await;
            result
                .map(|_| exchange)
                .map_err(|e| to_expression_failed(e, &meta))
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
}
