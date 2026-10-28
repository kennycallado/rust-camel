//! Queue-mode blocking-pop tests against a fake RESP server.
//!
//! Regression coverage for bd rc-y0pwj: redis-rs 1.7.1's default
//! `AsyncConnectionConfig` carries a 500 ms response timeout, which cuts
//! off `BLPOP`/`BRPOP` pops whose own timeout exceeds 500 ms and forces a
//! spurious reconnect. These tests drive the real `RedisQueueIo::connect`
//! through `queue_session` and assert one delivery on one connection.
//!
//! The fake server accepts (and counts) every connection, answers the
//! driver handshake, and answers a configured `BLPOP`/`BRPOP` after a
//! programmed delay — or withholds the reply.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use camel_component_api::{Body, Exchange, Message, NetworkRetryPolicy};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::queue::{QueueConsumerParams, QueuePopCommand, RedisQueueIo, queue_session};
use crate::topology::FakeTopology;

/// Programmed server behavior for one blocking-pop request.
struct BlockingPopPlan {
    /// Time the server waits after reading the pop before replying.
    delay: Duration,
    /// RESP reply bytes; `None` withholds the reply entirely.
    reply: Option<Vec<u8>>,
}

/// A loopback RESP server that accepts every connection until shutdown.
struct BlockingPopServer {
    addr: SocketAddr,
    connections: Arc<AtomicUsize>,
    commands: watch::Receiver<Vec<String>>,
}

impl BlockingPopServer {
    async fn start(plan: BlockingPopPlan) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let connections = Arc::new(AtomicUsize::new(0));
        let (commands_tx, commands) = watch::channel(Vec::new());
        let plan = Arc::new(plan);
        let accepted = Arc::clone(&connections);
        tokio::spawn(async move {
            // Accept and count EVERY connection: a reconnect attempt must
            // surface as connection_count() >= 2, never as a refused socket.
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                accepted.fetch_add(1, Ordering::SeqCst);
                let plan = Arc::clone(&plan);
                let commands_tx = commands_tx.clone();
                tokio::spawn(async move {
                    serve_connection(stream, plan, commands_tx).await;
                });
            }
        });
        Ok(Self {
            addr,
            connections,
            commands,
        })
    }

    fn addr(&self) -> SocketAddr {
        self.addr
    }

    fn url(&self) -> String {
        format!("redis://{}", self.addr())
    }

    fn connection_count(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// Snapshot of the application command names received so far.
    fn command_names(&self) -> Vec<String> {
        self.commands.borrow().clone()
    }

    /// Wait (bounded) until the server has parsed a blocking-pop request, so
    /// callers can start their observation window *after* the pop is in flight.
    async fn wait_for_blocking_pop(&mut self) {
        let observed = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !self.commands.borrow().is_empty() {
                    return;
                }
                if self.commands.changed().await.is_err() {
                    return;
                }
            }
        })
        .await;
        assert!(
            observed.is_ok(),
            "fake server must observe the blocking-pop request within 2s"
        );
        assert!(
            !self.commands.borrow().is_empty(),
            "fake server must have parsed the blocking-pop request"
        );
    }
}

/// Serve one connection until the peer disconnects or a write fails.
async fn serve_connection(
    mut stream: TcpStream,
    plan: Arc<BlockingPopPlan>,
    commands: watch::Sender<Vec<String>>,
) {
    let mut buf = Vec::with_capacity(256);
    loop {
        let Ok(Some(arguments)) = next_frame(&mut stream, &mut buf).await else {
            break;
        };
        let Some(first) = arguments.first() else {
            continue;
        };
        let name = String::from_utf8_lossy(first).to_ascii_uppercase();
        if is_handshake_command(&name) {
            if stream.write_all(b"+OK\r\n").await.is_err() {
                break;
            }
            continue;
        }
        if name == "BLPOP" || name == "BRPOP" {
            commands.send_modify(|names| names.push(name.clone()));
            tokio::time::sleep(plan.delay).await;
            if let Some(reply) = &plan.reply
                && stream.write_all(reply).await.is_err()
            {
                break;
            }
            continue;
        }
        // Unknown application command: acknowledge so nothing hangs.
        if stream.write_all(b"+OK\r\n").await.is_err() {
            break;
        }
    }
}

/// Commands the redis driver issues while establishing a connection.
fn is_handshake_command(name: &str) -> bool {
    ["CLIENT", "SELECT", "AUTH", "HELLO"].contains(&name)
}

/// Consume one complete RESP array-of-bulk-strings request from `stream`.
async fn next_frame(
    stream: &mut TcpStream,
    buf: &mut Vec<u8>,
) -> std::io::Result<Option<Vec<Vec<u8>>>> {
    let mut chunk = [0u8; 256];
    loop {
        if let Some((frame_len, arguments)) = parse_frame(buf)? {
            let _ = buf.drain(..frame_len);
            return Ok(Some(arguments));
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..read]);
    }
}

