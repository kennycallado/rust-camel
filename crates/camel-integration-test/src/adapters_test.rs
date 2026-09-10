//! Partner router address-math tests (ADR-0069 §5, §8).
//!
//! Unit-test module of the lib target, declared in `src/lib.rs` under
//! `#[cfg(test)]`. These tests exercise the router's pure addressing
//! helpers — [`PartnerRouter::wire_target`](crate::adapters::PartnerRouter::wire_target)
//! and [`PartnerRouter::lane_key_for`](crate::adapters::PartnerRouter::lane_key_for)
//! — through a stub adapter that declares a bound authority without
//! owning a listener, so no feature `http` and no wire are involved,
//! plus the scripted-queue arrival-stamp semantics (also no wire).
//! The runtime plain-string dial lives with the http partner tests.

use std::collections::BTreeMap;
use std::time::Duration;

use camel_api::Value;
use futures::future::BoxFuture;

use crate::adapters::{
    FakeAdapter, IncomingMessage, PartnerAdapter, PartnerRouter, ReceiveError, TransportError,
};

/// A partner-shaped stub: declares a bound authority without owning a
/// listener, so router address math is testable without the wire.
struct BoundStub {
    /// The authority the stub reports; `None` for a non-http adapter.
    authority: Option<String>,
}

impl PartnerAdapter for BoundStub {
    fn receive<'a>(
        &'a self,
        _lane_key: &'a str,
        _source_uri: &'a str,
        _deadline: Duration,
    ) -> BoxFuture<'a, Result<IncomingMessage, ReceiveError>> {
        Box::pin(async {
            Err(ReceiveError::Transport(TransportError::Other {
                message: "the stub never delivers".to_string(),
            }))
        })
    }

    fn bound_authority(&self) -> Option<String> {
        self.authority.clone()
    }
}

/// A router over stub adapters, keyed as declared.
fn router_with(entries: &[(&str, &str)]) -> PartnerRouter {
    PartnerRouter::new(
        entries
            .iter()
            .map(|(key, authority)| {
                (
                    key.to_string(),
                    Box::new(BoundStub {
                        authority: Some(authority.to_string()),
                    }) as Box<dyn PartnerAdapter>,
                )
            })
            .collect(),
    )
}

/// A partner registered under the harness-declared `:0` form: the
/// wire target rewrites only the authority to the bound address,
/// preserving the interpolated path and query.
#[test]
fn wire_target_rewrites_authority_only() {
    let router = router_with(&[("http://127.0.0.1:0/orders", "127.0.0.1:45678")]);
    assert_eq!(
        router.wire_target("http://127.0.0.1:0/orders", "http://127.0.0.1:0/orders?x=1"),
        Some("http://127.0.0.1:45678/orders?x=1".to_string())
    );
}

/// A declared key that names no registered partner and whose
/// interpolated authority matches no bound partner resolves to no
/// wire target: the caller dials the interpolated URI literally.
#[test]
fn wire_target_passthrough_when_not_partner() {
    let router = router_with(&[("http://127.0.0.1:0/orders", "127.0.0.1:45678")]);
    assert_eq!(
        router.wire_target("http://10.9.8.7:1/nowhere", "http://10.9.8.7:1/nowhere"),
        None
    );
}

/// A dynamic declared key (`http://${P}/orders`, not registered)
/// whose interpolated authority equals a bound partner's authority
/// resolves to that partner's authority rewrite, path preserved.
#[test]
fn wire_target_matches_bound_authority() {
    let router = router_with(&[("http://127.0.0.1:0/orders", "127.0.0.1:45678")]);
    assert_eq!(
        router.wire_target("http://${PARTNER}/orders", "http://127.0.0.1:45678/orders"),
        Some("http://127.0.0.1:45678/orders".to_string())
    );
}

/// The declared string names a registered partner key: the lane key
/// is that key unchanged, whatever the interpolated address.
#[test]
fn lane_key_for_prefers_declared_key() {
    let router = router_with(&[("http://127.0.0.1:0/orders", "127.0.0.1:45678")]);
    assert_eq!(
        router.lane_key_for("http://127.0.0.1:0/orders", "http://127.0.0.1:45678/orders"),
        Some("http://127.0.0.1:0/orders".to_string())
    );
}

/// A dynamic declared key resolves through the interpolated
/// authority to the registered key of the partner bound there.
#[test]
fn lane_key_for_resolves_dynamic_ref() {
    let router = router_with(&[("http://127.0.0.1:0/orders", "127.0.0.1:45678")]);
    assert_eq!(
        router.lane_key_for("http://${PARTNER}/orders", "http://127.0.0.1:45678/orders"),
        Some("http://127.0.0.1:0/orders".to_string())
    );
}

// -------------------------------------------------------------------------
// Wire-arrival instants (ADR-0069 §5: the wire is the proof)
// -------------------------------------------------------------------------

/// A scripted message's `arrival` instant is stamped when the message
/// is constructed (the wire moment), not when a receive action
/// consumes it: after a deliberate sleep between construction and
/// receive, the arrival age covers the whole sleep.
#[tokio::test]
async fn incoming_message_carries_arrival_instant() {
    let fake = FakeAdapter::scripted(vec![IncomingMessage {
        body: Value::String("queued".to_string()),
        headers: BTreeMap::new(),
        status: None,
        method: None,
        path: None,
        arrival: std::time::Instant::now(),
    }]);
    let router = PartnerRouter::new(BTreeMap::from([(
        "partner://fake".to_string(),
        Box::new(fake) as Box<dyn PartnerAdapter>,
    )]));
    tokio::time::sleep(Duration::from_millis(50)).await;
    let message = router
        .receive("partner://fake", "partner://fake", Duration::from_secs(1))
        .await
        .expect("the scripted message must arrive");
    assert!(
        message.arrival.elapsed() >= Duration::from_millis(50),
        "arrival must be stamped at construction (the wire), not at \
         receive consumption: elapsed {:?}",
        message.arrival.elapsed()
    );
    assert!(
        message.arrival.elapsed() < Duration::from_secs(5),
        "arrival sanity bound: the message was stamped milliseconds ago"
    );
}

// ---------------------------------------------------------------------------
// redact_wire_path (ADR-0051 positive secret rule, rc-dhkeo)
// ---------------------------------------------------------------------------

#[test]
fn redact_wire_path_matches_secret_keys_case_insensitively() {
    let secrets = vec!["authpassword".to_string()];
    assert_eq!(
        crate::adapters::redact_wire_path("/secure?AuthPassword=hunter2&ok=1", &secrets),
        "/secure?AuthPassword=***&ok=1",
        "mixed-case secret key masks; the raw key span stays as authored"
    );
    assert_eq!(
        crate::adapters::redact_wire_path("/secure?AUTHPASSWORD=hunter2", &secrets),
        "/secure?AUTHPASSWORD=***",
        "uppercase secret key masks too"
    );
    assert_eq!(
        crate::adapters::redact_wire_path("/secure?authpassword=hunter2", &secrets),
        "/secure?authpassword=***",
        "exact-case match keeps working"
    );
}

#[test]
fn redact_wire_path_non_secret_keys_untouched() {
    let secrets = vec!["authpassword".to_string()];
    assert_eq!(
        crate::adapters::redact_wire_path("/p?auth=x&token=y", &secrets),
        "/p?auth=x&token=y",
        "unrelated keys ride unmasked — no over-redaction"
    );
}
