//! HTTP partner adapter tests (ADR-0069 §5, §8).
//!
//! Unit-test module of the lib target, declared in `src/lib.rs`
//! under `#[cfg(all(test, feature = "http"))]`. The partner plays
//! both wire roles against itself or a second partner on loopback:
//! the listener records what actually reached the wire, and the
//! client role returns the response for partner-side normative
//! assertions. The listener binds `127.0.0.1:0` only; no free-port
//! probing (ADR-0069 §8).

use std::collections::BTreeMap;
use std::io;
use std::time::Duration;

use camel_api::Value;
use tokio::net::TcpStream;

use crate::adapters::http::{HttpPartner, ScriptedResponse};
use crate::adapters::{OutgoingMessage, PartnerAdapter, PartnerRouter};
use crate::document::PartnerFault;

/// The permissive default is non-consuming: every request no scripted
/// response matches is answered with the permissive status for the
/// partner's whole lifetime. The CLI full-boot path relies on this —
/// a scripted one-shot entry would turn the second request to the
/// same harness endpoint into the unmatched-500 scripting gap its
/// author never scripted.
#[tokio::test]
async fn permissive_default_serves_every_unmatched_request() {
    let server = HttpPartner::start_permissive(200)
        .await
        .expect("server partner must bind 127.0.0.1:0");
    let target = format!("http://{}/hook", server.bound_addr());
    let client = HttpPartner::start(Vec::new())
        .await
        .expect("client partner must bind 127.0.0.1:0");
    let router = router_for(&target, client);

    // Two sequential exchanges against the same listener: the client
    // role parks one response at a time, so send/receive in rounds.
    for round in 0..2 {
        router
            .send(
                &target,
                &target,
                OutgoingMessage {
                    body: Value::String(format!("req-{round}")),
                    headers: BTreeMap::new(),
                    method: "POST".to_string(),
                },
            )
            .await
            .expect("send must perform the real HTTP roundtrip");
        let response = router
            .receive(&target, &target, Duration::from_secs(5))
            .await
            .expect("the permissive response must arrive");
        assert_eq!(
            response.status,
            Some(200),
            "round {round} must serve the permissive default, got {response:?}"
        );
    }
    assert_eq!(
        server.recorder().recorded_requests().len(),
        2,
        "both requests must reach the wire"
    );
}

/// A single-entry router over one adapter, keyed by endpoint URI.
fn router_for(uri: &str, adapter: HttpPartner) -> PartnerRouter {
    PartnerRouter::new(BTreeMap::from([(
        uri.to_string(),
        Box::new(adapter) as Box<dyn PartnerAdapter>,
    )]))
}

/// The outbound partner records the wire request: method, path,
/// headers, and exact body bytes that reached the listener.
#[tokio::test]
async fn outbound_partner_records_wire_request() {
    let partner = HttpPartner::start(vec![ScriptedResponse {
        method: Some("POST".to_string()),
        path: Some("/orders".to_string()),
        status: 201,
        headers: BTreeMap::from([("X-Accepted".to_string(), "yes".to_string())]),
        body: b"accepted".to_vec(),
        ..Default::default()
    }])
    .await
    .expect("partner must bind 127.0.0.1:0");
    let recorder = partner.recorder();
    let uri = format!("http://{}/orders", partner.bound_addr());
    let router = router_for(&uri, partner);

    router
        .send(
            &uri,
            &uri,
            OutgoingMessage {
                body: Value::String("payload-bytes".to_string()),
                headers: BTreeMap::from([
                    ("X-Trace".to_string(), Value::String("t-42".to_string())),
                    (
                        "Content-Type".to_string(),
                        Value::String("text/plain".to_string()),
                    ),
                ]),
                method: "POST".to_string(),
            },
        )
        .await
        .expect("send must perform the real HTTP roundtrip on loopback");

    // The receive completes the request/response pair and therefore
    // also synchronizes the server-side recording.
    let response = router
        .receive(&uri, &uri, Duration::from_secs(5))
        .await
        .expect("scripted response must arrive");
    assert_eq!(response.status, Some(201));

    let recorded = recorder.recorded_requests();
    assert_eq!(
        recorded.len(),
        1,
        "exactly one wire request, got {recorded:?}"
    );
    assert_eq!(recorded[0].method, "POST");
    assert_eq!(recorded[0].path, "/orders");
    assert_eq!(recorded[0].body, b"payload-bytes".to_vec());
    // Header names arrive lowercased (hyper normalization); the
    // declared values must round-trip exactly.
    assert_eq!(
        recorded[0].headers.get("x-trace"),
        Some(&"t-42".to_string()),
        "declared headers must reach the wire exactly: {:?}",
        recorded[0].headers
    );
    assert_eq!(
        recorded[0].headers.get("content-type"),
        Some(&"text/plain".to_string())
    );
}

