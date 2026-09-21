//! Shared cfg(test) support for the in-crate test modules
//! (bd rc-kkznl): the single-entry [`PartnerRouter`] construction
//! the test modules previously duplicated module-privately
//! (`http_partner_test`, `runner_test`) or inlined per site — the
//! keyed-single-adapter incantation must not drift between copies.

use std::collections::BTreeMap;

use crate::adapters::{PartnerAdapter, PartnerRouter};

/// A single-entry router over one adapter, keyed by endpoint URI.
pub(crate) fn router_for<A: PartnerAdapter + 'static>(uri: &str, adapter: A) -> PartnerRouter {
    PartnerRouter::new(BTreeMap::from([(
        uri.to_string(),
        Box::new(adapter) as Box<dyn PartnerAdapter>,
    )]))
}

#[cfg(test)]
mod tests {
    use super::router_for;
    use crate::adapters::FakeAdapter;

    /// The router keys the single adapter under the given URI: a
    /// receive under that declared key reads its own lane.
    #[test]
    fn router_for_keys_the_single_adapter() {
        let router = router_for("partner://fake", FakeAdapter::scripted(Vec::new()));
        assert_eq!(
            router.lane_key_for("partner://fake", "partner://fake"),
            Some("partner://fake".to_string())
        );
    }
}
