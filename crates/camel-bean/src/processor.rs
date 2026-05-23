use async_trait::async_trait;
use camel_api::{CamelError, Exchange};

#[async_trait]
pub trait BeanProcessor: Send + Sync {
    /// Invoke a named method on this bean, mutating the exchange in place.
    ///
    /// The `method` parameter selects the target method by name. When a bean
    /// exposes multiple methods with the same name (overloads), callers may
    /// supply [`method_params`](BeanProcessor::method_params) to disambiguate.
    async fn call(&self, method: &str, exchange: &mut Exchange) -> Result<(), CamelError>;

    /// Returns the list of method names this bean exposes.
    fn methods(&self) -> Vec<String>;

    /// Optional parameter-type hints for overload resolution.
    ///
    /// When `Some`, the vector contains fully-qualified type names (e.g.
    /// `"String"`, `"i32"`) that identify which overload of a method to invoke.
    ///
    /// TODO: Full overload resolution (matching by parameter types at runtime)
    /// is not yet implemented. Currently only method-name dispatch is supported.
    fn method_params(&self) -> Option<Vec<String>> {
        None
    }

    async fn on_start(&self) -> Result<(), CamelError> {
        Ok(())
    }

    async fn on_stop(&self) -> Result<(), CamelError> {
        Ok(())
    }
}