/// The inbound client role returns the response object with status,
/// headers, and body for validation.
#[tokio::test]
async fn inbound_client_receives_status_headers_body() {
    // The far-side partner: a local listener serving a canned
    // response. The client partner talks to it over a real
    // connection; its own listener stays unused.
    let server = HttpPartner::start(vec![ScriptedResponse {
        method: None,
        path: Some("/canned".to_string()),
        status: 200,
        headers: BTreeMap::from([("X-Canned".to_string(), "yes".to_string())]),
        body: b"canned-body".to_vec(),
        ..Default::default()
    }])
    .await
    .expect("server partner must bind 127.0.0.1:0");
    let target = format!("http://{}/canned", server.bound_addr());
    let client = HttpPartner::start(Vec::new())
        .await
        .expect("client partner must bind 127.0.0.1:0");
    let router = router_for(&target, client);

    router
        .send(
            &target,
            &target,
            OutgoingMessage {
                body: Value::Null,
                headers: BTreeMap::new(),
                method: "GET".to_string(),
            },
        )
        .await
        .expect("send must reach the far-side listener");

    let response = router
        .receive(&target, &target, Duration::from_secs(5))
        .await
        .expect("canned response must arrive");
    assert_eq!(response.status, Some(200), "status must be exposed");
    assert_eq!(
        response.headers.get("x-canned"),
        Some(&Value::String("yes".to_string())),
        "headers must be exposed: {:?}",
        response.headers
    );
    assert_eq!(
        response.body,
        Value::String("canned-body".to_string()),
        "body must be exposed"
    );
}

/// A receive with no send in flight to that endpoint falls through to
/// the server role: the wait is bounded by the deadline and reports a
/// verdict-class timeout. Nothing is provably dead (an arrival can come
/// at any moment), so the failure is a deadline wait, never immediate.
#[tokio::test]
async fn receive_without_send_times_out() {
    let partner = HttpPartner::start(Vec::new())
        .await
        .expect("partner must bind 127.0.0.1:0");
    let uri = format!("http://{}/never", partner.bound_addr());
    let router = router_for(&uri, partner);
    let started = std::time::Instant::now();
    let failure = router
        .receive(&uri, &uri, Duration::from_millis(200))
        .await
        .expect_err("no arrival must time out");
    assert!(
        matches!(failure, crate::adapters::ReceiveError::Timeout(_)),
        "expected Timeout, got {failure:?}"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(150),
        "the server-role wait must honor the deadline, not fail early"
    );
}

// -------------------------------------------------------------------------
// Outbound arrival queue (review amendment: listener arrivals reach
// `receive` as `IncomingMessage`s)
// -------------------------------------------------------------------------

