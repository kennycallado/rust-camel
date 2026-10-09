//! Shared RabbitMQ docker fixture for the component's integration tests.
//!
//! One `rabbitmq:3.13-alpine` container per `RabbitFixture`, addressed by an
//! ephemeral host port and torn down with `docker rm -f` on `Drop`. Helpers
//! accrue across phases (consumer tests add `start_on_port`/`restart`/…), so
//! the whole module allows dead code by design rather than churning
//! `#[allow]`s on every landed helper (task 1.6).
#![allow(dead_code)]

use std::io::Read;
use std::net::TcpListener;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use lapin::Channel;
use lapin::options::QueueDeclareOptions;
use lapin::types::{FieldTable, ShortString};
use tracing_subscriber::layer::SubscriberExt;

/// Broker image the fixture runs.
const IMAGE: &str = "rabbitmq:3.13-alpine";
/// Known fixture credentials. The official image's default `guest` user only
/// authenticates from the container loopback; a host connection arrives from
/// the docker bridge gateway and is refused, so the fixture creates a named
/// default user/password instead.
const FIXTURE_USER: &str = "rmq";
const FIXTURE_PASS: &str = "rmq";

/// Bounded wall-clock budget for one synchronous docker invocation.
const DOCKER_CMD_TIMEOUT: Duration = Duration::from_secs(30);
/// Readiness poll budget: real AMQP handshake, never a magic sleep.
const READY_TIMEOUT: Duration = Duration::from_secs(60);
/// Poll interval between readiness attempts.
const READY_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Binary activation gate: unset `RABBITMQ_ITEST` skips, an activated gate
/// without docker is an infrastructure failure (never a silent skip).
pub enum Gate {
    /// Broker tests do not run; the string is the printed notice.
    Skip(String),
    /// Broker tests run.
    Run,
}

/// Pure gate decision (no I/O): `itest` unset skips carrying the notice;
/// `itest` set probes docker and either runs or panics `infra-unavailable`.
pub fn gate(itest: Option<&str>, docker_ok: impl Fn() -> bool) -> Gate {
    match itest {
        None => Gate::Skip(
            "RabbitMQ integration tests skipped: RABBITMQ_ITEST is not set; \
             run with RABBITMQ_ITEST=1"
                .to_string(),
        ),
        Some(_) => {
            if docker_ok() {
                Gate::Run
            } else {
                panic!("infra-unavailable: rabbitmq tier requires docker (RABBITMQ_ITEST=1)");
            }
        }
    }
}

/// Read `RABBITMQ_ITEST`, apply [`gate`], and build the fixture when active.
///
/// On [`Gate::Skip`] the notice is printed and `None` is returned, so every
/// broker-dependent test can start with
/// `let Some(fx) = require_fixture() else { return; };` and still exit 0.
pub fn require_fixture() -> Option<RabbitFixture> {
    if !gate_active() {
        return None;
    }
    Some(RabbitFixture::start())
}

/// Apply the binary gate without constructing a fixture.
///
/// Returns `true` when the broker tier is active (and docker is reachable),
/// `false` on the skip notice. Panics `infra-unavailable` when the gate is
/// activated but docker is absent (never a silent skip). Used by readiness
/// tests that must reserve a port and start the broker themselves.
pub fn gate_active() -> bool {
    let itest = std::env::var("RABBITMQ_ITEST").ok();
    match gate(itest.as_deref(), docker_available) {
        Gate::Skip(notice) => {
            eprintln!("{notice}");
            false
        }
        Gate::Run => true,
    }
}

/// A running fixture broker.
pub struct RabbitFixture {
    container_id: String,
    /// Concrete minimal connection URL: named credentials over host loopback,
    /// default vhost `%2f` (see `FIXTURE_USER`).
    pub amqp_url: String,
}

impl RabbitFixture {
    /// Reserve a port, start the container, and wait for a real AMQP handshake.
    ///
    /// Image presence is deliberately not pre-checked: `docker run` pulls a
    /// missing image on demand. The activated tier is expected to pre-pull
    /// under its own disk guard (task 1.6 verification pre-step).
    fn start() -> Self {
        Self::start_with_port(reserve_free_port())
    }

    /// Start the container bound to a caller-provided host port.
    ///
    /// The readiness test reserves the port first (so the broker is absent at
    /// a known address), points a consumer at it, then starts the fixture here.
    pub fn start_on_port(port: u16) -> Self {
        Self::start_with_port(port)
    }

