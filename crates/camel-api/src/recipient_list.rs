use std::sync::Arc;

use crate::Exchange;
use crate::error::CamelError;

pub type RecipientListExpression = Arc<dyn Fn(&Exchange) -> String + Send + Sync>;

/// Default cap on the number of recipients an expression may yield.
/// A million-token expression like `"a,b,c,…"` is truncated to this size
/// before any endpoint resolution. The cap is per-call; the operator may
/// override per-component. The H13 audit finding was that the recipient
/// list had no count cap; a malicious expression yielded millions of URIs
/// and exhausted memory.
pub const DEFAULT_MAX_RECIPIENTS: usize = 1_000;

#[derive(Clone)]
pub struct RecipientListConfig {
    pub expression: RecipientListExpression,
    pub delimiter: String,
    pub parallel: bool,
    pub parallel_limit: Option<usize>,
    pub stop_on_exception: bool,
    pub strategy: crate::MulticastStrategy,
    /// Maximum number of URIs the expression may produce. A larger list
    /// is truncated to `max_recipients` before endpoint resolution.
    /// Defaults to `DEFAULT_MAX_RECIPIENTS` (1_000). The processor MUST
    /// consult this cap, not the DSL author.
    pub max_recipients: usize,
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
            max_recipients: DEFAULT_MAX_RECIPIENTS,
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

    /// Override the per-call recipient count cap. Pass a value larger than
    /// `DEFAULT_MAX_RECIPIENTS` only when the operator has a real reason;
    /// the value is a hard ceiling, not a soft target.
    pub fn max_recipients(mut self, cap: usize) -> Self {
        self.max_recipients = cap;
        self
    }

    /// Validates the configuration.
    ///
    /// Returns `Err(CamelError::Config)` if:
    ///   - `parallel` is set with `parallel_limit == 0` (would deadlock / no progress)
    ///   - `max_recipients == 0` (denies every call; reject at config time)
    pub fn validate(&self) -> Result<(), CamelError> {
        if self.parallel && self.parallel_limit == Some(0) {
            return Err(CamelError::Config(
                "recipient_list parallel_limit must be > 0".to_string(),
            ));
        }
        if self.max_recipients == 0 {
            return Err(CamelError::Config(
                "recipient_list max_recipients must be > 0".to_string(),
            ));
        }
        Ok(())
    }
}

impl std::fmt::Debug for RecipientListConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecipientListConfig")
            .field("delimiter", &self.delimiter)
            .field("parallel", &self.parallel)
            .field("parallel_limit", &self.parallel_limit)
            .field("stop_on_exception", &self.stop_on_exception)
            .field("max_recipients", &self.max_recipients)
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

    /// H13: `RecipientListConfig` carries a `max_recipients` field with a
    /// sensible default. The default is 1_000 — a malicious expression
    /// yielding millions of URIs must be capped before it materializes a
    /// million endpoint resolutions.
    #[test]
    fn test_recipient_list_max_recipients_default() {
        let cfg = RecipientListConfig::new(noop_expr());
        assert_eq!(cfg.max_recipients, 1_000);
    }

    /// `validate` rejects a zero cap (would deny every call). The default
    /// and any positive override pass; zero fails closed.
    #[test]
    fn test_recipient_list_validate_rejects_zero_cap() {
        let cfg = RecipientListConfig::new(noop_expr()).max_recipients(0);
        assert!(cfg.validate().is_err());
    }
}
