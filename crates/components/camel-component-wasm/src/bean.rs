use async_trait::async_trait;
use camel_api::{CamelError, Exchange};
use camel_bean::BeanProcessor;

/// Stub implementation of [`BeanProcessor`] for WASM beans.
///
/// This validates the interface contract without real WASM invocation,
/// allowing the rest of the pipeline (config, CLI, registration) to be
/// tested end-to-end.
pub struct WasmBean {
    methods: Vec<String>,
}

impl WasmBean {
    /// Create a new stub bean with the given declared methods.
    pub fn new_stub(methods: Vec<String>) -> Self {
        Self { methods }
    }
}

#[async_trait]
impl BeanProcessor for WasmBean {
    async fn call(&self, method: &str, _exchange: &mut Exchange) -> Result<(), CamelError> {
        if !self.methods.iter().any(|m| m == method) {
            return Err(CamelError::ProcessorError(format!(
                "unknown bean method: {method}"
            )));
        }
        Ok(())
    }

    fn methods(&self) -> Vec<String> {
        self.methods.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::Message;

    #[tokio::test]
    async fn test_wasm_bean_methods_returns_declared() {
        let bean = WasmBean::new_stub(vec!["validate-jwt".to_string(), "generate-jwt".to_string()]);
        assert_eq!(bean.methods(), vec!["validate-jwt", "generate-jwt"]);
    }

    #[tokio::test]
    async fn test_wasm_bean_rejects_unknown_method() {
        let bean = WasmBean::new_stub(vec!["validate-jwt".to_string()]);
        let mut exchange = Exchange::new(Message::default());
        let result = bean.call("unknown", &mut exchange).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_wasm_bean_accepts_known_method() {
        let bean = WasmBean::new_stub(vec!["validate-jwt".to_string()]);
        let mut exchange = Exchange::new(Message::default());
        let result = bean.call("validate-jwt", &mut exchange).await;
        assert!(result.is_ok());
    }
}