    fn start_with_port(port: u16) -> Self {
        let name = format!(
            "rmq-itest-{}-{}-{}",
            std::process::id(),
            nanos(),
            fixture_seq()
        );
        // Bind the host side to loopback so the actual bind matches the
        // loopback URL the fixture hands out (never 0.0.0.0).
        let port_mapping = format!("127.0.0.1:{port}:5672");
        let user_env = format!("RABBITMQ_DEFAULT_USER={FIXTURE_USER}");
        let pass_env = format!("RABBITMQ_DEFAULT_PASS={FIXTURE_PASS}");
        // `name` is unique to this call, so a best-effort `docker rm -f <name>`
        // on any failure path can only ever remove this fixture's own
        // container (the `RabbitFixture`/`Drop` does not exist yet).
        let output = match docker_exec(&[
            "run",
            "-d",
            "--name",
            &name,
            "-p",
            &port_mapping,
            "-e",
            &user_env,
            "-e",
            &pass_env,
            IMAGE,
        ]) {
            Ok(output) => output,
            Err(error) => {
                remove_own_container(&name);
                panic!("infra-unavailable: docker run failed: {error}");
            }
        };
        if !output.status.success() {
            remove_own_container(&name);
            panic!(
                "infra-unavailable: docker run failed: {}",
                output.stderr.trim()
            );
        }
        let container_id = output.stdout.trim().to_string();
        if container_id.is_empty() {
            remove_own_container(&name);
            panic!("infra-unavailable: docker run returned no container id");
        }

        let fixture = Self {
            container_id,
            amqp_url: format!("amqp://{FIXTURE_USER}:{FIXTURE_PASS}@127.0.0.1:{port}/%2f"),
        };
        fixture.wait_ready();
        fixture
    }

    /// Poll a real AMQP connect/channel/close handshake until the broker
    /// answers or [`READY_TIMEOUT`] elapses. Runs on a dedicated thread with
    /// its own runtime so it never blocks the caller's tokio reactor.
    fn wait_ready(&self) {
        let url = self.amqp_url.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("readiness runtime must build");
            let result = runtime.block_on(async move {
                let deadline = Instant::now() + READY_TIMEOUT;
                loop {
                    match probe_amqp(&url).await {
                        Ok(()) => return Ok(()),
                        Err(error) => {
                            if Instant::now() >= deadline {
                                return Err(error);
                            }
                            tokio::time::sleep(READY_POLL_INTERVAL).await;
                        }
                    }
                }
            });
            let _ = tx.send(result);
        });

        match rx.recv_timeout(READY_TIMEOUT + Duration::from_secs(5)) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                panic!("infra-unavailable: rabbitmq fixture not ready: {error}")
            }
            Err(error) => {
                panic!("infra-unavailable: rabbitmq fixture readiness timed out: {error}")
            }
        }
    }

    /// Declare a durable queue with a raw lapin connection.
    ///
    /// Returns the connection alongside the channel: the channel rides the
    /// connection, so the caller keeps both alive for the test body (e.g. for
    /// a later `basic_get`).
    pub async fn declare_queue(&self, name: &str) -> (lapin::Connection, Channel) {
        let connection =
            lapin::Connection::connect(&self.amqp_url, lapin::ConnectionProperties::default())
                .await
                .expect("fixture connection for queue declare");
        let channel = connection
            .create_channel()
            .await
            .expect("fixture channel for queue declare");
        channel
            .queue_declare(
                ShortString::from(name),
                QueueDeclareOptions {
                    durable: true,
                    ..QueueDeclareOptions::default()
                },
                FieldTable::default(),
            )
            .await
            .expect("fixture durable queue declare");
        (connection, channel)
    }

    /// Restart this fixture's broker container in place (`docker restart`).
    ///
    /// The container is stopped and started again, never recreated, so its
    /// writable layer (and the durable queue/messages on it) survives. The
    /// point is to force every client connection to drop so the manager's
    /// reconnect path is exercised. The command is bounded by
    /// [`DOCKER_CMD_TIMEOUT`], and readiness is re-polled with a real AMQP
    /// handshake before returning. No `--rm` is used anywhere in this fixture,
    /// so the container survives to be restarted (task 1.6 / 2.5).
    pub fn restart(&self) {
        let output = match docker_exec(&["restart", &self.container_id]) {
            Ok(output) => output,
            Err(error) => panic!("infra-unavailable: docker restart failed: {error}"),
        };
        if !output.status.success() {
            panic!(
                "infra-unavailable: docker restart failed: {}",
                output.stderr.trim()
            );
        }
        self.wait_ready();
    }

    /// Freeze this fixture's broker container (`docker pause`).
    ///
    /// A paused container keeps the TCP socket ESTABLISHED but processes no
    /// frames, so a publish write still fits the socket buffer while its
    /// confirm never arrives — the deterministic way to exercise the confirm
    /// bound without a fake broker. Bounded by [`DOCKER_CMD_TIMEOUT`]; a
    /// failure is an infrastructure error (never a silent no-op). The pause
    /// is always released: callers call [`unpause`](Self::unpause) in
    /// teardown, and [`Drop`] retries it best-effort even after a panic.
    pub fn pause(&self) {
        let output = match docker_exec(&["pause", &self.container_id]) {
            Ok(output) => output,
            Err(error) => panic!("infra-unavailable: docker pause failed: {error}"),
        };
        if !output.status.success() {
            panic!(
                "infra-unavailable: docker pause failed: {}",
                output.stderr.trim()
            );
        }
    }

    /// Unfreeze this fixture's broker container (`docker unpause`).
    ///
    /// Failure is an infrastructure error: the caller explicitly asked for
    /// it. `Drop` uses its own best-effort variant so cleanup still runs.
    pub fn unpause(&self) {
        let output = match docker_exec(&["unpause", &self.container_id]) {
            Ok(output) => output,
            Err(error) => panic!("infra-unavailable: docker unpause failed: {error}"),
        };
        if !output.status.success() {
            panic!(
                "infra-unavailable: docker unpause failed: {}",
                output.stderr.trim()
            );
        }
    }
}

