use crate::{BeanError, BeanProcessor};
use camel_api::{CamelError, Exchange};
use std::collections::HashMap;
use std::sync::Arc;

pub struct BeanRegistry {
    beans: HashMap<String, Arc<dyn BeanProcessor>>,
}

impl BeanRegistry {
    pub fn new() -> Self {
        Self {
            beans: HashMap::new(),
        }
    }

    pub fn register<B>(&mut self, name: impl Into<String>, bean: B)
    where
        B: BeanProcessor + 'static,
    {
        self.beans.insert(name.into(), Arc::new(bean));
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
        let bean = self
            .get(bean_name)
            .ok_or_else(|| BeanError::NotFound(bean_name.to_string()))?;

        if !bean.methods().contains(&method) {
            return Err(BeanError::MethodNotFound(method.to_string()).into());
        }

        bean.call(method, exchange).await
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

    #[async_trait]
    impl BeanProcessor for MockBean {
        async fn call(&self, method: &str, _exchange: &mut Exchange) -> Result<(), CamelError> {
            match method {
                "process" => Ok(()),
                _ => Err(BeanError::MethodNotFound(method.to_string()).into()),
            }
        }

        fn methods(&self) -> Vec<&'static str> {
            vec!["process"]
        }
    }

    #[test]
    fn test_register_and_get() {
        let mut registry = BeanRegistry::new();
        registry.register("mock", MockBean);

        assert!(registry.get("mock").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_len_and_is_empty() {
        let mut registry = BeanRegistry::new();
        assert!(registry.is_empty());

        registry.register("mock", MockBean);
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[tokio::test]
    async fn test_invoke_success() {
        let mut registry = BeanRegistry::new();
        registry.register("mock", MockBean);

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
        registry.register("mock", MockBean);

        let mut exchange = Exchange::new(Message::default());
        let result = registry.invoke("mock", "unknown", &mut exchange).await;

        assert!(result.is_err());
    }
}
