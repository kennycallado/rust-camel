//! Wire-level `XREADGROUP` slice tests against a scripted RESP server.
//!
//! These tests drive the real [`crate::consumer::RedisConsumer`] in Streams
//! mode through the real [`crate::streams::RedisStreamIo`] connection, so the
//! bytes on the wire are exercised end to end: the exact `XREADGROUP`
//! arguments, the `XACK` argument vectors, and the no-ack paths (pipeline
//! failure, channel close, cancellation while blocked, budget exhaustion).
//!
//! The fake server replays a scripted plan of explicit actions. It records the
//! full argument vector of every non-handshake command, answers unmatched
//! application commands (`:1` for `XACK`, nil otherwise), and can withhold a
//! reply or drop the connection.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use camel_component_api::CamelError;
use camel_component_api::Consumer;
use camel_component_api::ConsumerContext;
use camel_component_api::Exchange;
use camel_component_api::ExchangeEnvelope;
use camel_component_api::NetworkRetryPolicy;
use camel_component_api::RuntimeObservability;
use camel_component_api::test_support::NoopRuntimeObservability;
use camel_component_api::test_support::RecordingRuntimeObservability;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::config::RedisEndpointConfig;
use crate::consumer::RedisConsumer;

/// RESP reply for a single stream entry `1-1` with `payload=hello`.
const ONE_ENTRY_REPLY: &[u8] =
    b"*1\r\n*2\r\n$2\r\ns1\r\n*1\r\n*2\r\n$3\r\n1-1\r\n*2\r\n$7\r\npayload\r\n$5\r\nhello\r\n";

/// RESP reply for two stream entries: `1-1` and `2-2` (`a=1`, `b=2`).
const TWO_ENTRY_REPLY: &[u8] = b"*1\r\n*2\r\n$2\r\ns1\r\n*2\r\n*2\r\n$3\r\n1-1\r\n*2\r\n$7\r\npayload\r\n$5\r\nhello\r\n*2\r\n$3\r\n2-2\r\n*4\r\n$1\r\na\r\n$1\r\n1\r\n$1\r\nb\r\n$1\r\n2\r\n";

/// RESP integer reply acknowledging one entry.
const XACK_REPLY: &[u8] = b":1\r\n";

/// One recorded application command: the upper-cased name and its full
/// argument vector (the command name itself is not part of the arguments).
type RecordedFrame = (String, Vec<Vec<u8>>);

/// Programmed server behavior consumed in order across connections.
#[derive(Debug, Clone)]
enum ScriptedAction {
    /// Answer the named command with `reply`; a non-matching command gets the
    /// default reply and leaves this action for the next frame.
    Reply {
        matches_command: &'static str,
        reply: Vec<u8>,
    },
    /// Read and drop the next application frame without replying, leaving the
    /// client blocked.
    Withhold,
    /// Drop the connection immediately.
    Close,
}

/// Mutable plan shared by the accept loop and every served connection.
struct ScriptedPlan {
    actions: VecDeque<ScriptedAction>,
    /// Once this plan is exhausted, every new connection is closed at accept
    /// (the repeat-close behavior the budget-exhaustion test relies on).
    close_on_exhaustion: bool,
}

/// What to do with one application frame.
enum FrameStep {
    /// Write these bytes back.
    Write(Vec<u8>),
    /// Consume the frame without a reply.
    Skip,
    /// Drop the connection.
    Close,
    /// Write the default reply for the command.
    Default,
}

/// A loopback RESP server that accepts every connection until shutdown.
struct StreamFakeServer {
    addr: SocketAddr,
    connections: Arc<AtomicUsize>,
    commands: watch::Receiver<Vec<RecordedFrame>>,
}

