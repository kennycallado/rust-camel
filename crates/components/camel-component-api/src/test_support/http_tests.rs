//! Unit tests for the shared HTTP/1.1 test harness. Every test drives a
//! real `tokio::net::TcpStream` client against a started listener.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

use super::{
    HttpTestServer, RecordedRequest, Responder, Response, ResponseHeaders, serve_connection,
};

const TEST_DEADLINE: Duration = Duration::from_secs(5);

async fn connect(addr: SocketAddr) -> TcpStream {
    tokio::time::timeout(TEST_DEADLINE, TcpStream::connect(addr))
        .await
        .expect("connect within deadline")
        .expect("connect to harness")
}

async fn start(responder: Responder) -> HttpTestServer {
    tokio::time::timeout(TEST_DEADLINE, HttpTestServer::start(responder))
        .await
        .expect("server start within deadline")
        .expect("start harness")
}

async fn start_fixed(response: Response) -> HttpTestServer {
    start(Responder::Fixed(response)).await
}

async fn read_to_eof(stream: &mut TcpStream) -> Vec<u8> {
    let mut out = Vec::new();
    tokio::time::timeout(TEST_DEADLINE, stream.read_to_end(&mut out))
        .await
        .expect("response read within deadline")
        .expect("read response to EOF");
    out
}

async fn request_text(addr: SocketAddr, request: &[u8]) -> String {
    let mut client = connect(addr).await;
    client.write_all(request).await.expect("write request");
    String::from_utf8_lossy(&read_to_eof(&mut client).await).into_owned()
}

/// Write a POST head, signal readiness, then write the body only after the
/// test releases the gate. The probe before the gate proves the harness did
/// not answer a head-only request.
async fn gated_post(addr: SocketAddr, head: &'static [u8]) -> Vec<u8> {
    let (go_tx, go_rx) = oneshot::channel::<()>();
    let (ready_tx, ready_rx) = oneshot::channel::<()>();

    let client = tokio::spawn(async move {
        let mut stream = connect(addr).await;
        stream.write_all(head).await.expect("write request head");
        ready_tx.send(()).expect("signal head written");

        let mut probe = [0u8; 1];
        match stream.try_read(&mut probe) {
            Ok(0) => panic!("harness closed the connection before the request body"),
            Ok(_) => panic!("harness replied before the request body arrived"),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(err) => panic!("request probe failed: {err}"),
        }

        tokio::time::timeout(TEST_DEADLINE, go_rx)
            .await
            .expect("body write signal within deadline")
            .expect("body write signal");
        stream
            .write_all(b"hello")
            .await
            .expect("write request body");
        read_to_eof(&mut stream).await
    });

    tokio::time::timeout(TEST_DEADLINE, ready_rx)
        .await
        .expect("head-written signal within deadline")
        .expect("head-written signal");
    go_tx.send(()).expect("release the body write");

    tokio::time::timeout(TEST_DEADLINE, client)
        .await
        .expect("client task within deadline")
        .expect("client task join")
}

#[tokio::test]
async fn roundtrip_answers_and_closes() {
    let server = start_fixed(Response::ok("ok")).await;
    let mut client = connect(server.local_addr()).await;
    client
        .write_all(b"GET /x HTTP/1.1\r\nHost: h\r\n\r\n")
        .await
        .expect("write GET");
    let raw = read_to_eof(&mut client).await;
    let text = String::from_utf8_lossy(&raw);

    assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "status: {text}");
    assert!(
        text.contains("\r\nContent-Length: 2\r\n"),
        "content length: {text}"
    );
    assert!(
        text.contains("\r\nConnection: close\r\n"),
        "connection close header: {text}"
    );
    assert!(text.ends_with("\r\n\r\nok"), "body: {text}");
    // `read_to_end` returned only after the harness closed the socket, so EOF
    // followed the complete response.
    server.shutdown().await;
}

