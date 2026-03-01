use crate::exchange::Exchange;
use std::sync::Arc;

/// Predicate that determines whether an exchange passes the filter.
/// Returns `true` to forward the exchange into the filter body; `false` to skip it.
pub type FilterPredicate = Arc<dyn Fn(&Exchange) -> bool + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Exchange, Message};

    #[test]
    fn test_filter_predicate_is_callable() {
        let pred: FilterPredicate = Arc::new(|ex: &Exchange| ex.input.body.as_text().is_some());
        let ex = Exchange::new(Message::new("hello"));
        assert!(pred(&ex));
    }
}