impl StreamFakeServer {
    /// Bind on an ephemeral loopback port and start serving `actions`.
    async fn start(actions: Vec<ScriptedAction>) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let connections = Arc::new(AtomicUsize::new(0));
        let (commands_tx, commands) = watch::channel(Vec::new());
        let close_on_exhaustion = actions
            .iter()
            .any(|action| matches!(action, ScriptedAction::Close));
        let plan = Arc::new(Mutex::new(ScriptedPlan {
            actions: actions.into(),
            close_on_exhaustion,
        }));
        let accepted = Arc::clone(&connections);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                accepted.fetch_add(1, Ordering::SeqCst);
                let plan = Arc::clone(&plan);
                let commands = commands_tx.clone();
                if close_connection(&plan) {
                    drop(stream);
                    continue;
                }
                tokio::spawn(async move {
                    serve_connection(stream, plan, commands).await;
                });
            }
        });
        Ok(Self {
            addr,
            connections,
            commands,
        })
    }

    /// Port the server is bound to.
    fn port(&self) -> u16 {
        self.addr.port()
    }

    /// How many connections have been accepted so far.
    fn connection_count(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// Snapshot of every recorded application command.
    fn recorded(&self) -> Vec<RecordedFrame> {
        self.commands.borrow().clone()
    }

    /// Recorded argument vectors for every command named `name`.
    fn args_of(&self, name: &str) -> Vec<Vec<Vec<u8>>> {
        self.recorded()
            .into_iter()
            .filter(|(recorded_name, _)| recorded_name == name)
            .map(|(_, args)| args)
            .collect()
    }

    /// Recorded application command names.
    fn command_names(&self) -> Vec<String> {
        self.recorded().into_iter().map(|(name, _)| name).collect()
    }

    /// Wait (bounded) until `count` commands named `name` have been recorded.
    async fn wait_for_command(&mut self, name: &str, count: usize) {
        let what = format!("{name} x{count}");
        let observed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let seen = self
                    .commands
                    .borrow()
                    .iter()
                    .filter(|(recorded_name, _)| recorded_name == name)
                    .count();
                if seen >= count {
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
            "fake server must observe {what} within 5s"
        );
        let seen = self
            .commands
            .borrow()
            .iter()
            .filter(|(recorded_name, _)| recorded_name == name)
            .count();
        assert!(
            seen >= count,
            "fake server must have observed {what}, saw {seen}"
        );
    }
}

/// Pop a connection-level `Close` action, or close because the plan is
/// exhausted after a `Close` action was scripted.
fn close_connection(plan: &Mutex<ScriptedPlan>) -> bool {
    let mut plan = plan.lock().expect("scripted plan mutex");
    if matches!(plan.actions.front(), Some(ScriptedAction::Close)) {
        plan.actions.pop_front();
        return true;
    }
    plan.actions.is_empty() && plan.close_on_exhaustion
}

/// Decide what to do with the frame named `name`, consuming a matching action.
fn next_step(plan: &Mutex<ScriptedPlan>, name: &str) -> FrameStep {
    let mut plan = plan.lock().expect("scripted plan mutex");
    let consume = match plan.actions.front() {
        Some(ScriptedAction::Reply {
            matches_command, ..
        }) => *matches_command == name,
        Some(ScriptedAction::Withhold | ScriptedAction::Close) => true,
        None => false,
    };
    if !consume {
        return if plan.actions.is_empty() && plan.close_on_exhaustion {
            FrameStep::Close
        } else {
            FrameStep::Default
        };
    }
    match plan.actions.pop_front() {
        Some(ScriptedAction::Reply { reply, .. }) => FrameStep::Write(reply),
        Some(ScriptedAction::Withhold) => FrameStep::Skip,
        Some(ScriptedAction::Close) | None => FrameStep::Close,
    }
}

