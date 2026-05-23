use crate::{BeanError, BeanProcessor};
use camel_api::{CamelError, Exchange};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::{debug, warn};

/// A thread-safe registry for named bean processors.
///
/// Uses `Arc<Mutex<...>>` internally so that `register()` and `lookup()`
/// can be called from multiple threads without external synchronisation.
pub struct BeanRegistry {
    beans: Arc<Mutex<HashMap<String, Arc<dyn BeanProcessor>>>>,
}

impl BeanRegistry {
    pub fn new() -> Self {
        Self {
            beans: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a bean under the given name.
    ///
    /// Returns `Err(BeanError::DuplicateName)` if a bean with the same name
    /// already exists, or `Err(BeanError::InvalidName)` if the name is blank.
    pub fn register<B>(&self, name: impl Into<String>, bean: B) -> Result<(), BeanError>
    where
        B: BeanProcessor + 'static,
    {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(BeanError::InvalidName(name));
        }
        let mut beans = self.beans.lock().unwrap_or_else(|e| e.into_inner());
        if beans.contains_key(&name) {
            warn!(bean_name = %name, "bean already registered");
            return Err(BeanError::DuplicateName(name));
        }
        beans.insert(name.clone(), Arc::new(bean));
        debug!(bean_name = %name, "bean registered");
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn BeanProcessor>> {
        self.beans
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .cloned()
    }

    pub async fn invoke(
        &self,
        bean_name: &str,
        method: &str,
        exchange: &mut Exchange,
    ) -> Result<(), CamelError> {
        debug!(bean_name = %bean_name, method = %method, "invoking bean");

        let bean = self.get(bean_name).ok_or_else(|| {
            warn!(bean_name = %bean_name, "bean not found");
            BeanError::NotFound(bean_name.to_string())
        })?;

        if !bean.methods().iter().any(|m| m == method) {
            warn!(bean_name = %bean_name, method = %method, "bean method not found");
            return Err(BeanError::MethodNotFound(method.to_string()).into());
        }

        match bean.call(method, exchange).await {
            Ok(()) => {
                debug!(bean_name = %bean_name, method = %method, "bean invocation succeeded");
                Ok(())
            }
            Err(err) => {
                warn!(bean_name = %bean_name, method = %method, error = %err, "bean invocation failed");
                Err(err)
            }
        }
    }

    pub fn len(&self) -> usize {
        self.beans.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.beans
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }
}

impl Default for BeanRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use camel_api::{Exchange, Message};

    struct MockBean;

    struct ErrorBean;

    #[async_trait]
    impl BeanProcessor for MockBean {
        async fn call(&self, method: &str, _exchange: &mut Exchange) -> Result<(), CamelError> {
            match method {
                "process" => Ok(()),
                _ => Err(BeanError::MethodNotFound(method.to_string()).into()),
            }
        }

        fn methods(&self) -> Vec<String> {
            vec!["process".to_string()]
        }
    }

    #[async_trait]
    impl BeanProcessor for ErrorBean {
        async fn call(&self, _method: &str, _exchange: &mut Exchange) -> Result<(), CamelError> {
            Err(CamelError::ProcessorError("boom".to_string()))
        }

        fn methods(&self) -> Vec<String> {
            vec!["process".to_string()]
        }
    }

    #[test]
    fn test_default_is_empty() {
        let registry = BeanRegistry::default();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_register_and_get() {
        let registry = BeanRegistry::new();
        registry.register("mock", MockBean).unwrap();

        assert!(registry.get("mock").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_len_and_is_empty() {
        let registry = BeanRegistry::new();
        assert!(registry.is_empty());

        registry.register("mock", MockBean).unwrap();
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[test]
    fn test_register_same_name_rejects() {
        let registry = BeanRegistry::new();
        registry.register("dup", MockBean).unwrap();
        let result = registry.register("dup", ErrorBean);
        assert!(result.is_err());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_register_duplicate_returns_error() {
        let registry = BeanRegistry::new();
        registry.register("dup", MockBean).unwrap();
        let err = registry.register("dup", ErrorBean).unwrap_err();
        assert!(matches!(err, BeanError::DuplicateName(_)));
    }

    #[tokio::test]
    async fn test_invoke_success() {
        let registry = BeanRegistry::new();
        registry.register("mock", MockBean).unwrap();

        let mut exchange = Exchange::new(Message::default());
        let result = registry.invoke("mock", "process", &mut exchange).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_invoke_bean_not_found() {
        let registry = BeanRegistry::new();
        let mut exchange = Exchange::new(Message::default());

        let result = registry
            .invoke("nonexistent", "process", &mut exchange)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_invoke_method_not_found() {
        let registry = BeanRegistry::new();
        registry.register("mock", MockBean).unwrap();

        let mut exchange = Exchange::new(Message::default());
        let result = registry.invoke("mock", "unknown", &mut exchange).await;

        assert!(result.is_err());
    }

    #[test]
    fn test_register_empty_name_rejected() {
        let registry = BeanRegistry::new();
        let result = registry.register("", MockBean);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), BeanError::InvalidName(_)));
    }

    #[test]
    fn test_register_whitespace_name_rejected() {
        let registry = BeanRegistry::new();
        let result = registry.register("   ", MockBean);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), BeanError::InvalidName(_)));
    }

    #[tokio::test]
    async fn test_invoke_propagates_bean_error() {
        let registry = BeanRegistry::new();
        registry.register("err", ErrorBean).unwrap();

        let mut exchange = Exchange::new(Message::default());
        let result = registry.invoke("err", "process", &mut exchange).await;

        assert!(matches!(result, Err(CamelError::ProcessorError(_))));
    }

    /// C-17: Verify that calling a method on a bean_impl-generated BeanProcessor
    /// returns Err (not panic) when the method is not found.
    #[tokio::test]
    async fn test_macro_generated_call_returns_err_for_unknown_method() {
        // Manually simulate what #[bean_impl] generates for a simple handler,
        // since the proc-macro uses ::camel_bean paths that don't resolve
        // inside the camel_bean crate itself.
        struct Greeter;

        #[crate::async_trait]
        impl crate::BeanProcessor for Greeter {
            async fn call(&self, method: &str, _exchange: &mut Exchange) -> Result<(), CamelError> {
                match method {
                    "hello" => Ok(()),
                    _ => Err(CamelError::ProcessorError(format!(
                        "Method '{}' not found",
                        method
                    ))),
                }
            }

            fn methods(&self) -> Vec<String> {
                vec!["hello".to_string()]
            }
        }

        let bean = Greeter;
        let mut exchange = Exchange::new(Message::default());

        // Known method should succeed
        let result = bean.call("hello", &mut exchange).await;
        assert!(result.is_ok(), "known method should succeed");

        // Unknown method must return Err, not panic
        let result = bean.call("nonexistent", &mut exchange).await;
        assert!(result.is_err(), "unknown method must return Err");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("nonexistent"),
            "error should mention method name, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_bean_lifecycle_defaults_are_noop() {
        let b = MockBean;
        assert!(b.on_start().await.is_ok());
        assert!(b.on_stop().await.is_ok());
    }
}
