//! Integration tests for the axum-bare reference contender (bd rc-u034).
//!
//! Contract under test:
//! - Marker: exactly one `BENCH_ROUTE_READY` stdout line after listener bind.
//! - Route: POST /bench returns 200 `text/plain; charset=utf-8` body `pong`,
//!   fully drains the request body (keep-alive reuse proves the drain), and
//!   logs `BENCH_HTTP_REQUEST received` + `BENCH_HTTP_REQUEST id=<n>`.
//! - Bind failure: occupied port → nonzero exit with `error` on stderr.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// RAII guard: kills and reaps the spawned server on every exit path so a
/// failed assert cannot leak the process.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Reserve a free TCP port by binding to :0 and immediately releasing it.
fn free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral port");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);
    port
}

/// Dedicated stdout reader thread: lines flow through a channel so the test
/// thread can wait with an overall deadline and never hang.
fn spawn_stdout_reader(child: &mut Child) -> Receiver<String> {
    let stdout = child.stdout.take().expect("stdout piped");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    rx
}

/// Receive one stdout line or give up at the deadline.
fn next_line(rx: &Receiver<String>, deadline: Instant) -> Option<String> {
    let budget = deadline.checked_duration_since(Instant::now())?;
    rx.recv_timeout(budget).ok()
}

fn spawn_server(port: u16) -> ChildGuard {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_axum-bare-fixture"));
    cmd.env("BENCH_AXUM_BARE_PORT", port.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    ChildGuard(cmd.spawn().expect("spawn axum-bare-fixture"))
}

/// One POST /bench exchange: head + two body chunks, 3 separate write_all
/// calls (TCP may coalesce them; the drain proof is keep-alive reuse, not
/// write count).
fn write_post(stream: &mut TcpStream) {
    const BODY_LEN: usize = 32768;
    let body = vec![b'x'; BODY_LEN];
    let head = format!(
        "POST /bench HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {BODY_LEN}\r\n\
         Connection: keep-alive\r\n\r\n"
    );
    stream
        .write_all(head.as_bytes())
        .expect("write request head");
    stream
        .write_all(&body[..BODY_LEN / 2])
        .expect("write body part 1");
    stream
        .write_all(&body[BODY_LEN / 2..])
        .expect("write body part 2");
}

/// Byte-substring search over a buffer that may not be valid UTF-8.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Framed response read: headers up to the first `\r\n\r\n`, then EXACTLY
/// Content-Length body bytes. Never read-to-EOF (that would race the
/// keep-alive reuse of the next request).
fn read_response(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut buf = [0u8; 4096];
    let mut raw: Vec<u8> = Vec::new();
    let header_end = loop {
        let n = stream.read(&mut buf).expect("read response head");
        assert!(n > 0, "connection closed before headers completed");
        raw.extend_from_slice(&buf[..n]);
        if let Some(pos) = find_subslice(&raw, b"\r\n\r\n") {
            break pos;
        }
        assert!(raw.len() <= 64 * 1024, "response headers too large");
    };
    let head = String::from_utf8_lossy(&raw[..header_end]).into_owned();
    let content_length: usize = head
        .lines()
        .find_map(|l| {
            let (name, value) = l.split_once(':')?;
            if name.trim().eq_ignore_ascii_case("content-length") {
                value.trim().parse().ok()
            } else {
                None
            }
        })
        .expect("Content-Length header present");
    let mut body = raw[header_end + 4..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut buf).expect("read response body");
        assert!(n > 0, "connection closed before body completed");
        body.extend_from_slice(&buf[..n]);
    }
    body.truncate(content_length);
    (head, body)
}

/// Both handler-log lines for both requests arrived.
fn logs_complete(lines: &[String]) -> bool {
    let c = lines.join("\n");
    c.matches("BENCH_HTTP_REQUEST received").count() >= 2
        && c.contains("id=1")
        && c.contains("id=2")
}

#[test]
fn marker_then_two_keepalive_requests_succeed() {
    let port = free_port();
    let mut child = spawn_server(port);
    let rx = spawn_stdout_reader(&mut child.0);
    let deadline = Instant::now() + Duration::from_secs(5);

    // Marker contract: a stdout line exactly `BENCH_ROUTE_READY` after bind.
    let mut lines: Vec<String> = Vec::new();
    let ready = loop {
        match next_line(&rx, deadline) {
            Some(line) => {
                let is_ready = line == "BENCH_ROUTE_READY";
                lines.push(line);
                if is_ready {
                    break true;
                }
            }
            None => break false,
        }
    };
    assert!(
        ready,
        "no BENCH_ROUTE_READY line within deadline; saw: {lines:?}"
    );

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");

    // TWO requests over the SAME stream: keep-alive reuse proves the 32 KiB
    // request body was fully drained both times.
    for _ in 0..2 {
        write_post(&mut stream);
        let (head, body) = read_response(&mut stream);
        assert!(
            head.starts_with("HTTP/1.1 200"),
            "status line not 200: {head}"
        );
        assert!(
            head.to_ascii_lowercase()
                .contains("content-type: text/plain; charset=utf-8"),
            "missing content-type header: {head}"
        );
        assert_eq!(body, b"pong");
    }

    let log_deadline = Instant::now() + Duration::from_secs(5);
    while !logs_complete(&lines) {
        match next_line(&rx, log_deadline) {
            Some(line) => lines.push(line),
            None => panic!(
                "deadline exceeded waiting for request logs; saw: {:?}",
                lines.join("\n")
            ),
        }
    }
    let collected = lines.join("\n");
    assert!(
        collected.matches("BENCH_HTTP_REQUEST received").count() >= 2,
        "expected >=2 received lines: {collected}"
    );
    assert!(collected.contains("id=1"), "missing id=1: {collected}");
    assert!(collected.contains("id=2"), "missing id=2: {collected}");
    let ready_lines = lines.iter().filter(|l| *l == "BENCH_ROUTE_READY").count();
    assert_eq!(ready_lines, 1, "exactly one BENCH_ROUTE_READY expected");
    // Child killed+waited by ChildGuard::drop on every path.
}

#[test]
fn bind_failure_exits_nonzero() {
    // Hold the port with a pre-bound listener so the server bind must fail.
    let holder = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral port");
    let port = holder.local_addr().expect("local_addr").port();

    let mut child = spawn_server(port);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.0.try_wait().expect("try_wait") {
            Some(status) => {
                assert!(!status.success(), "server exited 0 despite port conflict");
                // Child has exited: the stderr pipe write end is closed, so
                // read_to_string returns at EOF instead of blocking.
                let mut stderr = String::new();
                if let Some(mut pipe) = child.0.stderr.take() {
                    pipe.read_to_string(&mut stderr).expect("read stderr");
                }
                assert!(stderr.contains("error"), "stderr missing error: {stderr:?}");
                return;
            }
            None => {
                assert!(
                    Instant::now() < deadline,
                    "server did not exit within 10s of port conflict"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}