#[tokio::test]
async fn post_body_drained_before_reply() {
    let server = start_fixed(Response::ok("done")).await;
    let head: &'static [u8] = b"POST /x HTTP/1.1\r\nHost: h\r\nContent-Length: 5\r\n\r\n";

    let raw = gated_post(server.local_addr(), head).await;
    let text = String::from_utf8_lossy(&raw);
    assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "status: {text}");

    let recorded = server.recorded();
    assert_eq!(recorded.len(), 1, "one request recorded");
    assert_eq!(recorded[0].method, "POST");
    assert_eq!(recorded[0].body.as_slice(), &b"hello"[..]);
    server.shutdown().await;
}

#[tokio::test]
async fn segmented_body_recorded_fully() {
    let server = start_fixed(Response::ok("ok")).await;
    let head: &'static [u8] = b"POST /x?a=1&b=2 HTTP/1.1\r\nHost: h\r\nContent-Length: 5\r\n\r\n";

    let raw = gated_post(server.local_addr(), head).await;
    assert!(
        String::from_utf8_lossy(&raw).starts_with("HTTP/1.1 200 OK\r\n"),
        "response status"
    );

    let recorded = server.recorded();
    assert_eq!(recorded.len(), 1, "one request recorded");
    assert_eq!(
        recorded[0].wire_head.as_slice(),
        head,
        "wire head must be byte-identical to the bytes the client sent"
    );
    assert_eq!(recorded[0].target, "/x?a=1&b=2");
    assert_eq!(recorded[0].body.as_slice(), &b"hello"[..]);
    server.shutdown().await;
}

#[tokio::test]
async fn path_routing_answers_per_path() {
    let responder = Responder::route(|request: &RecordedRequest| {
        if request.target == "/fail" {
            Response::with_status(500, "Internal Server Error", "failed")
        } else {
            Response::ok("ok")
        }
    });
    let server = start(responder).await;

    let first = request_text(
        server.local_addr(),
        b"GET /fail HTTP/1.1\r\nHost: h\r\n\r\n",
    )
    .await;
    let second = request_text(server.local_addr(), b"GET /ok HTTP/1.1\r\nHost: h\r\n\r\n").await;

    assert!(
        first.starts_with("HTTP/1.1 500 Internal Server Error\r\n"),
        "first status: {first}"
    );
    assert!(
        second.starts_with("HTTP/1.1 200 OK\r\n"),
        "second status: {second}"
    );

    let recorded = server.recorded();
    assert_eq!(recorded.len(), 2, "both requests recorded");
    assert_eq!(recorded[0].target, "/fail");
    assert_eq!(recorded[1].target, "/ok");
    server.shutdown().await;
}

#[tokio::test]
async fn raw_response_gets_close_injected() {
    let server = start_fixed(Response::raw(
        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec(),
    ))
    .await;

    let text = request_text(server.local_addr(), b"GET /x HTTP/1.1\r\nHost: h\r\n\r\n").await;

    assert!(
        text.starts_with("HTTP/1.1 204 No Content\r\n"),
        "status: {text}"
    );
    let close = text
        .find("\r\nConnection: close\r\n")
        .expect("injected Connection: close header line");
    let blank = text.find("\r\n\r\n").expect("head terminator");
    assert!(
        close < blank,
        "Connection: close must sit before the blank line: {text}"
    );
    server.shutdown().await;
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Read until the accumulated bytes contain `needle`, returning everything
/// read in this call.
async fn read_until(stream: &mut TcpStream, needle: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if find_subslice(&out, needle).is_some() {
            return out;
        }
        let read = tokio::time::timeout(TEST_DEADLINE, stream.read(&mut chunk))
            .await
            .expect("read within deadline")
            .expect("read from harness");
        assert!(
            read > 0,
            "EOF before {:?} arrived",
            String::from_utf8_lossy(needle)
        );
        out.extend_from_slice(&chunk[..read]);
    }
}

/// Read exactly one `Content-Length`-framed response, leaving any following
/// bytes unread.
async fn read_one_response(stream: &mut TcpStream) -> Vec<u8> {
    let mut buf = read_until(stream, b"\r\n\r\n").await;
    let head_end = find_subslice(&buf, b"\r\n\r\n").expect("response head terminator") + 4;
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let content_length = head
        .split("\r\n")
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);

    let mut chunk = [0u8; 4096];
    while buf.len() < head_end + content_length {
        let read = tokio::time::timeout(TEST_DEADLINE, stream.read(&mut chunk))
            .await
            .expect("body read within deadline")
            .expect("read response body");
        assert!(read > 0, "EOF before the response body completed");
        buf.extend_from_slice(&chunk[..read]);
    }
    buf
}