/// A request that reaches the partner's listener arrives at `receive`
/// as an `IncomingMessage`: the request line (method, path), the
/// request headers, and the request body — `status` is `None` because
/// requests carry no status (the scripted response status is
/// harness-known).
#[tokio::test]
async fn outbound_arrival_reaches_receive() {
    let server = HttpPartner::start(vec![ScriptedResponse {
        method: Some("POST".to_string()),
        path: Some("/orders".to_string()),
        status: 200,
        headers: BTreeMap::new(),
        body: b"ok".to_vec(),
        ..Default::default()
    }])
    .await
    .expect("server partner must bind 127.0.0.1:0");
    let target = format!("http://{}/orders", server.bound_addr());
    // A second partner plays the system under test's HTTP client: its
    // client role performs the real request into the server's listener.
    let sut = HttpPartner::start(Vec::new())
        .await
        .expect("sut client partner must bind 127.0.0.1:0");
    let send_router = router_for(&target, sut);
    send_router
        .send(
            &target,
            &target,
            OutgoingMessage {
                body: Value::String("wire-body".to_string()),
                headers: BTreeMap::from([(
                    "Content-Type".to_string(),
                    Value::String("text/plain".to_string()),
                )]),
                method: "POST".to_string(),
            },
        )
        .await
        .expect("sut send must reach the server listener");
    // Drain the parked client response so the send completes.
    let _ = send_router
        .receive(&target, &target, Duration::from_secs(5))
        .await;

    let server_router = router_for(&target, server);
    let arrival = server_router
        .receive(&target, &target, Duration::from_secs(5))
        .await
        .expect("the wire request must arrive for validation");
    assert_eq!(arrival.method.as_deref(), Some("POST"));
    assert_eq!(arrival.path.as_deref(), Some("/orders"));
    assert_eq!(arrival.status, None, "requests carry no status");
    assert_eq!(arrival.body, Value::String("wire-body".to_string()));
    assert_eq!(
        arrival.headers.get("content-type"),
        Some(&Value::String("text/plain".to_string())),
        "wire headers arrive lowercase (hyper normalization): {:?}",
        arrival.headers
    );
}

/// The arrival instant is stamped when the listener finishes receiving
/// the request (the enqueue point), not when a receive action consumes
/// it: the request sits queued while the scenario sleeps, and the late
/// receive still measures the arrival age from the wire moment.
#[tokio::test]
async fn server_arrival_stamp_predates_consumption() {
    let server = HttpPartner::start(vec![ScriptedResponse {
        method: Some("POST".to_string()),
        path: Some("/orders".to_string()),
        status: 200,
        headers: BTreeMap::new(),
        body: b"ok".to_vec(),
        ..Default::default()
    }])
    .await
    .expect("server partner must bind 127.0.0.1:0");
    let target = format!("http://{}/orders", server.bound_addr());
    // A second partner plays the system under test's HTTP client: its
    // client role performs the real request into the server's listener.
    let sut = HttpPartner::start(Vec::new())
        .await
        .expect("sut client partner must bind 127.0.0.1:0");
    let send_router = router_for(&target, sut);
    send_router
        .send(
            &target,
            &target,
            OutgoingMessage {
                body: Value::String("wire-body".to_string()),
                headers: BTreeMap::new(),
                method: "POST".to_string(),
            },
        )
        .await
        .expect("sut send must reach the server listener");
    // Drain the parked client response so the send completes.
    let _ = send_router
        .receive(&target, &target, Duration::from_secs(5))
        .await;

    // The request is already queued on the partner; the late receive
    // must measure from the wire moment, not from consumption.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let server_router = router_for(&target, server);
    let arrival = server_router
        .receive(&target, &target, Duration::from_secs(5))
        .await
        .expect("the wire request must arrive for validation");
    assert!(
        arrival.arrival.elapsed() >= Duration::from_millis(100),
        "the arrival stamp must be taken at enqueue (the transport \
         receive), before the sleep: elapsed {:?}",
        arrival.arrival.elapsed()
    );
}

