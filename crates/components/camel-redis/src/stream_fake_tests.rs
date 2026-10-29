//! Wire-level stream tests against a scripted RESP server.
//!
//! These tests drive the real [`crate::consumer::RedisConsumer`] in Streams
//! mode through the real [`crate::streams::RedisStreamIo`] halves, so the
//! bytes on the wire are exercised end to end: the exact `XREADGROUP`,
//! `XPENDING`, `XAUTOCLAIM`, and `XACK` argument vectors, plus the no-ack
//! paths (pipeline failure, channel close, cancellation while blocked, budget
//! exhaustion).
//!
//! The fake server replays a scripted plan of explicit actions. It records the
//! full argument vector of every non-handshake command per connection, answers
//! unmatched application commands (`:1` for `XACK`, nil otherwise), can
//! withhold, defer, or drop a reply, and records when a connection ends.
//! Read-half and recovery-half traffic is targeted by connection id, so the
//! two independent sockets can be scripted deterministically.

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
use tokio::sync::Notify;
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

/// Encode a RESP bulk string.
fn resp_bulk(value: &str) -> Vec<u8> {
    let mut out = format!("${}\r\n", value.len()).into_bytes();
    out.extend_from_slice(value.as_bytes());
    out.extend_from_slice(b"\r\n");
    out
}

/// Encode a RESP array of already-encoded elements.
fn resp_array(items: &[Vec<u8>]) -> Vec<u8> {
    let mut out = format!("*{}\r\n", items.len()).into_bytes();
    for item in items {
        out.extend_from_slice(item);
    }
    out
}

/// Encode a RESP integer.
fn resp_int(value: i64) -> Vec<u8> {
    format!(":{value}\r\n").into_bytes()
}

/// RESP nil bulk string (parsed as `redis::Value::Nil`).
fn resp_nil() -> Vec<u8> {
    b"$-1\r\n".to_vec()
}

/// Aggregate `XPENDING <key> <group>` summary:
/// `[count, min-id, max-id, [[consumer, count]]]`.
fn xpending_summary(count: u64) -> Vec<u8> {
    resp_array(&[
        resp_int(count as i64),
        resp_bulk("1-1"),
        resp_bulk("1-1"),
        resp_array(&[resp_array(&[resp_bulk("c"), resp_int(count as i64)])]),
    ])
}

/// Targeted `XPENDING <key> <group> IDLE 0 <id> <id> 1` row with a delivery
/// count.
fn xpending_targeted(id: &str, delivery_count: u64) -> Vec<u8> {
    resp_array(&[resp_array(&[
        resp_bulk(id),
        resp_bulk("c"),
        resp_int(0),
        resp_int(delivery_count as i64),
    ])])
}

/// One `XAUTOCLAIM` entry: `[id, [field, value, ...]]`.
fn claim_entry(id: &str, fields: &[(&str, &str)]) -> Vec<u8> {
    let flat: Vec<Vec<u8>> = fields
        .iter()
        .flat_map(|(field, value)| [resp_bulk(field), resp_bulk(value)])
        .collect();
    resp_array(&[resp_bulk(id), resp_array(&flat)])
}

/// Two-element `XAUTOCLAIM` reply: `[next-cursor, entries]`.
fn xautoclaim_reply(cursor: &str, entries: &[Vec<u8>]) -> Vec<u8> {
    resp_array(&[resp_bulk(cursor), resp_array(entries)])
}

/// Three-element `XAUTOCLAIM` reply:
/// `[next-cursor, entries, deleted-ids]`.
fn xautoclaim_reply_with_deleted(cursor: &str, entries: &[Vec<u8>], deleted: &[&str]) -> Vec<u8> {
    let deleted: Vec<Vec<u8>> = deleted.iter().map(|id| resp_bulk(id)).collect();
    resp_array(&[resp_bulk(cursor), resp_array(entries), resp_array(&deleted)])
}

/// One recorded application command: the upper-cased name and its full
/// argument vector (the command name itself is not part of the arguments).
type RecordedFrame = (String, Vec<Vec<u8>>);

/// One recorded application command with the connection that issued it.
struct WireFrame {
    connection: usize,
    name: String,
    args: Vec<Vec<u8>>,
}