/// Serve one connection until the peer disconnects or a write fails.
async fn serve_connection(
    mut stream: TcpStream,
    plan: Arc<Mutex<ScriptedPlan>>,
    commands: watch::Sender<Vec<RecordedFrame>>,
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
        commands.send_modify(|recorded| recorded.push((name.clone(), arguments[1..].to_vec())));
        match next_step(&plan, &name) {
            FrameStep::Write(reply) => {
                if stream.write_all(&reply).await.is_err() {
                    break;
                }
            }
            FrameStep::Skip => {}
            FrameStep::Close => break,
            FrameStep::Default => {
                let reply: &[u8] = if name == "XACK" {
                    b":1\r\n"
                } else {
                    b"$-1\r\n"
                };
                if stream.write_all(reply).await.is_err() {
                    break;
                }
            }
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

/// Stream consumer config pointed at `port`, with `extra` query parameters.
fn stream_config(port: u16, extra: &str) -> RedisEndpointConfig {
    RedisEndpointConfig::from_uri(&format!(
        "redis://127.0.0.1:{port}?command=XREADGROUP&key=s1&group=g&consumer=c{extra}"
    ))
    .expect("stream endpoint URI must parse")
}

/// Render a recorded argument vector as UTF-8 strings for assertions.
fn args_as_strings(args: &[Vec<u8>]) -> Vec<String> {
    args.iter()
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect()
}

/// Bounded receive of one delivered envelope.
async fn recv_envelope(rx: &mut mpsc::Receiver<ExchangeEnvelope>) -> ExchangeEnvelope {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("consumer must deliver an exchange within 5s")
        .expect("consumer delivery channel must stay open")
}

/// Split a delivered envelope into its exchange and reply channel.
fn split_envelope(
    envelope: ExchangeEnvelope,
) -> (Exchange, oneshot::Sender<Result<Exchange, CamelError>>) {
    let reply_tx = envelope
        .reply_tx
        .expect("stream delivery must request a reply");
    (envelope.exchange, reply_tx)
}

/// Read the `CamelRedis.StreamId` header off a delivered exchange.
fn stream_id(exchange: &Exchange) -> String {
    exchange
        .input
        .header("CamelRedis.StreamId")
        .and_then(|value| value.as_str())
        .expect("stream delivery must carry a CamelRedis.StreamId string header")
        .to_string()
}

/// Join a consumer task under a deadline.
async fn join_consumer(consumer: &mut RedisConsumer) -> Result<(), CamelError> {
    let handle = consumer
        .background_task_handle()
        .expect("consumer must expose its task handle after start");
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("consumer task must finish within 5s")
        .expect("consumer task must not panic")
}

#[tokio::test]
async fn fake_server_one_entry_delivered_and_acked_on_wire() {
    // The trailing Withhold parks the connection after the ack so the read
    // loop cannot issue unbounded extra XREADGROUPs against the fake.
    let mut server = StreamFakeServer::start(vec![
        ScriptedAction::Reply {
            matches_command: "XREADGROUP",
            reply: ONE_ENTRY_REPLY.to_vec(),
        },
        ScriptedAction::Reply {
            matches_command: "XACK",
            reply: XACK_REPLY.to_vec(),
        },
        ScriptedAction::Withhold,
    ])
    .await
    .expect("fake server must bind");

    let config = stream_config(server.port(), "&count=10&block=5000");
    let rt = RecordingRuntimeObservability::new(false);
    let mut consumer = RedisConsumer::new(config, rt).expect("XREADGROUP consumer must construct");
    let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "stream-test-route".to_string());
    consumer.start(ctx).await.expect("consumer must start");

    let (exchange, reply_tx) = split_envelope(recv_envelope(&mut rx).await);
    assert_eq!(
        exchange.input.header("CamelRedis.StreamId"),
        Some(&serde_json::json!("1-1")),
        "delivered exchange must carry the stream entry id"
    );
    assert_eq!(
        exchange.input.body.as_text(),
        Some("hello"),
        "single-field entry must deliver its field value as the body"
    );
    reply_tx
        .send(Ok(exchange))
        .expect("pipeline must accept the reply");

    server.wait_for_command("XACK", 1).await;

    let commands = server.recorded();
    assert!(
        commands.iter().any(|(name, args)| {
            name.as_str() == "XREADGROUP"
                && args_as_strings(args)
                    == vec![
                        "GROUP", "g", "c", "COUNT", "10", "BLOCK", "5000", "STREAMS", "s1", ">",
                    ]
        }),
        "recorded commands must contain the exact XREADGROUP wire arguments: {commands:?}"
    );
    let xack = server.args_of("XACK");
    assert_eq!(
        xack.len(),
        1,
        "exactly one XACK must reach the wire: {xack:?}"
    );
    assert_eq!(args_as_strings(&xack[0]), vec!["s1", "g", "1-1"]);

    cancel.cancel();
    let joined = join_consumer(&mut consumer).await;
    assert!(
        joined.is_ok(),
        "cancelled stream session must end Ok: {joined:?}"
    );
}

#[tokio::test]
async fn fake_server_pipeline_failure_no_ack_live_wiring() {
    let mut server = StreamFakeServer::start(vec![
        ScriptedAction::Reply {
            matches_command: "XREADGROUP",
            reply: TWO_ENTRY_REPLY.to_vec(),
        },
        ScriptedAction::Reply {
            matches_command: "XACK",
            reply: XACK_REPLY.to_vec(),
        },
        ScriptedAction::Withhold,
    ])
    .await
    .expect("fake server must bind");

    let config = stream_config(server.port(), "");
    let rt = RecordingRuntimeObservability::new(false);
    let rt_for_consumer: Arc<dyn RuntimeObservability> = rt.clone();
    let mut consumer =
        RedisConsumer::new(config, rt_for_consumer).expect("consumer must construct");
    let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "stream-test-route".to_string());
    consumer.start(ctx).await.expect("consumer must start");

    let mut delivered = Vec::new();
    for expected in ["1-1", "2-2"] {
        let (exchange, reply_tx) = split_envelope(recv_envelope(&mut rx).await);
        let id = stream_id(&exchange);
        assert_eq!(id, expected, "entries must be delivered in order");
        delivered.push(id);
        let outcome = if expected == "1-1" {
            Err(CamelError::InvalidUri("pipeline failed".to_string()))
        } else {
            Ok(exchange)
        };
        reply_tx
            .send(outcome)
            .expect("pipeline must accept the reply");
    }
    assert_eq!(
        delivered,
        vec!["1-1".to_string(), "2-2".to_string()],
        "delivery must continue after a failed entry"
    );

    server.wait_for_command("XACK", 1).await;
    let xack = server.args_of("XACK");
    assert_eq!(
        xack.len(),
        1,
        "only the accepted entry may be acknowledged: {xack:?}"
    );
    assert_eq!(args_as_strings(&xack[0]), vec!["s1", "g", "2-2"]);
    assert!(
        rt.errors().is_empty(),
        "a pipeline failure must not record an error metric: {:?}",
        rt.errors()
    );

    cancel.cancel();
    let joined = join_consumer(&mut consumer).await;
    assert!(
        joined.is_ok(),
        "cancelled stream session must end Ok: {joined:?}"
    );
}

