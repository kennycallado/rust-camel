use crate::{BeanError, BeanProcessor};
use camel_api::{CamelError, Exchange};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, warn};

pub struct BeanRegistry {
    beans: HashMap<String, Arc<dyn BeanProcessor>>,
}

impl BeanRegistry {
    pub fn new() -> Self {
        Self {
            beans: HashMap::new(),
        }
    }

    pub fn register<B>(&mut self, name: impl Into<String>, bean: B) -> Result<(), BeanError>
    where
        B: BeanProcessor + 'static,
    {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(BeanError::InvalidName(name));
        }
        if self.beans.contains_key(&name) {
            warn!(bean_name = %name, "bean already registered");
            return Err(BeanError::DuplicateName(name));
        }
        self.beans.insert(name.clone(), Arc::new(bean));
        debug!(bean_name = %name, "bean registered");
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn BeanProcessor>> {
        self.beans.get(name).cloned()
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

        bean.call(method, exchange).await.map_err(|err| {
            warn!(bean_name = %bean_name, method = %method, error = %err, "bean invocation failed");
            err
        })
    }

    pub fn len(&self) -> usize {
        self.beans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.beans.is_empty()
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
        let mut registry = BeanRegistry::new();
        registry.register("mock", MockBean).unwrap();

        assert!(registry.get("mock").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_len_and_is_empty() {
        let mut registry = BeanRegistry::new();
        assert!(registry.is_empty());

        registry.register("mock", MockBean).unwrap();
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[test]
    fn test_register_same_name_rejects() {
        let mut registry = BeanRegistry::new();
        registry.register("dup", MockBean).unwrap();
        let result = registry.register("dup", ErrorBean);
        assert!(result.is_err());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_register_duplicate_returns_error() {
        let mut registry = BeanRegistry::new();
        registry.register("dup", MockBean).unwrap();
        let err = registry.register("dup", ErrorBean).unwrap_err();
        assert!(matches!(err, BeanError::DuplicateName(_)));
    }

    #[tokio::test]
    async fn test_invoke_success() {
        let mut registry = BeanRegistry::new();
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
        let mut registry = BeanRegistry::new();
        registry.register("mock", MockBean).unwrap();

        let mut exchange = Exchange::new(Message::default());
        let result = registry.invoke("mock", "unknown", &mut exchange).await;

        assert!(result.is_err());
    }

    #[test]
    fn test_register_empty_name_rejected() {
        let mut registry = BeanRegistry::new();
        let result = registry.register("", MockBean);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), BeanError::InvalidName(_)));
    }

    #[test]
    fn test_register_whitespace_name_rejected() {
        let mut registry = BeanRegistry::new();
        let result = registry.register("   ", MockBean);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), BeanError::InvalidName(_)));
    }

    #[tokio::test]
    async fn test_invoke_propagates_bean_error() {
        let mut registry = BeanRegistry::new();
        registry.register("err", ErrorBean).unwrap();

        let mut exchange = Exchange::new(Message::default());
        let result = registry.invoke("err", "process", &mut exchange).await;

        assert!(matches!(result, Err(CamelError::ProcessorError(_))));
    }
}
