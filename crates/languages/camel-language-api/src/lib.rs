pub mod error;

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
pub trait Expression: Send + Sync {
    fn evaluate(&self, exchange: &Exchange) -> Result<Value, LanguageError>;
}

/// Expression that may modify the Exchange during evaluation.
/// Changes to headers, properties, or body are propagated back.
pub trait MutatingExpression: Send + Sync {
    fn evaluate(&self, exchange: &mut Exchange) -> Result<Value, LanguageError>;
}

/// Evaluates to bool against an Exchange.
pub trait Predicate: Send + Sync {
    fn matches(&self, exchange: &Exchange) -> Result<bool, LanguageError>;
}

/// Predicate that may modify the Exchange during evaluation.
/// Changes to headers, properties, or body are propagated back.
///
/// Reserved for future use. Not yet implemented by any language.
pub trait MutatingPredicate: Send + Sync {
    fn matches(&self, exchange: &mut Exchange) -> Result<bool, LanguageError>;
}