/// Arrivals queue per endpoint while the scenario has not received:
/// two requests received in arrival order, one `receive` per arrival.
#[tokio::test]
async fn arrivals_queue_per_endpoint() {
    let server = HttpPartner::start(vec![
        ScriptedResponse {
            method: Some("POST".to_string()),
            path: Some("/orders".to_string()),
            status: 200,
            headers: BTreeMap::new(),
            body: b"ok".to_vec(),
            ..Default::default()
        };
        2
    ])
    .await
    .expect("server partner must bind 127.0.0.1:0");
    let target = format!("http://{}/orders", server.bound_addr());
    let sut = HttpPartner::start(Vec::new())
        .await
        .expect("sut client partner must bind 127.0.0.1:0");
    {
        let send_router = router_for(&target, sut);
        for body in ["first-body", "second-body"] {
            send_router
                .send(
                    &target,
                    &target,
                    OutgoingMessage {
                        body: Value::String(body.to_string()),
                        headers: BTreeMap::new(),
                        method: "POST".to_string(),
                    },
                )
                .await
                .expect("sut send must reach the server listener");
            // Drain the parked response so the send's slot is free.
            let _ = send_router
                .receive(&target, &target, Duration::from_secs(5))
                .await;
        }
    }

    let server_router = router_for(&target, server);
    let first = server_router
        .receive(&target, &target, Duration::from_secs(5))
        .await
        .expect("first arrival must dequeue");
    let second = server_router
        .receive(&target, &target, Duration::from_secs(5))
        .await
        .expect("second arrival must dequeue");
    assert_eq!(first.body, Value::String("first-body".to_string()));
    assert_eq!(second.body, Value::String("second-body".to_string()));
}

// -------------------------------------------------------------------------
// Plain-string dispatch (case c: no partner involved)
// -------------------------------------------------------------------------

/// Whether the buffered bytes carry a complete HTTP/1.1 request head
/// (the blank line after the headers).
fn head_is_complete(bytes: &[u8]) -> bool {
    bytes.windows(4).any(|window| window == b"\r\n\r\n")
}

/// Case (c) of the router's http dispatch: a plain-string endpoint
/// reference with no registered partner dials its literal URI through
/// the router's own client lane, and a receive on the same declared
/// string returns the parked roundtrip. The far side is a raw
/// `TcpListener` (no partner), so the only way the request line below
/// can be recorded is a literal dial of the interpolated address.
#[tokio::test]
async fn plain_string_send_dials_literal_without_partner() {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the raw far-side listener must bind 127.0.0.1:0");
    let port = listener
        .local_addr()
        .expect("the listener must report its bound address")
        .port();
    let served = tokio::spawn(async move {
        let (mut stream, _) = listener
            .accept()
            .await
            .expect("the literal dial must connect");
        let mut head = Vec::new();
        let mut chunk = [0u8; 256];
        loop {
            if head_is_complete(&head) {
                break;
            }
            let read = stream
                .read(&mut chunk)
                .await
                .expect("the connection must be readable");
            if read == 0 {
                break;
            }
            head.extend_from_slice(&chunk[..read]);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .await
            .expect("the response must be writable");
        String::from_utf8_lossy(&head).into_owned()
    });

    let router = PartnerRouter::new(BTreeMap::new());
    let uri = format!("http://127.0.0.1:{port}/x");
    router
        .send(
            &uri,
            &uri,
            OutgoingMessage {
                body: Value::Null,
                headers: BTreeMap::new(),
                method: "POST".to_string(),
            },
        )
        .await
        .expect("a plain-string http send must dial without a partner");

    let response = router
        .receive(&uri, &uri, Duration::from_secs(5))
        .await
        .expect("the receive must return the parked roundtrip");
    assert_eq!(response.status, Some(200));
    assert_eq!(response.body, Value::String("ok".to_string()));

    let recorded = served.await.expect("the far-side task must finish");
    assert!(
        recorded.starts_with("POST /x "),
        "the literal dial must carry the request line, got {recorded:?}"
    );
}

// -------------------------------------------------------------------------
// Serve semantics: times consume, delay, fault
// -------------------------------------------------------------------------

/// Dials one raw HTTP/1.1 request on a fresh connection and reads to
/// EOF. The request carries `Connection: close` so the served
/// response is followed by a server-side close; the returned bytes
/// are exactly what the server wrote. Bytes without an HTTP status
/// line — or a socket error — are a transport-level failure, never an
/// HTTP response.
async fn raw_request(target: &str, request: &[u8]) -> io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    // Dial the authority (host:port) only; the request path lives in
    // the request line.
    let authority = target
        .trim_start_matches("http://")
        .split('/')
        .next()
        .expect("the target URI must carry a host:port authority");
    let mut stream = TcpStream::connect(authority).await?;
    stream.write_all(request).await?;
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 512];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
}

