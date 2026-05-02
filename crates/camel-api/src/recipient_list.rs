use std::sync::Arc;

use crate::Exchange;

pub type RecipientListExpression = Arc<dyn Fn(&Exchange) -> String + Send + Sync>;

#[derive(Clone)]
pub struct RecipientListConfig {
    pub expression: RecipientListExpression,
    pub delimiter: String,
    pub parallel: bool,
    pub parallel_limit: Option<usize>,
    pub stop_on_exception: bool,
    pub strategy: crate::MulticastStrategy,
}

impl RecipientListConfig {
    pub fn new(expression: RecipientListExpression) -> Self {
        Self {
            expression,
            delimiter: ",".to_string(),
            parallel: false,
            parallel_limit: None,
            stop_on_exception: false,
            strategy: crate::MulticastStrategy::default(),
        }
    }

    pub fn delimiter(mut self, d: impl Into<String>) -> Self {
        self.delimiter = d.into();
        self
    }

    pub fn parallel(mut self, parallel: bool) -> Self {
        self.parallel = parallel;
        self
    }

    pub fn parallel_limit(mut self, limit: usize) -> Self {
        self.parallel_limit = Some(limit);
        self
    }

    pub fn stop_on_exception(mut self, stop: bool) -> Self {
        self.stop_on_exception = stop;
        self
    }

    pub fn strategy(mut self, strategy: crate::MulticastStrategy) -> Self {
        self.strategy = strategy;
        self
    }
}

impl std::fmt::Debug for RecipientListConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecipientListConfig")
            .field("delimiter", &self.delimiter)
            .field("parallel", &self.parallel)
            .field("parallel_limit", &self.parallel_limit)
            .field("stop_on_exception", &self.stop_on_exception)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn noop_expr() -> RecipientListExpression {
        Arc::new(|_| String::new())
    }

    #[test]
    fn new_has_defaults() {
        let cfg = RecipientListConfig::new(noop_expr());
        assert_eq!(cfg.delimiter, ",");
        assert!(!cfg.parallel);
        assert!(cfg.parallel_limit.is_none());
        assert!(!cfg.stop_on_exception);
    }

    #[test]
    fn builder_chaining() {
        let cfg = RecipientListConfig::new(noop_expr())
            .delimiter(";")
            .parallel(true)
            .parallel_limit(4)
            .stop_on_exception(true)
            .strategy(crate::MulticastStrategy::CollectAll);
        assert_eq!(cfg.delimiter, ";");
        assert!(cfg.parallel);
        assert_eq!(cfg.parallel_limit, Some(4));
        assert!(cfg.stop_on_exception);
    }

    #[test]
    fn clone_preserves_values() {
        let cfg = RecipientListConfig::new(noop_expr())
            .delimiter("|")
            .parallel(true);
        let cloned = cfg.clone();
        assert_eq!(cloned.delimiter, "|");
        assert!(cloned.parallel);
    }

    #[test]
    fn debug_format() {
        let cfg = RecipientListConfig::new(noop_expr());
        let debug = format!("{cfg:?}");
        assert!(debug.contains("RecipientListConfig"));
        assert!(debug.contains("delimiter"));
    }
}