/// Programmed server behavior consumed in order per connection.
///
/// Unqualified actions apply to any connection; `*On` actions target one
/// connection id (assignment order: the read half connects first, the
/// recovery half second). Actions targeted at another connection stay queued
/// for it and never consume the frame of the current connection.
#[derive(Clone)]
enum ScriptedAction {
    /// Answer the named command with `reply`; a non-matching command gets the
    /// default reply and leaves this action for the next frame.
    Reply {
        matches_command: &'static str,
        reply: Vec<u8>,
    },
    /// Connection-qualified [`ScriptedAction::Reply`].
    ReplyOn {
        connection: usize,
        matches_command: &'static str,
        reply: Vec<u8>,
    },
    /// Read and drop the next application frame without replying, leaving the
    /// client blocked.
    Withhold,
    /// Connection-qualified [`ScriptedAction::Withhold`].
    WithholdOn { connection: usize },
    /// Read the named command, record it, then wait for `release` before
    /// writing `reply`. Lets a test fire cancellation while the client is
    /// genuinely waiting and only then complete the exchange.
    DeferredOn {
        connection: usize,
        matches_command: &'static str,
        reply: Vec<u8>,
        release: Arc<Notify>,
    },
    /// Drop the connection immediately.
    Close,
}

impl ScriptedAction {
    /// Whether this action may consume a frame from `connection`.
    fn applies_to(&self, connection: usize) -> bool {
        match self {
            Self::Reply { .. } | Self::Withhold | Self::Close => true,
            Self::ReplyOn {
                connection: target, ..
            }
            | Self::WithholdOn { connection: target }
            | Self::DeferredOn {
                connection: target, ..
            } => *target == connection,
        }
    }

    /// Whether this action ends the connection at accept time.
    fn is_close(&self) -> bool {
        matches!(self, Self::Close)
    }
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
    /// Wait for `release`, then write `reply`.
    Deferred {
        reply: Vec<u8>,
        release: Arc<Notify>,
    },
    /// Drop the connection.
    Close,
    /// Write the default reply for the command.
    Default,
}

/// A loopback RESP server that accepts every connection until shutdown.
struct StreamFakeServer {
    addr: SocketAddr,
    connections: Arc<AtomicUsize>,
    commands: watch::Receiver<Vec<WireFrame>>,
    closed: watch::Receiver<Vec<usize>>,
}

impl StreamFakeServer {
    /// Bind on an ephemeral loopback port and start serving `actions`.
    async fn start(actions: Vec<ScriptedAction>) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let connections = Arc::new(AtomicUsize::new(0));
        let (commands_tx, commands) = watch::channel(Vec::new());
        let (closed_tx, closed) = watch::channel(Vec::new());
        let close_on_exhaustion = actions.iter().any(ScriptedAction::is_close);
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
                let connection = accepted.fetch_add(1, Ordering::SeqCst);
                let plan = Arc::clone(&plan);
                let commands = commands_tx.clone();
                let closed = closed_tx.clone();
                if close_connection(&plan, connection) {
                    closed.send_modify(|ids| ids.push(connection));
                    drop(stream);
                    continue;
                }
                tokio::spawn(async move {
                    serve_connection(stream, connection, plan, commands, closed).await;
                });
            }
        });
        Ok(Self {
            addr,
            connections,
            commands,
            closed,
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
        self.commands
            .borrow()
            .iter()
            .map(|frame| (frame.name.clone(), frame.args.clone()))
            .collect()
    }

    /// Snapshot of every recorded application command with its connection id.
    fn recorded_per_connection(&self) -> Vec<(usize, RecordedFrame)> {
        self.commands
            .borrow()
            .iter()
            .map(|frame| (frame.connection, (frame.name.clone(), frame.args.clone())))
            .collect()
    }

    /// Recorded argument vectors for every command named `name`.
    fn args_of(&self, name: &str) -> Vec<Vec<Vec<u8>>> {
        self.commands
            .borrow()
            .iter()
            .filter(|frame| frame.name == name)
            .map(|frame| frame.args.clone())
            .collect()
    }

    /// Recorded argument vectors for command `name` on `connection`.
    fn args_of_on_conn(&self, connection: usize, name: &str) -> Vec<Vec<Vec<u8>>> {
        self.commands
            .borrow()
            .iter()
            .filter(|frame| frame.connection == connection && frame.name == name)
            .map(|frame| frame.args.clone())
            .collect()
    }

    /// Recorded application command names across every connection.
    fn command_names(&self) -> Vec<String> {
        self.commands
            .borrow()
            .iter()
            .map(|frame| frame.name.clone())
            .collect()
    }

    /// Recorded application command names on `connection`.
    fn command_names_on_conn(&self, connection: usize) -> Vec<String> {
        self.commands
            .borrow()
            .iter()
            .filter(|frame| frame.connection == connection)
            .map(|frame| frame.name.clone())
            .collect()
    }

    /// Number of recorded frames named `name` on `connection`.
    fn count_on_conn(&self, connection: usize, name: &str) -> usize {
        self.commands
            .borrow()
            .iter()
            .filter(|frame| frame.connection == connection && frame.name == name)
            .count()
    }

    /// Wait (bounded) until `predicate` holds over the recorded frames.
    async fn wait_for_frames<F>(&mut self, what: &str, predicate: F)
    where
        F: Fn(&[WireFrame]) -> bool,
    {
        let observed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if predicate(&self.commands.borrow()) {
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
        assert!(
            predicate(&self.commands.borrow()),
            "fake server must have observed {what}"
        );
    }

    /// Wait (bounded) until `count` commands named `name` have been recorded.
    async fn wait_for_command(&mut self, name: &str, count: usize) {
        let what = format!("{name} x{count}");
        self.wait_for_frames(&what, |frames| {
            frames.iter().filter(|frame| frame.name == name).count() >= count
        })
        .await;
    }

    /// Wait (bounded) until `count` commands named `name` were recorded on
    /// `connection`.
    async fn wait_for_command_on_conn(&mut self, connection: usize, name: &str, count: usize) {
        let what = format!("{name} x{count} on connection {connection}");
        self.wait_for_frames(&what, |frames| {
            frames
                .iter()
                .filter(|frame| frame.connection == connection && frame.name == name)
                .count()
                >= count
        })
        .await;
    }

    /// Wait (bounded) until the server observed `connection` ending.
    async fn wait_for_connection_close(&mut self, connection: usize) {
        let what = format!("connection {connection} close");
        let observed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if self.closed.borrow().contains(&connection) {
                    return;
                }
                if self.closed.changed().await.is_err() {
                    return;
                }
            }
        })
        .await;
        assert!(
            observed.is_ok(),
            "fake server must observe {what} within 5s"
        );
        assert!(
            self.closed.borrow().contains(&connection),
            "fake server must have observed {what}"
        );
    }
}