/// A `times: 2` entry serves the first two matching requests and is
/// then exhausted; the third request falls through to the next entry.
#[tokio::test]
async fn times_two_serves_two_then_falls_through() {
    let partner = HttpPartner::start(vec![
        ScriptedResponse {
            status: 201,
            body: b"two-times".to_vec(),
            times: 2,
            ..Default::default()
        },
        ScriptedResponse {
            status: 200,
            body: b"fallback".to_vec(),
            ..Default::default()
        },
    ])
    .await
    .expect("partner must bind 127.0.0.1:0");
    let target = format!("http://{}/any", partner.bound_addr());
    let request = b"GET /any HTTP/1.1\r\nHost: partner\r\nConnection: close\r\n\r\n";

    let first = raw_request(&target, request)
        .await
        .expect("the first request must complete");
    let second = raw_request(&target, request)
        .await
        .expect("the second request must complete");
    let third = raw_request(&target, request)
        .await
        .expect("the third request must complete");

    assert!(
        first.starts_with(b"HTTP/1.1 201"),
        "the times:2 entry must serve the first request, got {first:?}"
    );
    assert!(
        second.starts_with(b"HTTP/1.1 201"),
        "the times:2 entry must serve twice, got {second:?}"
    );
    assert!(
        third.starts_with(b"HTTP/1.1 200"),
        "the third request must fall through to the next entry, got {third:?}"
    );
    assert!(
        third.ends_with(b"fallback"),
        "the third request must get the second entry's body, got {third:?}"
    );
}

/// A `delay` holds the response back by at least the scripted
/// duration before serving it.
#[tokio::test]
async fn delay_holds_response() {
    let partner = HttpPartner::start(vec![ScriptedResponse {
        delay: Some(Duration::from_millis(300)),
        status: 200,
        body: b"slow".to_vec(),
        ..Default::default()
    }])
    .await
    .expect("partner must bind 127.0.0.1:0");
    let target = format!("http://{}/slow", partner.bound_addr());

    let started = std::time::Instant::now();
    let bytes = raw_request(
        &target,
        b"GET /slow HTTP/1.1\r\nHost: partner\r\nConnection: close\r\n\r\n",
    )
    .await
    .expect("the delayed response must arrive");

    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "the response must be held by the delay, got {:?}",
        started.elapsed()
    );
    assert!(
        bytes.starts_with(b"HTTP/1.1 200"),
        "the delayed response must still serve, got {bytes:?}"
    );
}

/// The close fault surfaces as a transport-level failure on the
/// client — the connection closes with no HTTP status line — while
/// the faulted request is still recorded.
#[tokio::test]
async fn fault_close_yields_transport_error_and_records() {
    let partner = HttpPartner::start(vec![ScriptedResponse {
        fault: Some(PartnerFault::Close),
        status: 200,
        ..Default::default()
    }])
    .await
    .expect("partner must bind 127.0.0.1:0");
    let recorder = partner.recorder();
    let target = format!("http://{}/boom", partner.bound_addr());
    let request = b"GET /boom HTTP/1.1\r\nHost: partner\r\nConnection: close\r\n\r\n";

    // A transport-level failure: an abrupt disconnect may also error
    // the socket; either way, no HTTP status may surface.
    if let Ok(bytes) = raw_request(&target, request).await {
        assert!(
            !bytes.starts_with(b"HTTP/"),
            "a close fault must not surface an HTTP status, got {bytes:?}"
        );
    }

    let recorded = recorder.recorded_requests();
    assert_eq!(
        recorded.len(),
        1,
        "the faulted request must still be recorded, got {recorded:?}"
    );
    assert_eq!(recorded[0].path, "/boom");
}

