//! camel-language-api — traits and types for building Camel expression languages.
//!
//! Language SPI for rust-camel — defines the core traits all expression/predicate languages implement.
//!
//! Main traits: `Language`, `Expression`, `Predicate`, `MutatingExpression`, `MutatingPredicate`.
//! Main modules: `error`.

pub mod error;

pub use async_trait::async_trait;
pub use camel_api::Value;
pub use camel_api::body::Body;
pub use camel_api::exchange::Exchange;
pub use camel_api::message::Message;
pub use error::LanguageError;

/// A Language factory: produces Expression and Predicate objects.
pub trait Language: Send + Sync {
    fn name(&self) -> &'static str;
    fn create_expression(&self, script: &str) -> Result<Box<dyn Expression>, LanguageError>;
    fn create_predicate(&self, script: &str) -> Result<Box<dyn Predicate>, LanguageError>;

    /// Create a mutating expression. Default returns NotSupported.
    fn create_mutating_expression(
        &self,
        _script: &str,
    ) -> Result<Box<dyn MutatingExpression>, LanguageError> {
        Err(LanguageError::NotSupported {
            feature: "mutating expressions".into(),
            language: self.name().into(),
        })
    }

    /// Create a mutating predicate. Default returns NotSupported.
    fn create_mutating_predicate(
        &self,
        _script: &str,
    ) -> Result<Box<dyn MutatingPredicate>, LanguageError> {
        Err(LanguageError::NotSupported {
            feature: "mutating predicates".into(),
            language: self.name().into(),
        })
    }
}

/// Evaluates to a Value against an Exchange.
#[async_trait]
pub trait Expression: Send + Sync {
    async fn evaluate(&self, exchange: &Exchange) -> Result<Value, LanguageError>;
}

/// Expression that may modify the Exchange during evaluation.
/// Changes to headers, properties, or body are propagated back.
#[async_trait]
pub trait MutatingExpression: Send + Sync {
    async fn evaluate(&self, exchange: &mut Exchange) -> Result<Value, LanguageError>;
}

/// Evaluates to bool against an Exchange.
#[async_trait]
pub trait Predicate: Send + Sync {
    async fn matches(&self, exchange: &Exchange) -> Result<bool, LanguageError>;
}

/// Predicate that may modify the Exchange during evaluation.
/// Changes to headers, properties, or body are propagated back.
///
/// Reserved for future use. Not yet implemented by any language.
#[async_trait]
pub trait MutatingPredicate: Send + Sync {
    async fn matches(&self, exchange: &mut Exchange) -> Result<bool, LanguageError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal `Language` impl used to verify the trait compiles and methods work.
    struct MockLanguage;

    impl Language for MockLanguage {
        fn name(&self) -> &'static str {
            "mock"
        }

        fn create_expression(&self, _script: &str) -> Result<Box<dyn Expression>, LanguageError> {
            Ok(Box::new(MockExpression))
        }

        fn create_predicate(&self, _script: &str) -> Result<Box<dyn Predicate>, LanguageError> {
            Ok(Box::new(MockPredicate))
        }
    }

    struct MockExpression;

    #[async_trait]
    impl Expression for MockExpression {
        async fn evaluate(&self, _exchange: &Exchange) -> Result<Value, LanguageError> {
            Ok(Value::String("mock".into()))
        }
    }

    struct MockPredicate;

    #[async_trait]
    impl Predicate for MockPredicate {
        async fn matches(&self, _exchange: &Exchange) -> Result<bool, LanguageError> {
            Ok(true)
        }
    }

    #[test]
    fn mock_language_returns_name() {
        let lang = MockLanguage;
        assert_eq!(lang.name(), "mock");
    }

    #[tokio::test]
    async fn mock_language_creates_expression_and_evaluates() {
        let lang = MockLanguage;
        let expr = lang.create_expression("any").unwrap();
        let ex = Exchange::new(Message::default());
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result.as_str().unwrap(), "mock");
    }

    #[tokio::test]
    async fn mock_language_creates_predicate_and_matches() {
        let lang = MockLanguage;
        let pred = lang.create_predicate("any").unwrap();
        let ex = Exchange::new(Message::default());
        assert!(pred.matches(&ex).await.unwrap());
    }

    #[test]
    fn default_mutating_expression_returns_not_supported() {
        let lang = MockLanguage;
        let result = lang.create_mutating_expression("any");
        assert!(matches!(result, Err(LanguageError::NotSupported { .. })));
        if let Err(LanguageError::NotSupported { feature, language }) = result {
            assert_eq!(feature, "mutating expressions");
            assert_eq!(language, "mock");
        }
    }

    #[test]
    fn default_mutating_predicate_returns_not_supported() {
        let lang = MockLanguage;
        let result = lang.create_mutating_predicate("any");
        assert!(matches!(result, Err(LanguageError::NotSupported { .. })));
    }

    #[test]
    fn language_trait_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MockLanguage>();
    }
}