/// Pop a connection-level `Close` action, or close because the plan is
/// exhausted after a `Close` action was scripted.
fn close_connection(plan: &Mutex<ScriptedPlan>, connection: usize) -> bool {
    let mut plan = plan.lock().expect("scripted plan mutex");
    let index = plan
        .actions
        .iter()
        .position(|action| action.applies_to(connection));
    if let Some(index) = index {
        if plan.actions[index].is_close() {
            plan.actions.remove(index);
            return true;
        }
        return false;
    }
    plan.actions.is_empty() && plan.close_on_exhaustion
}

/// Decide what to do with the frame named `name`, consuming a matching action
/// for `connection`. Actions targeted at other connections stay queued.
fn next_step(plan: &Mutex<ScriptedPlan>, connection: usize, name: &str) -> FrameStep {
    let mut plan = plan.lock().expect("scripted plan mutex");
    let Some(index) = plan
        .actions
        .iter()
        .position(|action| action.applies_to(connection))
    else {
        return if plan.actions.is_empty() && plan.close_on_exhaustion {
            FrameStep::Close
        } else {
            FrameStep::Default
        };
    };
    let consume = match &plan.actions[index] {
        ScriptedAction::Reply {
            matches_command, ..
        }
        | ScriptedAction::ReplyOn {
            matches_command, ..
        }
        | ScriptedAction::DeferredOn {
            matches_command, ..
        } => *matches_command == name,
        ScriptedAction::Withhold | ScriptedAction::WithholdOn { .. } | ScriptedAction::Close => {
            true
        }
    };
    if !consume {
        return FrameStep::Default;
    }
    match plan.actions.remove(index) {
        Some(ScriptedAction::Reply { reply, .. } | ScriptedAction::ReplyOn { reply, .. }) => {
            FrameStep::Write(reply)
        }
        Some(ScriptedAction::DeferredOn { reply, release, .. }) => {
            FrameStep::Deferred { reply, release }
        }
        Some(ScriptedAction::Withhold | ScriptedAction::WithholdOn { .. }) => FrameStep::Skip,
        Some(ScriptedAction::Close) | None => FrameStep::Close,
    }
}

