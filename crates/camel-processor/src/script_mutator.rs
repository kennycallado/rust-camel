use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::CamelError;
use camel_api::exchange::Exchange;
use camel_language_api::MutatingExpression;

/// Processor that executes a mutating expression, allowing scripts to modify the Exchange.
/// Uses `Arc<dyn MutatingExpression>` to enable `Clone` (required by `BoxProcessor`).
#[derive(Clone)]
pub struct ScriptMutator {
    expression: Arc<dyn MutatingExpression>,
}

impl ScriptMutator {
    pub fn new(expression: Box<dyn MutatingExpression>) -> Self {
        Self {
            expression: expression.into(),
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
        let result = self.expression.evaluate(&mut exchange);
        Box::pin(async move { result.map(|_| exchange).map_err(language_err_to_camel) })
    }
}

fn language_err_to_camel(e: camel_language_api::LanguageError) -> CamelError {
    use camel_language_api::LanguageError;
    match e {
        LanguageError::EvalError(msg) => CamelError::ProcessorError(msg),
        LanguageError::ParseError { expr, reason } => {
            CamelError::ProcessorError(format!("parse error in `{expr}`: {reason}"))
        }
        LanguageError::NotSupported { feature, language } => CamelError::ProcessorError(format!(
            "feature '{feature}' not supported by language '{language}'"
        )),
        other => CamelError::ProcessorError(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use camel_api::{Exchange, Message, Value};
    use camel_language_api::LanguageError;
    use tower::ServiceExt;

    use super::*;

    /// A simple test mutating expression that sets a header
    struct TestMutatingExpression;

    impl MutatingExpression for TestMutatingExpression {
        fn evaluate(&self, exchange: &mut Exchange) -> Result<Value, LanguageError> {
            exchange
                .input
                .headers
                .insert("mutated".into(), Value::Bool(true));
            Ok(Value::Null)
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
        // If this compiles, Clone is implemented correctly via Arc
    }
}
