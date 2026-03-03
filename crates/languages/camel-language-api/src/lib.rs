pub mod error;

use camel_api::exchange::Exchange;
use camel_api::Value;
pub use error::LanguageError;

/// A Language factory: produces Expression and Predicate objects.
pub trait Language: Send + Sync {
    fn name(&self) -> &'static str;
    fn create_expression(&self, script: &str) -> Result<Box<dyn Expression>, LanguageError>;
    fn create_predicate(&self, script: &str) -> Result<Box<dyn Predicate>, LanguageError>;
}

/// Evaluates to a Value against an Exchange.
pub trait Expression: Send + Sync {
    fn evaluate(&self, exchange: &Exchange) -> Result<Value, LanguageError>;
}

/// Evaluates to bool against an Exchange.
pub trait Predicate: Send + Sync {
    fn matches(&self, exchange: &Exchange) -> Result<bool, LanguageError>;
}
