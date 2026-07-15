use crate::exchange::Exchange;
use std::sync::Arc;

/// Predicate that determines whether an exchange passes the filter.
/// Returns `true` to forward the exchange into the filter body; `false` to skip it.
///
/// This is a newtype around `Arc<dyn Fn(&Exchange) -> bool + Send + Sync>` so that
/// it can implement `Debug` (used by `#[derive(Debug)]` on `BuilderStep` and friends).
/// The `Deref` impl keeps the call-site ergonomic: `predicate(&exchange)` still works
/// because `FilterPredicate` derefs to the inner `dyn Fn`.
///
/// Pre-v1.0: this used to be a type alias. Converted to a newtype (H2) so the
/// containing structs (`WhenStep`, etc.) can be `#[derive(Debug)]`.
pub struct FilterPredicate(pub Arc<dyn Fn(&Exchange) -> bool + Send + Sync>);

impl FilterPredicate {
    /// Create a new `FilterPredicate` from a closure or function pointer.
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&Exchange) -> bool + Send + Sync + 'static,
    {
        FilterPredicate(Arc::new(f))
    }
}

impl Clone for FilterPredicate {
    fn clone(&self) -> Self {
        FilterPredicate(Arc::clone(&self.0))
    }
}

impl std::fmt::Debug for FilterPredicate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FilterPredicate(..)")
    }
}

impl std::ops::Deref for FilterPredicate {
    type Target = dyn Fn(&Exchange) -> bool + Send + Sync;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Exchange, Message};

    #[test]
    fn test_filter_predicate_is_callable() {
        let pred = FilterPredicate::new(|ex: &Exchange| ex.input.body.as_text().is_some());
        let ex = Exchange::new(Message::new("hello"));
        assert!(pred(&ex));
    }

    #[test]
    fn test_filter_predicate_debug_is_redacted() {
        let pred = FilterPredicate::new(|_: &Exchange| true);
        assert_eq!(format!("{pred:?}"), "FilterPredicate(..)");
    }

    #[test]
    fn test_filter_predicate_clone_shares_arc() {
        let pred = FilterPredicate::new(|_: &Exchange| true);
        let cloned = pred.clone();
        assert!(matches!(cloned, FilterPredicate(_)));
    }
}