#[tokio::test]
async fn fake_server_channel_closed_no_ack_supervision() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Reply {
        matches_command: "XREADGROUP",
        reply: ONE_ENTRY_REPLY.to_vec(),
    }])
    .await
    .expect("fake server must bind");

    let config = stream_config(server.port(), "");
    let rt = RecordingRuntimeObservability::new(false);
    let rt_for_consumer: Arc<dyn RuntimeObservability> = rt.clone();
    let mut consumer =
        RedisConsumer::new(config, rt_for_consumer).expect("consumer must construct");
    let (tx, rx) = mpsc::channel::<ExchangeEnvelope>(16);
    // No pipeline at all: the handoff target is already gone before the first
    // delivery, so the session must fail without acknowledging the entry.
    drop(rx);
    let ctx = ConsumerContext::new(
        tx,
        CancellationToken::new(),
        "stream-test-route".to_string(),
    );
    consumer.start(ctx).await.expect("consumer must start");

    let joined = join_consumer(&mut consumer).await;
    assert!(
        joined.is_err(),
        "a closed delivery channel must fail supervision: {joined:?}"
    );
    assert!(
        server.args_of("XACK").is_empty(),
        "a closed channel must never acknowledge an entry"
    );
    let errors = rt.errors();
    let channel_closed = errors
        .iter()
        .filter(|(route, label)| {
            route == "stream-test-route" && label == "b-prime:redis:xreadgroup-channel-closed"
        })
        .count();
    assert_eq!(
        channel_closed, 1,
        "the channel-closed metric must be recorded exactly once: {errors:?}"
    );
}

#[tokio::test]
async fn fake_server_cancel_while_blocked_no_ack() {
    let mut server = StreamFakeServer::start(vec![ScriptedAction::Withhold])
        .await
        .expect("fake server must bind");

    let config = stream_config(server.port(), "");
    let mut consumer = RedisConsumer::new(config, Arc::new(NoopRuntimeObservability))
        .expect("consumer must construct");
    let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "stream-test-route".to_string());
    consumer.start(ctx).await.expect("consumer must start");

    // Wait until the XREADGROUP is on the wire and its reply is withheld, so
    // the cancellation is fired while the read is genuinely blocked.
    server.wait_for_command("XREADGROUP", 1).await;
    cancel.cancel();

    let joined = join_consumer(&mut consumer).await;
    assert!(
        joined.is_ok(),
        "cancelled blocked stream session must end Ok: {joined:?}"
    );
    assert!(
        server.args_of("XACK").is_empty(),
        "a blocked read must not acknowledge anything"
    );
    assert_eq!(
        server.connection_count(),
        1,
        "cancellation must not trigger a reconnect"
    );
}

#[tokio::test]
async fn fake_server_budget_exhaustion_issues_no_blpop() {
    let server = StreamFakeServer::start(vec![ScriptedAction::Close])
        .await
        .expect("fake server must bind");

    let mut config = stream_config(server.port(), "");
    config.reconnect = NetworkRetryPolicy {
        max_attempts: 2,
        initial_delay: Duration::ZERO,
        ..NetworkRetryPolicy::default()
    };
    let mut consumer = RedisConsumer::new(config, Arc::new(NoopRuntimeObservability))
        .expect("consumer must construct");
    let (tx, _rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let ctx = ConsumerContext::new(
        tx,
        CancellationToken::new(),
        "stream-test-route".to_string(),
    );
    consumer.start(ctx).await.expect("consumer must start");

    let joined = join_consumer(&mut consumer).await;
    assert!(
        joined.is_err(),
        "exhausted reconnect budget must fail supervision: {joined:?}"
    );
    assert!(
        server.connection_count() >= 1,
        "the consumer must have attempted a connection"
    );
    let names = server.command_names();
    assert!(
        names.iter().all(|name| name != "BLPOP" && name != "BRPOP"),
        "Streams mode must never fall back to a blocking pop: {names:?}"
    );
}