impl Drop for RabbitFixture {
    fn drop(&mut self) {
        // A paused container can refuse removal until it is unfrozen, and
        // `Drop` runs even when a test panicked while paused (task 3.1).
        // Best-effort: the call is bounded by `DOCKER_CMD_TIMEOUT`, and a
        // failure (never paused, or already gone) is ignored so the removal
        // below still runs.
        let _ = docker_exec(&["unpause", &self.container_id]);
        remove_own_container(&self.container_id);
    }
}

/// Best-effort bounded removal of this fixture's own container by its unique
/// name. Used by [`RabbitFixture::drop`] and by the `start()` failure paths,
/// before a `RabbitFixture` exists. Never sweeps other containers: the name is
/// the one this call generated.
fn remove_own_container(name: &str) {
    match docker_exec(&["rm", "-f", name]) {
        Ok(output) if output.status.success() => {}
        Ok(output) => eprintln!(
            "rabbitmq fixture cleanup failed (container {name}): {}",
            output.stderr.trim()
        ),
        Err(error) => eprintln!("rabbitmq fixture cleanup failed (container {name}): {error}"),
    }
}

/// One connect → create channel → close handshake against `url`.
async fn probe_amqp(url: &str) -> Result<(), String> {
    let connection = lapin::Connection::connect(url, lapin::ConnectionProperties::default())
        .await
        .map_err(|error| error.to_string())?;
    let channel = connection
        .create_channel()
        .await
        .map_err(|error| error.to_string())?;
    channel
        .close(200, ShortString::from("readiness probe"))
        .await
        .map_err(|error| error.to_string())?;
    connection
        .close(200, ShortString::from("readiness probe"))
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Docker daemon reachability. Image presence is not probed: `docker run`
/// pulls a missing image, and the activated tier pre-pulls under its own
/// disk guard before invoking the test binary.
fn docker_available() -> bool {
    matches!(docker_exec(&["info"]), Ok(output) if output.status.success())
}

/// Captured result of a bounded docker invocation.
struct DockerOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

/// Run one docker command with a hard [`DOCKER_CMD_TIMEOUT`]. Returns `Err`
/// on spawn failure or timeout (the child is killed); a non-zero exit is a
/// successful call with a failing status, left to the caller.
fn docker_exec(args: &[&str]) -> Result<DockerOutput, String> {
    let mut child = Command::new("docker")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot spawn docker: {error}"))?;

    let deadline = Instant::now() + DOCKER_CMD_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "docker {args:?} timed out after {DOCKER_CMD_TIMEOUT:?}"
                    ));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(format!("docker wait failed: {error}")),
        }
    };

    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }
    Ok(DockerOutput {
        status,
        stdout,
        stderr,
    })
}