/// The delay applies before the fault: the connection closes only
/// after the scripted delay elapsed.
#[tokio::test]
async fn delay_before_fault() {
    let partner = HttpPartner::start(vec![ScriptedResponse {
        delay: Some(Duration::from_millis(200)),
        fault: Some(PartnerFault::Close),
        status: 200,
        ..Default::default()
    }])
    .await
    .expect("partner must bind 127.0.0.1:0");
    let target = format!("http://{}/boom", partner.bound_addr());
    let request = b"GET /boom HTTP/1.1\r\nHost: partner\r\nConnection: close\r\n\r\n";

    let started = std::time::Instant::now();
    if let Ok(bytes) = raw_request(&target, request).await {
        assert!(
            !bytes.starts_with(b"HTTP/"),
            "a close fault must not surface an HTTP status, got {bytes:?}"
        );
    }
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "the fault must wait for the delay first, got {:?}",
        started.elapsed()
    );
}

/// A fault entry with `times: 2` faults the first two matching
/// requests (transport-level failures, no HTTP status) and is then
/// spent: the third request falls through to the fallback entry's
/// 200. Every faulted request is still recorded.
#[tokio::test]
async fn fault_with_times_faults_twice_then_spends() {
    let partner = HttpPartner::start(vec![
        ScriptedResponse {
            fault: Some(PartnerFault::Close),
            times: 2,
            ..Default::default()
        },
        ScriptedResponse {
            status: 200,
            ..Default::default()
        },
    ])
    .await
    .expect("partner must bind 127.0.0.1:0");
    let recorder = partner.recorder();
    let target = format!("http://{}/any", partner.bound_addr());
    let request = b"GET /any HTTP/1.1\r\nHost: partner\r\nConnection: close\r\n\r\n";

    // The first two requests must not surface an HTTP status. An
    // abrupt disconnect may also error the socket; either way, no
    // status line may arrive.
    for round in 1..=2 {
        if let Ok(bytes) = raw_request(&target, request).await {
            assert!(
                !bytes.starts_with(b"HTTP/"),
                "close fault round {round} must not surface an HTTP status, got {bytes:?}"
            );
        }
    }
    let third = raw_request(&target, request)
        .await
        .expect("the third request must complete after the entry is spent");
    assert!(
        third.starts_with(b"HTTP/1.1 200"),
        "the third request must fall through to the fallback, got {third:?}"
    );

    let recorded = recorder.recorded_requests();
    assert_eq!(
        recorded.len(),
        3,
        "every faulted request must still be recorded, got {recorded:?}"
    );
}

/// Entries without `times` still serve exactly once each: two
/// identical plain entries answer in script order — the first
/// request gets entry one's body, the second entry two's.
#[tokio::test]
async fn matched_entries_without_times_still_serve_once() {
    let partner = HttpPartner::start(vec![
        ScriptedResponse {
            status: 200,
            body: b"first".to_vec(),
            ..Default::default()
        },
        ScriptedResponse {
            status: 200,
            body: b"second".to_vec(),
            ..Default::default()
        },
    ])
    .await
    .expect("partner must bind 127.0.0.1:0");
    let target = format!("http://{}/any", partner.bound_addr());
    let request = b"GET /any HTTP/1.1\r\nHost: partner\r\nConnection: close\r\n\r\n";

    let first = raw_request(&target, request)
        .await
        .expect("the first request must complete");
    let second = raw_request(&target, request)
        .await
        .expect("the second request must complete");

    assert!(
        first.ends_with(b"first"),
        "the first request must get entry one's body, got {first:?}"
    );
    assert!(
        second.ends_with(b"second"),
        "the second request must get entry two's body, got {second:?}"
    );
}
