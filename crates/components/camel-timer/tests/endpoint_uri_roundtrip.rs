//! Round-trip contract for [`EndpointUri`]: the canonical string produced by
//! `to_canonical_string()` must re-parse through a real component parser into
//! exactly the config the equivalent literal query-string URI produces
//! (task 1.3 of the `add-endpoint-parameters` change).

use std::collections::BTreeMap;

use camel_api::EndpointUri;
use camel_component_api::UriConfig;
use camel_component_timer::TimerConfig;

/// Build an `EndpointUri` from a base URI plus a `parameters:` map, render the
/// canonical string, and verify the timer component's `from_uri` parses it to
/// the same config as the literal `timer:tick?period=2500` URI.
///
/// `TimerConfig` lacks `PartialEq`, so equality is checked on `Debug` output
/// (all fields are deterministic; no nondeterministic content).
#[test]
fn canonical_string_roundtrips_timer_from_uri() {
    let params = BTreeMap::from([("period".to_string(), "2500".to_string())]);
    let endpoint_uri = EndpointUri::try_from_uri_and_params("timer:tick", params)
        .expect("valid base URI and params must construct EndpointUri");
    let canonical = endpoint_uri.to_canonical_string();
    assert_eq!(canonical, "timer:tick?period=2500");

    let from_canonical = TimerConfig::from_uri(&canonical)
        .expect("canonical string must re-parse via TimerConfig::from_uri");
    let from_literal = TimerConfig::from_uri("timer:tick?period=2500")
        .expect("literal URI must parse via TimerConfig::from_uri");

    assert_eq!(
        from_canonical.period, from_literal.period,
        "period from canonical URI must equal period from literal URI"
    );
    assert_eq!(
        format!("{from_canonical:?}"),
        format!("{from_literal:?}"),
        "canonical URI and literal URI must produce identical TimerConfig values"
    );
}