#[tokio::test]
async fn keep_alive_serves_two_on_one_connection() {
    let server = tokio::time::timeout(
        TEST_DEADLINE,
        HttpTestServer::builder()
            .keep_alive("test asserts reuse")
            .start(Responder::Fixed(Response::ok("one"))),
    )
    .await
    .expect("builder start within deadline")
    .expect("start harness");

    let mut client = connect(server.local_addr()).await;

    client
        .write_all(b"GET /first HTTP/1.1\r\nHost: h\r\n\r\n")
        .await
        .expect("write first GET");
    let first = String::from_utf8_lossy(&read_one_response(&mut client).await).into_owned();
    assert!(
        first.starts_with("HTTP/1.1 200 OK\r\n"),
        "first status: {first}"
    );
    assert!(
        first.contains("\r\nConnection: keep-alive\r\n"),
        "keep-alive header: {first}"
    );
    assert!(
        !first.contains("Connection: close"),
        "keep-alive response must not close: {first}"
    );
    assert!(first.ends_with("\r\n\r\none"), "first body: {first}");

    client
        .write_all(b"GET /second HTTP/1.1\r\nHost: h\r\n\r\n")
        .await
        .expect("write second GET");
    let second = String::from_utf8_lossy(&read_one_response(&mut client).await).into_owned();
    assert!(
        second.starts_with("HTTP/1.1 200 OK\r\n"),
        "second status: {second}"
    );
    assert!(
        second.contains("\r\nConnection: keep-alive\r\n"),
        "second keep-alive header: {second}"
    );

    let recorded = server.recorded();
    assert_eq!(recorded.len(), 2, "two requests recorded");
    assert_eq!(recorded[0].target, "/first");
    assert_eq!(recorded[1].target, "/second");

    server.shutdown().await;
}

#[tokio::test]
async fn staged_headers_then_signaled_chunks() {
    let (tx, rx) = mpsc::channel::<Vec<u8>>(4);
    let headers = ResponseHeaders::new(
        200,
        "OK",
        vec![("Transfer-Encoding".to_string(), "chunked".to_string())],
    );
    let server = start_fixed(Response::staged(headers, rx)).await;
    let mut client = connect(server.local_addr()).await;
    client
        .write_all(b"GET /staged HTTP/1.1\r\nHost: h\r\n\r\n")
        .await
        .expect("write GET");

    // The header block must arrive before the first chunk is sent.
    let head = String::from_utf8_lossy(&read_until(&mut client, b"\r\n\r\n").await).into_owned();
    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "status: {head}");
    assert!(
        head.contains("\r\nTransfer-Encoding: chunked\r\n"),
        "framing: {head}"
    );
    assert!(
        head.contains("\r\nConnection: close\r\n"),
        "connection: {head}"
    );

    tx.send(b"a".to_vec()).await.expect("send first chunk");
    let first = read_until(&mut client, b"1\r\na\r\n").await;
    assert_eq!(first, b"1\r\na\r\n", "first chunk framing");

    tx.send(b"bc".to_vec()).await.expect("send second chunk");
    let second = read_until(&mut client, b"2\r\nbc\r\n").await;
    assert_eq!(second, b"2\r\nbc\r\n", "second chunk framing");

    drop(tx);
    let tail = read_to_eof(&mut client).await;
    assert_eq!(tail, b"0\r\n\r\n", "terminal chunk at channel close");

    server.shutdown().await;
}

