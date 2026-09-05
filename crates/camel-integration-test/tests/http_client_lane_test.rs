//! Client-lane failure semantics (ADR-0069 section 5): a pre-wire
//! send failure (bad endpoint, connect refused) is observed on the
//! send call itself and leaves no lane entry behind, while a
//! post-connect failure parks on the lane key for the scenario's
//! receive. The replace-then-fail race contract itself is the unit
//! test on the ClientLane module (no timing); these smoke tests cover
//! the two observable behaviors, sequentially and without sleeps.
#![cfg(feature = "http")]

use std::collections::BTreeMap;
use std::time::Duration;

use camel_api::Value;
use camel_integration_test::adapters::{
    IncomingMessage, OutgoingMessage, ReceiveError, TransportError,
};
use camel_integration_test::{HttpPartner, PartnerAdapter, PartnerRouter, ScriptedResponse};

/// One client-role send message: POST with a string body.
fn send_msg(body: &str) -> OutgoingMessage {
    OutgoingMessage {
        body: Value::String(body.to_string()),
        headers: BTreeMap::new(),
        method: "POST".to_string(),
    }
}

/// A partner serving one scripted response on `/orders`.
async fn orders_partner(body: &str) -> HttpPartner {
    HttpPartner::start(vec![ScriptedResponse {
        method: Some("POST".to_string()),
        path: Some("/orders".to_string()),
        status: 200,
        headers: BTreeMap::new(),
        body: body.as_bytes().to_vec(),
    }])
    .await
    .expect("partner binds 127.0.0.1:0")
}

/// The roundtrip target URI for the partner's `/orders` endpoint.
fn orders_uri(partner: &HttpPartner) -> String {
    format!("http://{}/orders", partner.bound_addr())
}

/// A pre-wire send failure (connect refused) is observed on the send
/// call itself and inserts no lane entry, so it cannot poison a later
/// receive: the next send under the same lane key roundtrips.
#[tokio::test]
async fn failed_send_does_not_poison_later_receive() {
    let partner = orders_partner("b-response").await;
    // A routable (non-port-zero) lane key: the router dials the
    // interpolated URI literally, so the send picks the wire target.
    let lane_key = orders_uri(&partner);
    let bound = partner.bound_addr();
    let router = PartnerRouter::new(BTreeMap::from([(
        lane_key.clone(),
        Box::new(partner) as Box<dyn PartnerAdapter>,
    )]));

    // The dead wire target: a bound port whose listener is gone.
    let dead = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("dead listener binds");
    let dead_addr = dead.local_addr().expect("dead listener has an address");
    drop(dead);

    // Send A fails inline at the transport — the send call itself
    // observes the connect refusal.
    let failed = router
        .send(
            &lane_key,
            &format!("http://{dead_addr}/orders"),
            send_msg("a"),
        )
        .await;
    assert!(
        matches!(&failed, Err(TransportError::Other { message }) if message.contains("connect")),
        "send A must fail inline at the transport, got {failed:?}"
    );

    // No lane entry exists: a client-role receive finds nothing parked
    // (a parked entry would surface its transport error immediately)
    // and falls through to the empty server-role lane, which times out.
    let empty = router
        .receive(&lane_key, &lane_key, Duration::from_millis(200))
        .await;
    assert!(
        matches!(empty, Err(ReceiveError::Timeout(_))),
        "no lane entry may exist after a pre-wire failure, got {empty:?}"
    );

    // Send B under the same lane key roundtrips against the live
    // partner, and the receive on that key consumes B's response.
    router
        .send(&lane_key, &format!("http://{bound}/orders"), send_msg("b"))
        .await
        .expect("send B dials the live partner");
    let response: IncomingMessage = router
        .receive(&lane_key, &lane_key, Duration::from_secs(5))
        .await
        .expect("B's roundtrip is parked on the lane key");
    assert_eq!(response.body, Value::String("b-response".to_string()));
}

/// A peer that accepts the connection and drops it immediately lets
/// the dial succeed and kills the exchange post-connect: the send
/// books the entry, and the receive surfaces the parked transport
/// error (the `fail_lane_entry` true path).
#[tokio::test]
async fn post_connect_failure_still_parks() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("listener binds");
    let addr = listener.local_addr().expect("listener has an address");
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            drop(stream);
        }
    });

    // No adapters: a plain-string reference dials its literal URI, and
    // the lane key is the declared string itself.
    let router = PartnerRouter::new(BTreeMap::new());
    let lane_key = format!("http://{addr}/orders");

    router
        .send(&lane_key, &lane_key, send_msg("a"))
        .await
        .expect("the dial succeeds against the accepting listener");

    let parked = router
        .receive(&lane_key, &lane_key, Duration::from_secs(5))
        .await;
    assert!(
        matches!(&parked, Err(ReceiveError::Transport(_))),
        "the post-connect failure must park on the lane key, got {parked:?}"
    );
}