/// Parse one complete RESP array-of-bulk-strings frame from `buf`.
fn parse_frame(buf: &[u8]) -> std::io::Result<Option<(usize, Vec<Vec<u8>>)>> {
    let Some((elements, mut pos)) = parse_header(buf, 0, b'*')? else {
        return Ok(None);
    };
    let mut arguments = Vec::with_capacity(elements);
    for _ in 0..elements {
        let Some((len, payload_at)) = parse_header(buf, pos, b'$')? else {
            return Ok(None);
        };
        let Some(end) = payload_at.checked_add(len).and_then(|e| e.checked_add(2)) else {
            return Err(invalid_data("bulk string length overflow"));
        };
        if buf.len() < end {
            return Ok(None);
        }
        arguments.push(buf[payload_at..payload_at + len].to_vec());
        pos = end;
    }
    Ok(Some((pos, arguments)))
}

/// Parse `<marker><digits>\r\n` at `offset`, returning the value and the
/// offset just past the header.
fn parse_header(buf: &[u8], offset: usize, marker: u8) -> std::io::Result<Option<(usize, usize)>> {
    let Some(&first) = buf.get(offset) else {
        return Ok(None);
    };
    if first != marker {
        return Err(invalid_data(format!(
            "expected '{}' RESP marker, got {:?}",
            marker as char, first as char
        )));
    }
    let mut cursor = offset + 1;
    loop {
        match buf.get(cursor) {
            Some(byte) if byte.is_ascii_digit() => cursor += 1,
            Some(b'\r') => match buf.get(cursor + 1) {
                Some(b'\n') => {
                    let digits = &buf[offset + 1..cursor];
                    let text = std::str::from_utf8(digits)
                        .map_err(|_| invalid_data("non-UTF-8 header digits"))?;
                    let value: usize = text
                        .parse()
                        .map_err(|_| invalid_data("header integer overflow"))?;
                    return Ok(Some((value, cursor + 2)));
                }
                Some(_) => return Err(invalid_data("expected newline after carriage return")),
                None => return Ok(None),
            },
            Some(_) => return Err(invalid_data("unexpected byte in RESP header")),
            None => return Ok(None),
        }
    }
}

fn invalid_data(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
}

/// RESP array reply `["k", "v"]`.
fn k_v_reply() -> Vec<u8> {
    b"*2\r\n$1\r\nk\r\n$1\r\nv\r\n".to_vec()
}

fn params(pop_command: QueuePopCommand, timeout: u64) -> QueueConsumerParams {
    QueueConsumerParams {
        key: "queue".to_string(),
        timeout,
        pop_command,
    }
}

fn fast_policy() -> NetworkRetryPolicy {
    NetworkRetryPolicy {
        max_attempts: 3,
        initial_delay: Duration::ZERO,
        ..NetworkRetryPolicy::default()
    }
}

/// Deliver exactly the expected item once, then cancel the session.
fn recording_deliver(
    delivered: Arc<Mutex<Vec<(String, String)>>>,
    cancel: CancellationToken,
) -> impl FnMut((String, String)) -> std::future::Ready<()> {
    move |item| {
        delivered.lock().expect("delivered list mutex").push(item);
        cancel.cancel();
        std::future::ready(())
    }
}

#[tokio::test]
async fn queue_blpop_longer_than_old_default_delivers_once() {
    let server = BlockingPopServer::start(BlockingPopPlan {
        delay: Duration::from_secs(1),
        reply: Some(k_v_reply()),
    })
    .await
    .expect("fake server must bind");

    let topology = FakeTopology::addrs(vec![server.url()]);
    let mut io = RedisQueueIo::new(10, QueuePopCommand::Blpop);
    let cancel = CancellationToken::new();
    let delivered = Arc::new(Mutex::new(Vec::new()));

    queue_session(
        &topology,
        &mut io,
        &params(QueuePopCommand::Blpop, 1),
        &fast_policy(),
        &cancel,
        recording_deliver(Arc::clone(&delivered), cancel.clone()),
    )
    .await
    .expect("session must end Ok after cancellation");

    assert_eq!(
        *delivered.lock().expect("delivered list mutex"),
        vec![("k".to_string(), "v".to_string())],
        "exactly one item must be delivered"
    );
    assert_eq!(
        server.connection_count(),
        1,
        "a pop longer than the old 500 ms default must not force a reconnect"
    );
}

#[tokio::test]
async fn queue_brpop_longer_than_old_default_delivers_once() {
    let server = BlockingPopServer::start(BlockingPopPlan {
        delay: Duration::from_secs(1),
        reply: Some(k_v_reply()),
    })
    .await
    .expect("fake server must bind");

    let topology = FakeTopology::addrs(vec![server.url()]);
    let mut io = RedisQueueIo::new(10, QueuePopCommand::Brpop);
    let cancel = CancellationToken::new();
    let delivered = Arc::new(Mutex::new(Vec::new()));

    queue_session(
        &topology,
        &mut io,
        &params(QueuePopCommand::Brpop, 1),
        &fast_policy(),
        &cancel,
        recording_deliver(Arc::clone(&delivered), cancel.clone()),
    )
    .await
    .expect("session must end Ok after cancellation");

    assert!(
        server.command_names().iter().any(|name| name == "BRPOP"),
        "the server must receive a BRPOP request, got {:?}",
        server.command_names()
    );
    assert_eq!(
        *delivered.lock().expect("delivered list mutex"),
        vec![("k".to_string(), "v".to_string())],
        "exactly one item must be delivered"
    );
    assert_eq!(
        server.connection_count(),
        1,
        "a pop longer than the old 500 ms default must not force a reconnect"
    );
}