#[tokio::test]
async fn chunked_framing_is_legal() {
    let server = start_fixed(Response::chunked(vec![b"a".to_vec(), b"bc".to_vec()])).await;
    let raw = request_text(
        server.local_addr(),
        b"GET /chunked HTTP/1.1\r\nHost: h\r\n\r\n",
    )
    .await;

    assert!(raw.starts_with("HTTP/1.1 200 OK\r\n"), "status: {raw}");
    assert!(
        raw.contains("\r\nTransfer-Encoding: chunked\r\n"),
        "framing: {raw}"
    );
    assert!(!raw.contains("Content-Length"), "no content length: {raw}");

    let body_start = raw.find("\r\n\r\n").expect("head terminator") + 4;
    assert_eq!(
        &raw[body_start..],
        "1\r\na\r\n2\r\nbc\r\n0\r\n\r\n",
        "legal chunk framing assembles to abc"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn malformed_head_gets_400() {
    let server = start_fixed(Response::ok("unused")).await;
    let text = request_text(server.local_addr(), b"NOT-A-REQUEST\r\n\r\n").await;

    assert!(
        text.starts_with("HTTP/1.1 400 Bad Request\r\n"),
        "status: {text}"
    );
    assert!(
        text.contains("\r\nConnection: close\r\n"),
        "connection: {text}"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn truncated_body_gets_400() {
    let server = start_fixed(Response::ok("unused")).await;
    let mut client = connect(server.local_addr()).await;
    client
        .write_all(b"POST /x HTTP/1.1\r\nHost: h\r\nContent-Length: 10\r\n\r\nabc")
        .await
        .expect("write truncated request");
    // Close only the write half so the 400 response remains readable.
    client.shutdown().await.expect("shutdown write half");

    let text = String::from_utf8_lossy(&read_to_eof(&mut client).await).into_owned();
    assert!(
        text.starts_with("HTTP/1.1 400 Bad Request\r\n"),
        "status: {text}"
    );
    assert!(
        text.contains("\r\nConnection: close\r\n"),
        "connection: {text}"
    );

    server.shutdown().await;
}

#[tokio::test]
async fn unsupported_transfer_encoding_gets_400() {
    let server = start_fixed(Response::ok("unused")).await;
    let text = request_text(
        server.local_addr(),
        b"POST /x HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: gzip\r\n\r\n",
    )
    .await;

    assert!(
        text.starts_with("HTTP/1.1 400 Bad Request\r\n"),
        "status: {text}"
    );

    server.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn drop_is_non_blocking_and_releases_listener() {
    let server = start_fixed(Response::ok("unused")).await;
    let addr = server.local_addr();
    let mut client = connect(addr).await;
    client
        .write_all(b"GET /half HTTP/1.1\r\nHost: h\r\n")
        .await
        .expect("write partial head");

    drop(server);

    // The listener task is detached by `Drop`; it must stop accepting once the
    // runtime polls it. A blocking drop would have hung above instead.
    let deadline = Instant::now() + TEST_DEADLINE;
    loop {
        // allow-test-wait: bounded by TEST_DEADLINE assert in accept branch
        match TcpStream::connect(addr).await {
            Err(_) => break,
            Ok(stream) => {
                drop(stream);
                assert!(
                    Instant::now() < deadline,
                    "listener still accepting after drop"
                );
                tokio::task::yield_now().await;
            }
        }
    }

    drop(client);
}

#[tokio::test]
async fn serve_connection_over_duplex() {
    // No harness and no TCP: the duplex half stands in for a TLS-accepted
    // stream, proving `serve_connection` only needs the generic IO bounds.
    let (server_io, mut client) = tokio::io::duplex(4096);
    let serving = tokio::spawn(serve_connection(
        server_io,
        Responder::Fixed(Response::ok("duplex")),
        HttpTestServer::builder(),
    ));

    client
        .write_all(b"GET /duplex HTTP/1.1\r\nHost: h\r\n\r\n")
        .await
        .expect("write GET");

    let mut raw = Vec::new();
    tokio::time::timeout(TEST_DEADLINE, client.read_to_end(&mut raw))
        .await
        .expect("response read within deadline")
        .expect("read response to EOF");
    let text = String::from_utf8_lossy(&raw);

    assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "status: {text}");
    assert!(
        text.contains("\r\nConnection: close\r\n"),
        "connection: {text}"
    );
    assert!(text.ends_with("\r\n\r\nduplex"), "body: {text}");

    tokio::time::timeout(TEST_DEADLINE, serving)
        .await
        .expect("serving task within deadline")
        .expect("serving task join")
        .expect("serve_connection result");
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_confirms_reclamation() {
    let server = tokio::time::timeout(
        TEST_DEADLINE,
        HttpTestServer::builder()
            .keep_alive("shutdown must reclaim an idle keep-alive socket")
            .start(Responder::Fixed(Response::ok("idle"))),
    )
    .await
    .expect("builder start within deadline")
    .expect("start harness");
    let addr = server.local_addr();

    let mut client = connect(addr).await;
    client
        .write_all(b"GET /idle HTTP/1.1\r\nHost: h\r\n\r\n")
        .await
        .expect("write GET");
    let response = String::from_utf8_lossy(&read_one_response(&mut client).await).into_owned();
    assert!(
        response.contains("\r\nConnection: keep-alive\r\n"),
        "keep-alive: {response}"
    );

    // The connection stays idle; shutdown must cancel and join it.
    tokio::time::timeout(TEST_DEADLINE, server.shutdown())
        .await
        .expect("shutdown within deadline");

    drop(client);
    TcpListener::bind(addr)
        .await
        .expect("listener port released after shutdown");
}

#[tokio::test]
async fn staged_reused_across_keepalive_gets_500() {
    let (tx, rx) = mpsc::channel::<Vec<u8>>(4);
    let headers = ResponseHeaders::new(
        200,
        "OK",
        vec![("Transfer-Encoding".to_string(), "chunked".to_string())],
    );
    let server = tokio::time::timeout(
        TEST_DEADLINE,
        HttpTestServer::builder()
            .keep_alive("a staged response consumed by the first request")
            .start(Responder::Fixed(Response::staged(headers, rx))),
    )
    .await
    .expect("builder start within deadline")
    .expect("start harness");

    let mut client = connect(server.local_addr()).await;
    client
        .write_all(b"GET /first HTTP/1.1\r\nHost: h\r\n\r\n")
        .await
        .expect("write first GET");
    tx.send(b"a".to_vec()).await.expect("send staged chunk");
    drop(tx);

    let first = String::from_utf8_lossy(&read_until(&mut client, b"0\r\n\r\n").await).into_owned();
    assert!(
        first.starts_with("HTTP/1.1 200 OK\r\n"),
        "first status: {first}"
    );
    assert!(
        first.contains("\r\nConnection: keep-alive\r\n"),
        "first keep-alive: {first}"
    );

    client
        .write_all(b"GET /second HTTP/1.1\r\nHost: h\r\n\r\n")
        .await
        .expect("write second GET");
    // Deadline-bounded: a silent close would hang here instead of answering.
    let second = String::from_utf8_lossy(&read_to_eof(&mut client).await).into_owned();
    assert!(
        second.starts_with("HTTP/1.1 500 Internal Server Error\r\n"),
        "second status: {second}"
    );
    assert!(
        second.contains("\r\nConnection: close\r\n"),
        "second connection: {second}"
    );
    assert!(
        second.ends_with("\r\n\r\n"),
        "complete response before EOF: {second}"
    );

    assert_eq!(server.recorded().len(), 2, "both requests recorded");
    server.shutdown().await;
}

#[tokio::test]
async fn raw_close_honored_in_keepalive_mode() {
    let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_vec();
    let server = tokio::time::timeout(
        TEST_DEADLINE,
        HttpTestServer::builder()
            .keep_alive("raw response owns the connection header")
            .start(Responder::Fixed(Response::raw(raw))),
    )
    .await
    .expect("builder start within deadline")
    .expect("start harness");

    let mut client = connect(server.local_addr()).await;
    client
        .write_all(b"GET /raw HTTP/1.1\r\nHost: h\r\n\r\n")
        .await
        .expect("write GET");
    // `read_to_eof` is deadline-bounded: a server that kept looping for
    // another request would hang here and fail the test.
    let text = String::from_utf8_lossy(&read_to_eof(&mut client).await).into_owned();

    assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "status: {text}");
    assert_eq!(
        text.matches("Connection: close").count(),
        1,
        "raw connection header must not be duplicated: {text}"
    );
    assert!(text.ends_with("\r\n\r\nok"), "body before EOF: {text}");
    assert_eq!(
        server.recorded().len(),
        1,
        "server must not loop after a raw Connection: close"
    );
    server.shutdown().await;
}
