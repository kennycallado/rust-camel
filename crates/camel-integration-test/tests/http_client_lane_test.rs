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
        ..Default::default()
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

/// Same-key sends park in arrival order (bounded FIFO): three sends
/// under one lane key with no intervening receives, then three
/// receives resolve the oldest parked response first — wire order
/// A, B, C, never a later send's overwrite of the parked roundtrip.
#[tokio::test]
async fn same_key_sends_park_fifo() {
    // The delayed script holds every response past the send burst, so
    // all three roundtrips park before any of them resolves.
    let partner = HttpPartner::start(vec![
        ScriptedResponse {
            method: Some("POST".to_string()),
            path: Some("/orders".to_string()),
            body: b"a-response".to_vec(),
            delay: Some(Duration::from_millis(50)),
            ..Default::default()
        },
        ScriptedResponse {
            method: Some("POST".to_string()),
            path: Some("/orders".to_string()),
            body: b"b-response".to_vec(),
            delay: Some(Duration::from_millis(50)),
            ..Default::default()
        },
        ScriptedResponse {
            method: Some("POST".to_string()),
            path: Some("/orders".to_string()),
            body: b"c-response".to_vec(),
            delay: Some(Duration::from_millis(50)),
            ..Default::default()
        },
    ])
    .await
    .expect("partner binds 127.0.0.1:0");
    let lane_key = orders_uri(&partner);
    let router = PartnerRouter::new(BTreeMap::from([(
        lane_key.clone(),
        Box::new(partner) as Box<dyn PartnerAdapter>,
    )]));

    for body in ["a", "b", "c"] {
        router
            .send(&lane_key, &lane_key, send_msg(body))
            .await
            .expect("send parks its roundtrip on the lane key");
    }

    for expected in ["a-response", "b-response", "c-response"] {
        let response: IncomingMessage = router
            .receive(&lane_key, &lane_key, Duration::from_secs(5))
            .await
            .expect("the oldest parked roundtrip resolves first");
        assert_eq!(
            response.body,
            Value::String(expected.to_string()),
            "receives must drain the same-key park in wire order"
        );
    }
}

/// The client lane FIFO bound is an apparatus failure: when more
/// same-key sends are in flight than the bound, the overflowing send
/// fails with `TransportError::LaneFifoOverflow` naming the lane key
/// and the bound — never a silent overwrite of a parked roundtrip.
/// The refusal precedes the dial, so the refused send reaches no
/// wire and leaves no launched-path evidence behind.
#[tokio::test]
async fn lane_fifo_overflow_is_apparatus() {
    // Every response waits past the test window, so all earlier sends
    // stay in flight when the overflowing send launches.
    let partner = HttpPartner::start(vec![ScriptedResponse {
        method: Some("POST".to_string()),
        path: Some("/orders".to_string()),
        body: b"late".to_vec(),
        delay: Some(Duration::from_secs(30)),
        times: 65,
        ..Default::default()
    }])
    .await
    .expect("partner binds 127.0.0.1:0");
    let lane_key = orders_uri(&partner);
    let recorder = partner.recorder();
    let router = PartnerRouter::new(BTreeMap::from([(
        lane_key.clone(),
        Box::new(partner) as Box<dyn PartnerAdapter>,
    )]));

    for _ in 0..64 {
        router
            .send(&lane_key, &lane_key, send_msg("bulk"))
            .await
            .expect("the first 64 sends book inside the FIFO");
    }

    let overflow = router
        .send(&lane_key, &lane_key, send_msg("one-too-many"))
        .await;
    let Err(error) = &overflow else {
        panic!("the 65th send must fail at the transport, got {overflow:?}");
    };
    let TransportError::LaneFifoOverflow { lane_key, bound } = error else {
        panic!("the overflow must be the apparatus variant, got {error:?}");
    };
    assert_eq!(*bound, 64, "the overflow carries the FIFO bound");
    let rendered = error.to_string();
    assert!(
        rendered.contains(lane_key.as_str()),
        "the overflow names the lane key: {rendered}"
    );
    assert!(
        rendered.contains("64"),
        "the overflow names the bound: {rendered}"
    );
    // The 64 booked exchanges write their requests asynchronously;
    // wait until every one of them has reached the wire.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while recorder.recorded_requests().len() < 64 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the 64 booked sends must reach the wire"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // The refusal happens before the dial: the overflowing send
    // writes no request to the wire, so it leaves no launched-path
    // evidence for a later timeout to over-report.
    assert_eq!(
        recorder.recorded_requests().len(),
        64,
        "the refused send must reach no wire"
    );
}

/// The overflow diagnostic names the composite lane key with the path
/// half redacted (ADR-0051): a secret-marked query value on the
/// overflowed path renders masked, both in the key half and the path
/// half, so the apparatus failure never echoes the secret.
#[tokio::test]
async fn lane_fifo_overflow_redacts_secret_query() {
    // Permissive 200s: every booked exchange resolves quickly, and
    // the entries stay parked until a receive takes them, so the
    // FIFO fills from completed roundtrips alone.
    let partner = HttpPartner::start_permissive(200)
        .await
        .expect("partner binds 127.0.0.1:0");
    let uri = format!("http://{}/orders?authPassword=sekrit", partner.bound_addr());
    let router = PartnerRouter::new(BTreeMap::new());
    router.set_secret_query_keys(vec!["authPassword".to_string()]);

    for _ in 0..64 {
        router
            .send(&uri, &uri, send_msg("bulk"))
            .await
            .expect("the first 64 sends book inside the FIFO");
    }

    let overflow = router.send(&uri, &uri, send_msg("one-too-many")).await;
    let Err(error) = &overflow else {
        panic!("the 65th send must fail at the transport, got {overflow:?}");
    };
    let TransportError::LaneFifoOverflow { lane_key, .. } = error else {
        panic!("the overflow must be the apparatus variant, got {error:?}");
    };
    let rendered = error.to_string();
    assert!(
        rendered.contains("/orders"),
        "the overflow names the path: {rendered}"
    );
    assert!(
        lane_key.contains("authPassword=***") && rendered.contains("authPassword=***"),
        "both key and path halves mask the secret: {rendered}"
    );
    assert!(
        !rendered.contains("sekrit"),
        "the overflow never echoes the secret: {rendered}"
    );
}