/// Serve one connection until the peer disconnects or a write fails.
async fn serve_connection(
    mut stream: TcpStream,
    connection: usize,
    plan: Arc<Mutex<ScriptedPlan>>,
    commands: watch::Sender<Vec<WireFrame>>,
    closed: watch::Sender<Vec<usize>>,
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
        commands.send_modify(|recorded| {
            recorded.push(WireFrame {
                connection,
                name: name.clone(),
                args: arguments[1..].to_vec(),
            });
        });
        match next_step(&plan, connection, &name) {
            FrameStep::Write(reply) => {
                if stream.write_all(&reply).await.is_err() {
                    break;
                }
            }
            FrameStep::Skip => {}
            FrameStep::Deferred { reply, release } => {
                release.notified().await;
                if stream.write_all(&reply).await.is_err() {
                    break;
                }
            }
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
    closed.send_modify(|ids| ids.push(connection));
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

/// Start a real stream consumer against `server` with `extra` URI parameters.
async fn start_stream_consumer(
    server: &StreamFakeServer,
    extra: &str,
    rt: Arc<RecordingRuntimeObservability>,
) -> (
    RedisConsumer,
    mpsc::Receiver<ExchangeEnvelope>,
    CancellationToken,
) {
    let config = stream_config(server.port(), extra);
    let runtime: Arc<dyn RuntimeObservability> = rt;
    let mut consumer = RedisConsumer::new(config, runtime).expect("consumer must construct");
    let (tx, rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "stream-test-route".to_string());
    consumer.start(ctx).await.expect("consumer must start");
    (consumer, rx, cancel)
}

/// Recovery options: enabled with the given idle floor and batch size.
fn recovery_extra(min_idle_ms: u64, batch: u64) -> String {
    format!(
        "&streamRecoverEnabled=true&streamRecoverMinIdleMs={min_idle_ms}\
         &streamRecoverBatch={batch}&block=0"
    )
}

/// Reply to a claimed entry's targeted `XPENDING` read with `count`.
fn targeted_delivery_count(id: &str, count: u64) -> ScriptedAction {
    ScriptedAction::ReplyOn {
        connection: 1,
        matches_command: "XPENDING",
        reply: xpending_targeted(id, count),
    }
}

/// Reply to the recovery connection's aggregate `XPENDING` summary.
fn summary_reply(count: u64) -> ScriptedAction {
    ScriptedAction::ReplyOn {
        connection: 1,
        matches_command: "XPENDING",
        reply: xpending_summary(count),
    }
}

/// Reply to the recovery connection's `XAUTOCLAIM` frame.
fn claim_reply_action(cursor: &str, entries: &[Vec<u8>]) -> ScriptedAction {
    ScriptedAction::ReplyOn {
        connection: 1,
        matches_command: "XAUTOCLAIM",
        reply: xautoclaim_reply(cursor, entries),
    }
}

/// Park the read connection after its first application frame.
fn park_read_connection() -> ScriptedAction {
    ScriptedAction::WithholdOn { connection: 0 }
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

#[tokio::test]
async fn fake_server_recovery_command_shape_on_wire() {
    let mut server = StreamFakeServer::start(vec![
        summary_reply(1),
        claim_reply_action("0-0", &[]),
        park_read_connection(),
    ])
    .await
    .expect("fake server must bind");

    let rt = RecordingRuntimeObservability::new(false);
    let (mut consumer, _rx, cancel) =
        start_stream_consumer(&server, &recovery_extra(30_000, 5), Arc::clone(&rt)).await;

    // The recovery half opens its pass with the aggregate summary, then
    // claims from the first cursor with the configured idle floor and budget.
    server.wait_for_command_on_conn(1, "XPENDING", 1).await;
    server.wait_for_command_on_conn(1, "XAUTOCLAIM", 1).await;
    let claims = server.args_of_on_conn(1, "XAUTOCLAIM");
    assert_eq!(
        claims.len(),
        1,
        "the first recovery pass must issue exactly one claim: {claims:?}"
    );
    assert_eq!(
        args_as_strings(&claims[0]),
        vec!["s1", "g", "c", "30000", "0-0", "COUNT", "5"],
        "the claim frame must carry the configured idle floor and batch"
    );
    let recovery_frames: Vec<String> = server
        .recorded_per_connection()
        .into_iter()
        .filter(|(connection, _)| *connection == 1)
        .map(|(_, (name, _))| name)
        .collect();
    assert_eq!(
        recovery_frames,
        vec!["XPENDING".to_string(), "XAUTOCLAIM".to_string()],
        "the recovery connection must only carry recovery commands"
    );

    server.wait_for_command_on_conn(0, "XREADGROUP", 1).await;
    assert_eq!(
        server.command_names_on_conn(0),
        vec!["XREADGROUP".to_string()],
        "the read connection must carry XREADGROUP frames only"
    );

    cancel.cancel();
    let joined = join_consumer(&mut consumer).await;
    assert!(
        joined.is_ok(),
        "cancelled stream session must end Ok: {joined:?}"
    );
}

#[tokio::test]
async fn fake_server_claimed_delivery_count_and_ack_on_recovery_conn() {
    let mut server = StreamFakeServer::start(vec![
        summary_reply(1),
        claim_reply_action("0-0", &[claim_entry("1-1", &[("payload", "hello")])]),
        targeted_delivery_count("1-1", 3),
        park_read_connection(),
    ])
    .await
    .expect("fake server must bind");

    let rt = RecordingRuntimeObservability::new(false);
    let (mut consumer, mut rx, cancel) =
        start_stream_consumer(&server, &recovery_extra(30_000, 5), Arc::clone(&rt)).await;

    let (exchange, reply_tx) = split_envelope(recv_envelope(&mut rx).await);
    assert_eq!(stream_id(&exchange), "1-1");
    assert_eq!(
        exchange.input.header("CamelRedis.StreamDeliveryCount"),
        Some(&serde_json::json!("3")),
        "the claimed delivery count must come from the targeted XPENDING row"
    );
    reply_tx
        .send(Ok(exchange))
        .expect("pipeline must accept the reply");

    server.wait_for_command_on_conn(1, "XACK", 1).await;
    let acks = server.args_of_on_conn(1, "XACK");
    assert_eq!(
        acks.len(),
        1,
        "exactly one XACK must land on the recovery connection: {acks:?}"
    );
    assert_eq!(args_as_strings(&acks[0]), vec!["s1", "g", "1-1"]);
    assert!(
        server.args_of_on_conn(0, "XACK").is_empty(),
        "the read connection must not acknowledge a recovered entry"
    );

    cancel.cancel();
    let joined = join_consumer(&mut consumer).await;
    assert!(
        joined.is_ok(),
        "cancelled stream session must end Ok: {joined:?}"
    );
}

#[tokio::test]
async fn fake_server_empty_nonzero_continuation_then_claim() {
    let mut server = StreamFakeServer::start(vec![
        summary_reply(1),
        ScriptedAction::ReplyOn {
            connection: 1,
            matches_command: "XAUTOCLAIM",
            reply: xautoclaim_reply_with_deleted("5-0", &[], &["9-9"]),
        },
        claim_reply_action("0-0", &[claim_entry("7-7", &[("payload", "hello")])]),
        targeted_delivery_count("7-7", 1),
        park_read_connection(),
    ])
    .await
    .expect("fake server must bind");

    let rt = RecordingRuntimeObservability::new(false);
    let (mut consumer, mut rx, cancel) =
        start_stream_consumer(&server, &recovery_extra(30_000, 5), Arc::clone(&rt)).await;

    let (exchange, reply_tx) = split_envelope(recv_envelope(&mut rx).await);
    assert_eq!(
        stream_id(&exchange),
        "7-7",
        "the entry claimed after the empty continuation must be delivered"
    );
    reply_tx
        .send(Ok(exchange))
        .expect("pipeline must accept the reply");

    server.wait_for_command_on_conn(1, "XAUTOCLAIM", 2).await;
    let claims = server.args_of_on_conn(1, "XAUTOCLAIM");
    assert_eq!(claims.len(), 2, "the empty non-zero cursor must continue");
    assert_eq!(
        args_as_strings(&claims[0]),
        vec!["s1", "g", "c", "30000", "0-0", "COUNT", "5"],
        "the first claim starts at the pass cursor"
    );
    assert_eq!(
        args_as_strings(&claims[1]),
        vec!["s1", "g", "c", "30000", "5-0", "COUNT", "5"],
        "the empty reply's non-zero cursor must drive the continuation"
    );

    cancel.cancel();
    let joined = join_consumer(&mut consumer).await;
    assert!(
        joined.is_ok(),
        "cancelled stream session must end Ok: {joined:?}"
    );
}

#[tokio::test]
async fn fake_server_nil_entries_between_claims_are_skipped() {
    // Redis 7.0+ reports entries deleted while pending (trimmed/XDEL'ed) as
    // NIL items inside the XAUTOCLAIM entries array. They must be skipped
    // without failing the recovery pass.
    let mut server = StreamFakeServer::start(vec![
        summary_reply(2),
        claim_reply_action(
            "0-0",
            &[
                claim_entry("1-1", &[("payload", "hello")]),
                resp_nil(),
                claim_entry("2-2", &[("payload", "hello")]),
            ],
        ),
        targeted_delivery_count("1-1", 1),
        targeted_delivery_count("2-2", 1),
        park_read_connection(),
    ])
    .await
    .expect("fake server must bind");

    let rt = RecordingRuntimeObservability::new(false);
    let (mut consumer, mut rx, cancel) =
        start_stream_consumer(&server, &recovery_extra(30_000, 5), Arc::clone(&rt)).await;

    for expected in ["1-1", "2-2"] {
        let (exchange, reply_tx) = split_envelope(recv_envelope(&mut rx).await);
        assert_eq!(
            stream_id(&exchange),
            expected,
            "only the real entries around the nil must be delivered"
        );
        reply_tx
            .send(Ok(exchange))
            .expect("pipeline must accept the reply");
    }

    server.wait_for_command_on_conn(1, "XACK", 2).await;
    let acks = server.args_of_on_conn(1, "XACK");
    assert_eq!(
        acks.len(),
        2,
        "exactly the two real claimed entries must be acknowledged: {acks:?}"
    );
    assert_eq!(args_as_strings(&acks[0]), vec!["s1", "g", "1-1"]);
    assert_eq!(args_as_strings(&acks[1]), vec!["s1", "g", "2-2"]);

    cancel.cancel();
    let joined = join_consumer(&mut consumer).await;
    assert!(
        joined.is_ok(),
        "nil entries must not fail supervision: {joined:?}"
    );
}

#[tokio::test]
async fn fake_server_empty_claim_does_not_loop() {
    let mut server = StreamFakeServer::start(vec![
        summary_reply(1),
        claim_reply_action("0-0", &[]),
        ScriptedAction::ReplyOn {
            connection: 1,
            matches_command: "XPENDING",
            reply: resp_nil(),
        },
        park_read_connection(),
    ])
    .await
    .expect("fake server must bind");

    let rt = RecordingRuntimeObservability::new(false);
    let (mut consumer, _rx, cancel) =
        start_stream_consumer(&server, &recovery_extra(50, 5), Arc::clone(&rt)).await;

    // A drained claim (cursor 0-0, no entries) ends the pass. The next pass's
    // opening summary proves the first pass did not loop on its empty reply.
    server.wait_for_command_on_conn(1, "XPENDING", 2).await;
    assert_eq!(
        server.count_on_conn(1, "XAUTOCLAIM"),
        1,
        "an empty 0-0 claim must not issue a second XAUTOCLAIM"
    );

    cancel.cancel();
    let joined = join_consumer(&mut consumer).await;
    assert!(
        joined.is_ok(),
        "cancelled stream session must end Ok: {joined:?}"
    );
}

#[tokio::test]
async fn fake_server_cursor_resume_on_wire() {
    let entries: Vec<Vec<u8>> = (1..=5)
        .map(|index| claim_entry(&format!("{index}-{index}"), &[("payload", "hello")]))
        .collect();
    let mut actions = vec![summary_reply(5), claim_reply_action("5-0", &entries)];
    for index in 1..=5 {
        actions.push(targeted_delivery_count(&format!("{index}-{index}"), 1));
    }
    actions.push(summary_reply(5));
    actions.push(claim_reply_action("0-0", &[]));
    actions.push(park_read_connection());
    let mut server = StreamFakeServer::start(actions)
        .await
        .expect("fake server must bind");

    let rt = RecordingRuntimeObservability::new(false);
    let (mut consumer, mut rx, cancel) =
        start_stream_consumer(&server, &recovery_extra(50, 5), Arc::clone(&rt)).await;

    // The first pass exhausts its 5-entry budget and ends on cursor 5-0.
    for _ in 0..5 {
        let (exchange, reply_tx) = split_envelope(recv_envelope(&mut rx).await);
        reply_tx
            .send(Ok(exchange))
            .expect("pipeline must accept the reply");
    }

    // Summary 1 plus five targeted reads, then the second pass's summary.
    server.wait_for_command_on_conn(1, "XPENDING", 7).await;
    server.wait_for_command_on_conn(1, "XAUTOCLAIM", 2).await;
    let claims = server.args_of_on_conn(1, "XAUTOCLAIM");
    assert_eq!(
        args_as_strings(&claims[1]),
        vec!["s1", "g", "c", "50", "5-0", "COUNT", "5"],
        "the next pass must resume from the cursor the previous pass persisted"
    );

    cancel.cancel();
    let joined = join_consumer(&mut consumer).await;
    assert!(
        joined.is_ok(),
        "cancelled stream session must end Ok: {joined:?}"
    );
}

#[tokio::test]
async fn fake_server_budget_remaining_on_wire() {
    let entries: Vec<Vec<u8>> = (1..=4)
        .map(|index| claim_entry(&format!("{index}-{index}"), &[("payload", "hello")]))
        .collect();
    let mut server = StreamFakeServer::start(vec![
        summary_reply(4),
        claim_reply_action("3-0", &entries),
        claim_reply_action("0-0", &[]),
        park_read_connection(),
    ])
    .await
    .expect("fake server must bind");

    let rt = RecordingRuntimeObservability::new(false);
    let (mut consumer, _rx, cancel) =
        start_stream_consumer(&server, &recovery_extra(50, 5), Arc::clone(&rt)).await;

    server.wait_for_command_on_conn(1, "XAUTOCLAIM", 2).await;
    let claims = server.args_of_on_conn(1, "XAUTOCLAIM");
    assert_eq!(
        args_as_strings(&claims[1]),
        vec!["s1", "g", "c", "50", "3-0", "COUNT", "1"],
        "the continuation must carry the remaining budget and the last cursor"
    );

    cancel.cancel();
    let joined = join_consumer(&mut consumer).await;
    assert!(
        joined.is_ok(),
        "cancelled stream session must end Ok: {joined:?}"
    );
}

#[tokio::test]
async fn fake_server_failed_claimed_delivery_no_ack_retry_later() {
    let mut server = StreamFakeServer::start(vec![
        summary_reply(1),
        claim_reply_action("0-0", &[claim_entry("1-1", &[("payload", "hello")])]),
        targeted_delivery_count("1-1", 1),
        summary_reply(1),
        claim_reply_action("0-0", &[claim_entry("1-1", &[("payload", "hello")])]),
        targeted_delivery_count("1-1", 2),
        park_read_connection(),
    ])
    .await
    .expect("fake server must bind");

    let rt = RecordingRuntimeObservability::new(false);
    let (mut consumer, mut rx, cancel) =
        start_stream_consumer(&server, &recovery_extra(50, 5), Arc::clone(&rt)).await;

    let (exchange, reply_tx) = split_envelope(recv_envelope(&mut rx).await);
    assert_eq!(stream_id(&exchange), "1-1");
    reply_tx
        .send(Err(CamelError::InvalidUri("pipeline failed".to_string())))
        .expect("pipeline must accept the reply");

    // The next pass's opening summary proves the failed delivery completed.
    server.wait_for_command_on_conn(1, "XPENDING", 2).await;
    assert!(
        server.args_of("XACK").is_empty(),
        "a failed claimed delivery must never be acknowledged: {:?}",
        server.args_of("XACK")
    );

    server.wait_for_command_on_conn(1, "XAUTOCLAIM", 2).await;
    let (reclaimed, reply_tx) = split_envelope(recv_envelope(&mut rx).await);
    assert_eq!(
        stream_id(&reclaimed),
        "1-1",
        "the unacknowledged entry must be claimed again on a later pass"
    );
    assert_eq!(
        reclaimed.input.header("CamelRedis.StreamDeliveryCount"),
        Some(&serde_json::json!("2")),
        "the re-claim must carry the updated delivery count"
    );
    reply_tx
        .send(Err(CamelError::InvalidUri("pipeline failed".to_string())))
        .expect("pipeline must accept the reply");

    cancel.cancel();
    let joined = join_consumer(&mut consumer).await;
    assert!(
        joined.is_ok(),
        "cancelled stream session must end Ok: {joined:?}"
    );
    assert!(
        server.args_of("XACK").is_empty(),
        "no claimed entry may be acknowledged after a failed delivery"
    );
}

#[tokio::test]
async fn fake_server_recovery_channel_closed_records_metric() {
    let server = StreamFakeServer::start(vec![
        summary_reply(1),
        claim_reply_action("0-0", &[claim_entry("1-1", &[("payload", "hello")])]),
        targeted_delivery_count("1-1", 1),
    ])
    .await
    .expect("fake server must bind");

    let config = stream_config(server.port(), &recovery_extra(30_000, 5));
    let rt = RecordingRuntimeObservability::new(false);
    let runtime: Arc<dyn RuntimeObservability> = rt.clone();
    let mut consumer = RedisConsumer::new(config, runtime).expect("consumer must construct");
    let (tx, rx) = mpsc::channel::<ExchangeEnvelope>(16);
    // No pipeline at all: the recovered entry cannot be handed off.
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
        "a closed recovery delivery channel must fail supervision: {joined:?}"
    );
    assert!(
        server.args_of("XACK").is_empty(),
        "a channel-gone claimed entry must never be acknowledged"
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
async fn fake_server_nil_summary_issues_no_xautoclaim() {
    let mut server = StreamFakeServer::start(vec![
        ScriptedAction::ReplyOn {
            connection: 1,
            matches_command: "XPENDING",
            reply: resp_nil(),
        },
        ScriptedAction::ReplyOn {
            connection: 1,
            matches_command: "XPENDING",
            reply: resp_nil(),
        },
        ScriptedAction::ReplyOn {
            connection: 0,
            matches_command: "XREADGROUP",
            reply: resp_nil(),
        },
        ScriptedAction::WithholdOn { connection: 0 },
    ])
    .await
    .expect("fake server must bind");

    let rt = RecordingRuntimeObservability::new(false);
    let (mut consumer, _rx, cancel) =
        start_stream_consumer(&server, &recovery_extra(50, 5), Arc::clone(&rt)).await;

    // The second pass's opening summary is the pass boundary: it proves the
    // first nil-summary pass completed, so any read frame observed later
    // cannot predate it.
    server.wait_for_command_on_conn(1, "XPENDING", 2).await;
    // Liveness across that boundary: the read half keeps reading after the
    // nil-summary pass instead of stalling.
    server.wait_for_command_on_conn(0, "XREADGROUP", 2).await;
    assert_eq!(
        server.count_on_conn(1, "XAUTOCLAIM"),
        0,
        "a nil pending summary must never issue XAUTOCLAIM"
    );

    cancel.cancel();
    let joined = join_consumer(&mut consumer).await;
    assert!(
        joined.is_ok(),
        "cancelled stream session must end Ok: {joined:?}"
    );
}

#[tokio::test]
async fn fake_server_shutdown_during_recovery_no_ack_after_cancel() {
    let release = Arc::new(Notify::new());
    let mut server = StreamFakeServer::start(vec![
        summary_reply(1),
        claim_reply_action("0-0", &[claim_entry("1-1", &[("payload", "hello")])]),
        ScriptedAction::DeferredOn {
            connection: 1,
            matches_command: "XPENDING",
            reply: xpending_targeted("1-1", 1),
            release: Arc::clone(&release),
        },
    ])
    .await
    .expect("fake server must bind");

    let rt = RecordingRuntimeObservability::new(false);
    let (mut consumer, _rx, cancel) =
        start_stream_consumer(&server, &recovery_extra(30_000, 5), Arc::clone(&rt)).await;

    // The targeted XPENDING is on the wire but unanswered, so delivery cannot
    // start. Cancel first, then release the withheld reply.
    server.wait_for_command_on_conn(1, "XPENDING", 2).await;
    cancel.cancel();
    release.notify_one();

    let joined = join_consumer(&mut consumer).await;
    assert!(
        joined.is_ok(),
        "cancelled recovery session must end Ok: {joined:?}"
    );
    assert!(
        server.args_of("XACK").is_empty(),
        "a cancelled recovery delivery must never be acknowledged"
    );
}

#[tokio::test]
async fn fake_server_cancel_closes_read_connection() {
    let mut server = StreamFakeServer::start(vec![ScriptedAction::Withhold])
        .await
        .expect("fake server must bind");

    let rt = RecordingRuntimeObservability::new(false);
    let (mut consumer, _rx, cancel) =
        start_stream_consumer(&server, "&block=0", Arc::clone(&rt)).await;

    server.wait_for_command_on_conn(0, "XREADGROUP", 1).await;
    cancel.cancel();

    server.wait_for_connection_close(0).await;
    let joined = join_consumer(&mut consumer).await;
    assert!(
        joined.is_ok(),
        "cancelled blocked stream session must end Ok: {joined:?}"
    );
    assert_eq!(
        server.connection_count(),
        1,
        "recovery disabled must keep a single connection"
    );
}

#[tokio::test]
async fn fake_server_recovery_disabled_no_frames() {
    let mut server = StreamFakeServer::start(vec![ScriptedAction::Withhold])
        .await
        .expect("fake server must bind");

    let rt = RecordingRuntimeObservability::new(false);
    let (mut consumer, _rx, cancel) =
        start_stream_consumer(&server, "&block=0", Arc::clone(&rt)).await;

    // The first XREADGROUP proves the session is running.
    server.wait_for_command_on_conn(0, "XREADGROUP", 1).await;
    assert!(
        server.args_of("XPENDING").is_empty(),
        "recovery disabled must issue no XPENDING: {:?}",
        server.args_of("XPENDING")
    );
    assert!(
        server.args_of("XAUTOCLAIM").is_empty(),
        "recovery disabled must issue no XAUTOCLAIM: {:?}",
        server.args_of("XAUTOCLAIM")
    );
    assert_eq!(
        server.connection_count(),
        1,
        "recovery disabled must not open a recovery connection"
    );

    cancel.cancel();
    let joined = join_consumer(&mut consumer).await;
    assert!(
        joined.is_ok(),
        "cancelled stream session must end Ok: {joined:?}"
    );
}