/// Reserve an ephemeral host port by binding and immediately releasing it.
pub fn reserve_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
    let port = listener
        .local_addr()
        .expect("read the ephemeral port")
        .port();
    drop(listener);
    port
}

/// Unique-per-process suffix shared by the fixture name and test resources.
pub(crate) fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default()
}

/// Monotonic per-process counter appended to the fixture name.
///
/// `nanos()` alone can repeat when sibling tests call it within one clock
/// tick, which made two concurrent fixtures pick the same container name and
/// clobber each other (a `docker rm -f <name>` cleanup removes the other
/// test's broker). The counter guarantees distinct names within a process.
fn fixture_seq() -> u64 {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Handle over the process-wide recording tracing sink.
///
/// The reconnect tests need to prove that no AMQP protocol error
/// (`PRECONDITION_FAILED` / `UNKNOWN_DELIVERY_TAG`) is observed across a broker
/// restart: a stale delivery tag applied to the reconnected channel would
/// surface as such an error. The sink records every event's rendered fields
/// (including the structured `error` field the engine logs), so the assertion
/// sees the error text, not just the message.
pub struct LogCapture {
    records: Arc<Mutex<Vec<(tracing::Level, String)>>>,
}

impl LogCapture {
    /// Current number of recorded events — a snapshot boundary.
    pub fn snapshot_len(&self) -> usize {
        self.records.lock().expect("log records poisoned").len()
    }

    /// Number of captured events whose rendered fields contain `needle`.
    pub fn count_containing(&self, needle: &str) -> usize {
        self.records
            .lock()
            .expect("log records poisoned")
            .iter()
            .filter(|(_, fields)| fields.contains(needle))
            .count()
    }

    /// Number of captured events containing any of `needles`.
    pub fn count_containing_any(&self, needles: &[&str]) -> usize {
        self.records
            .lock()
            .expect("log records poisoned")
            .iter()
            .filter(|(_, fields)| needles.iter().any(|needle| fields.contains(needle)))
            .count()
    }

    /// Number of events recorded at or after the `start` snapshot whose fields
    /// contain any of `needles`.
    ///
    /// Scoping to a snapshot window lets a test attribute an exact
    /// generation-pair event to its own scenario even when sibling tests share
    /// the process-wide sink.
    pub fn count_containing_since(&self, start: usize, needles: &[&str]) -> usize {
        self.records
            .lock()
            .expect("log records poisoned")
            .iter()
            .skip(start)
            .filter(|(_, fields)| needles.iter().any(|needle| fields.contains(needle)))
            .count()
    }
}

/// Install (once per process) and return the recording sink.
///
/// A global subscriber is required: the engine logs from spawned tasks, so a
/// thread-local default would miss the events. Every test in a binary shares
/// one sink; each test scopes its assertion to the count delta over its own
/// scenario window.
pub fn log_capture() -> LogCapture {
    LogCapture {
        records: global_log_sink().clone(),
    }
}

type LogSink = Arc<Mutex<Vec<(tracing::Level, String)>>>;

fn global_log_sink() -> &'static LogSink {
    static SINK: OnceLock<LogSink> = OnceLock::new();
    SINK.get_or_init(|| {
        let records: LogSink = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(RecordingLayer {
            records: records.clone(),
        });
        tracing::subscriber::set_global_default(subscriber)
            .expect("global tracing subscriber must install exactly once");
        records
    })
}

/// `tracing_subscriber` layer recording `(level, rendered fields)` pairs.
struct RecordingLayer {
    records: LogSink,
}

impl<S> tracing_subscriber::Layer<S> for RecordingLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = FieldVisitor(String::new());
        event.record(&mut visitor);
        self.records
            .lock()
            .expect("log records poisoned")
            .push((*event.metadata().level(), visitor.0));
    }
}

/// Field visitor concatenating every field as `name=value`, so the structured
/// `error` field (not just `message`) is captured.
struct FieldVisitor(String);

impl tracing::field::Visit for FieldVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.push_str(field.name());
        self.0.push('=');
        self.0.push_str(value);
        self.0.push(' ');
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(field.name());
        self.0.push('=');
        self.0.push_str(&format!("{value:?}"));
        self.0.push(' ');
    }
}