#[tokio::test]
async fn queue_timeout_zero_replies_after_old_default_are_delivered() {
    let server = BlockingPopServer::start(BlockingPopPlan {
        delay: Duration::from_millis(700),
        reply: Some(k_v_reply()),
    })
    .await
    .expect("fake server must bind");

    let topology = FakeTopology::addrs(vec![server.url()]);
    let mut io = RedisQueueIo::new(10, QueuePopCommand::Blpop);
    let cancel = CancellationToken::new();
    let delivered = Arc::new(Mutex::new(Vec::new()));

    queue_session(
        &topology,
        &mut io,
        &params(QueuePopCommand::Blpop, 0),
        &fast_policy(),
        &cancel,
        recording_deliver(Arc::clone(&delivered), cancel.clone()),
    )
    .await
    .expect("session must end Ok after cancellation");

    assert_eq!(
        *delivered.lock().expect("delivered list mutex"),
        vec![("k".to_string(), "v".to_string())],
        "a BLPOP key 0 reply after the old 500 ms default must be delivered"
    );
    assert_eq!(
        server.connection_count(),
        1,
        "the client must not time out at 500 ms and reconnect"
    );
}

#[tokio::test]
async fn queue_timeout_zero_withheld_reply_cancels_cleanly() {
    let mut server = BlockingPopServer::start(BlockingPopPlan {
        delay: Duration::ZERO,
        reply: None,
    })
    .await
    .expect("fake server must bind");

    let topology = FakeTopology::addrs(vec![server.url()]);
    let cancel = CancellationToken::new();
    let delivered = Arc::new(Mutex::new(Vec::new()));

    let session_cancel = cancel.clone();
    let session_deliver = recording_deliver(Arc::clone(&delivered), cancel.clone());
    let mut session = tokio::spawn(async move {
        let mut io = RedisQueueIo::new(10, QueuePopCommand::Blpop);
        queue_session(
            &topology,
            &mut io,
            &params(QueuePopCommand::Blpop, 0),
            &fast_policy(),
            &session_cancel,
            session_deliver,
        )
        .await
    });

    // Start the observation window only after the server has parsed the
    // BLPOP: otherwise connection/handshake startup could consume most of
    // the window and let the old 500 ms response timeout slip through.
    server.wait_for_blocking_pop().await;

    let early = tokio::time::timeout(Duration::from_millis(700), &mut session).await;
    assert!(
        early.is_err(),
        "the session must stay blocked while the reply is withheld"
    );
    assert!(
        delivered.lock().expect("delivered list mutex").is_empty(),
        "no item may be delivered while the reply is withheld"
    );
    assert_eq!(
        server.connection_count(),
        1,
        "the old 500 ms response timeout must not reconnect during the window"
    );

    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), session)
        .await
        .expect("session must end after cancellation")
        .expect("session task must not panic");
    result.expect("cancelled session must end Ok");
    assert_eq!(
        server.connection_count(),
        1,
        "a withheld reply must not force a reconnect before cancellation"
    );
}

#[tokio::test]
async fn producer_blpop_executes_past_old_driver_default() {
    let server = BlockingPopServer::start(BlockingPopPlan {
        delay: Duration::from_millis(800),
        reply: Some(k_v_reply()),
    })
    .await
    .expect("fake server must bind");

    let config =
        crate::config::RedisEndpointConfig::from_uri("redis://127.0.0.1:6379?command=BLPOP&key=k")
            .expect("BLPOP endpoint URI must parse");
    let topology = FakeTopology::addrs(vec![server.url()]);
    let mut executor = crate::executor::MultiplexedExecutor::new(config, Arc::new(topology));

    let mut exchange = Exchange::new(Message::default());
    exchange
        .input
        .set_header("CamelRedis.Timeout", serde_json::json!(1));
    exchange
        .input
        .set_header("CamelRedis.Key", serde_json::json!("k"));

    crate::executor::RedisCommandExecutor::execute_command(
        &mut executor,
        &crate::config::RedisCommand::Blpop,
        &mut exchange,
    )
    .await
    .expect("BLPOP past the old 500 ms driver default must succeed");

    match &exchange.input.body {
        Body::Json(value) => assert_eq!(
            value,
            &serde_json::json!("v"),
            "the popped value must land in the body per list::dispatch semantics"
        ),
        other => panic!("expected a JSON body carrying the popped value, got {other:?}"),
    }
}
