//! Task 5.4 drill: reservation threshold and redelivery disposition recording.
//!
//! Included as a focused submodule of `jms_test`, so the prescribed
//! `cargo test -p camel-test --features integration-tests --test jms_test <name>`
//! commands select every case by name.
//!
//! The scenario is the spec's "Reservation threshold and reject-then-log poison
//! policy": a route that always errors rolls the delivery back, the broker
//! redelivers, and on the receipt whose recorded count has reached
//! `JMS_MAX_DELIVERY_ATTEMPTS` (default 6) the bridge applies the terminal
//! policy — acknowledge once and log the provider `JMSMessageID` and count only,
//! with no payload. The durable journal frames are the exact evidence (one
//! `RESERVE` per handoff, one `COMMIT` for the terminal acknowledgement), never
//! the broker's `JMSXDeliveryCount`.
//!
//! The native bridge logs its console to **stdout** by default (verified
//! against the 0.7.1 binary), so the terminal line is captured by moving the
//! Quarkus console handler to stderr (`QUARKUS_LOG_CONSOLE_STDERR=true`) and
//! letting `camel-bridge` redirect that stderr to a test-owned file
//! (`CAMEL_BRIDGE_LOG_STDERR`). Both variables travel on the probe child's
//! environment, so the parent test process never mutates its own environment.
//!
//! ## Disposition evidence boundaries (bd rc-5c6yv)
//!
//! Classic exposes numeric queue `QueueSize` and `ActiveMQ.DLQ`
//! `EnqueueCount`; Artemis exposes numeric source `MESSAGE COUNT` and `DLQ` /
//! `ExpiryQueue` `MESSAGES ADDED` through its own container CLI query
//! (`ArtemisBroker::queue_stat`: docker exec running the pinned container's
//! `artemis queue stat`). The Artemis case reads those counts through the
//! fixture query and asserts the destination was acknowledged and removed by
//! the bridge terminal policy, or records native evidence via
//! `classify_precedence`. A missing bridge `COMMIT` with Artemis evidence is
//! classified exactly like Classic: native-first is reported only with stable
//! numeric broker evidence (Classic source `QueueSize` 0 **and** a non-zero
//! `ActiveMQ.DLQ` `EnqueueCount`, or Artemis source `MESSAGE COUNT` 0 with a
//! non-zero `DLQ` or `ExpiryQueue` `MESSAGES ADDED`); otherwise the disposition
//! is an explicit `UNKNOWN` gap. The load-bearing rule: removing the Artemis
//! query, making it error, or having it report no evidence FAILS the case —
//! the Artemis destination is never treated as unobserved.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use camel_bridge::process::BrokerType;
use camel_component_jms::proto::bridge_service_client::BridgeServiceClient;

use super::support::activemq::{ACTIVEMQ_PASSWORD, ACTIVEMQ_USER, ActiveMqBroker};
use super::support::artemis::{ARTEMIS_PASSWORD, ARTEMIS_USER, ArtemisBroker, QueueStat};
use super::support::jms_bridge_procs::{require_jms_bridge_binary, spawn_jms_bridge_configured};
use super::support::jms_crash_probe::{ProbeBroker, spawn_probe_configured_with_logs};
use super::support::jms_journal::{
    TYPE_COMMIT, TYPE_RESERVE, TYPE_ROLLBACK, commit_observed, count_kind, find_journal_file,
    journal_broker_dir, read_frames,
};
use super::support::wait::wait_until;
use super::{await_broker_connected, init_tracing, send_one, support::install_crypto_provider};

/// The bridge's default terminal cap (`JMS_MAX_DELIVERY_ATTEMPTS`). The task
/// pins the default 6; no broker-limit override is assumed.
const DEFAULT_MAX_DELIVERY_ATTEMPTS: usize = 6;

/// A distinctive body; the test asserts it never appears in the native bridge's
/// terminal log (the terminal line carries the message id and count only).
const THRESHOLD_BODY: &str = "threshold-payload-sentinel-7f3a";

/// The prefix of the Java terminal discard line (`JmsConsumer.terminalDiscardLogLine`).
const TERMINAL_LOG_PREFIX: &str = "Poison message discarded after";

/// The logger that must emit the terminal discard record
/// (`org.rustcamel.jms.JmsConsumer`).
const TERMINAL_LOGGER: &str = "org.rustcamel.jms.JmsConsumer";

/// The only accepted level for the terminal discard record.
const ERROR_LEVEL: &str = "ERROR";

/// The dedicated broker a threshold proof runs against. Classic and Artemis
/// share one contract; only the broker type, credentials, readiness barrier and
/// broker-side observability differ.
enum DispositionBroker {
    Classic(Arc<ActiveMqBroker>),
    Artemis(Arc<ArtemisBroker>),
}

impl DispositionBroker {
    fn broker_url(&self) -> &str {
        match self {
            DispositionBroker::Classic(broker) => &broker.broker_url,
            DispositionBroker::Artemis(broker) => &broker.broker_url,
        }
    }

    fn broker_type(&self) -> BrokerType {
        match self {
            DispositionBroker::Classic(_) => BrokerType::ActiveMq,
            DispositionBroker::Artemis(_) => BrokerType::Artemis,
        }
    }

    fn broker_type_env(&self) -> &'static str {
        match self {
            DispositionBroker::Classic(_) => "activemq",
            DispositionBroker::Artemis(_) => "artemis",
        }
    }

    fn credentials(&self) -> (&'static str, &'static str) {
        match self {
            DispositionBroker::Classic(_) => (ACTIVEMQ_USER, ACTIVEMQ_PASSWORD),
            DispositionBroker::Artemis(_) => (ARTEMIS_USER, ARTEMIS_PASSWORD),
        }
    }

    /// Broker-side readiness barrier before the send. Classic proves the
    /// consumer is registered through its Jolokia console; the Artemis fixture
    /// exposes no equivalent console, so the durable queue retains the send and
    /// the journal/terminal observation is the barrier.
    async fn wait_consumer_registered(&self, queue: &str) {
        let DispositionBroker::Classic(broker) = self else {
            return;
        };
        let broker = broker.clone();
        let queue = queue.to_string();
        wait_until(
            "threshold consumer registered on the queue",
            Duration::from_secs(60),
            Duration::from_millis(200),
            move || {
                let broker = broker.clone();
                let queue = queue.clone();
                async move { broker.consumer_count(&queue).await.map(|count| count >= 1) }
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    }

    /// Reads the observable broker-side destination state after the run.
    ///
    /// Classic exposes numeric queue `QueueSize` and `ActiveMQ.DLQ`
    /// `EnqueueCount` through Jolokia; both numbers are retained for exact
    /// assertions, never reduced to a generated substring. Artemis is read
    /// through its own container CLI (`artemis queue stat`): the source
    /// `MESSAGE COUNT` plus `DLQ` and `ExpiryQueue` `MESSAGES ADDED`, which
    /// distinguish a bridge acknowledgement from the broker-native dead-letter
    /// and expiry paths. A failed Classic read panics (the fixture guarantees
    /// the console query); a failed Artemis query is reported as `None`
    /// evidence with an error note naming the mechanism, never inferred from
    /// client-side deliveries.
    async fn observe_destination(
        &self,
        queue: &str,
    ) -> (
        Option<ClassicDestination>,
        Option<ArtemisDestination>,
        String,
    ) {
        match self {
            DispositionBroker::Classic(broker) => {
                // The bridge acknowledges asynchronously: the COMMIT frame and
                // the terminal log precede the broker applying the ack, so a
                // one-shot read right after them can still see the message
                // (seen on loaded CI). Wait, bounded, for the queue to drain;
                // a real lost ack stays at 1 and is reported by the final
                // read below.
                let _ = wait_until(
                    "source queue drained by the terminal acknowledgement",
                    Duration::from_secs(30),
                    Duration::from_millis(200),
                    || {
                        let broker = broker.clone();
                        async move { broker.queue_size(queue).await.map(|n| n == 0) }
                    },
                )
                .await;
                let queue_size = broker
                    .queue_size(queue)
                    .await
                    .unwrap_or_else(|e| panic!("queue_size read failed: {e}"));
                let dlq_enqueue = broker
                    .enqueue_count("ActiveMQ.DLQ")
                    .await
                    .unwrap_or_else(|e| panic!("ActiveMQ.DLQ enqueue read failed: {e}"));
                let destination = ClassicDestination {
                    queue_size,
                    dlq_enqueue,
                };
                let note = destination.describe();
                (Some(destination), None, note)
            }
            DispositionBroker::Artemis(broker) => {
                // Same asynchronous-ack window as Classic: the bridge-side
                // COMMIT precedes the broker applying the ack. Poll, bounded,
                // until the source queue is empty; a poll error is retried by
                // the next iteration and a timeout is left to the final read,
                // which reports the actual counts.
                let _ = wait_until(
                    "Artemis source queue drained by the terminal acknowledgement",
                    Duration::from_secs(30),
                    Duration::from_millis(200),
                    || {
                        let broker = broker.clone();
                        async move {
                            broker
                                .queue_stat(queue)
                                .await
                                .map(|stat| stat.message_count == 0)
                        }
                    },
                )
                .await;
                match read_artemis_destination(broker, queue).await {
                    Ok(destination) => {
                        let note = destination.describe();
                        (None, Some(destination), note)
                    }
                    Err(error) => (
                        None,
                        None,
                        format!("broker-side queue-stat query failed: {error}"),
                    ),
                }
            }
        }
    }
}

/// Reads the three queues that make up the Artemis destination evidence: the
/// source queue, `DLQ` and `ExpiryQueue`.
async fn read_artemis_destination(
    broker: &ArtemisBroker,
    queue: &str,
) -> Result<ArtemisDestination, String> {
    let source = broker.queue_stat(queue).await?;
    let dlq = broker.queue_stat("DLQ").await?;
    let expiry = broker.queue_stat("ExpiryQueue").await?;
    Ok(ArtemisDestination {
        source,
        dlq,
        expiry,
    })
}

/// Numeric Classic broker-side destination evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ClassicDestination {
    /// Source queue `QueueSize` after the run.
    queue_size: u64,
    /// `ActiveMQ.DLQ` `EnqueueCount` after the run.
    dlq_enqueue: u64,
}

impl ClassicDestination {
    /// True when the numbers prove the terminal acknowledgement removed the
    /// message and no dead-letter delivery happened.
    fn acknowledged_and_removed(&self) -> bool {
        self.queue_size == 0 && self.dlq_enqueue == 0
    }

    /// True when the numbers prove the provider dead-lettered the message.
    fn native_dead_lettered(&self) -> bool {
        self.queue_size == 0 && self.dlq_enqueue > 0
    }

    fn describe(&self) -> String {
        format!(
            "source QueueSize={} ActiveMQ.DLQ EnqueueCount={} ({})",
            self.queue_size,
            self.dlq_enqueue,
            if self.acknowledged_and_removed() {
                "acknowledged and removed by the bridge terminal policy; no DLQ delivery"
            } else if self.native_dead_lettered() {
                "broker-native dead-letter path"
            } else {
                "inconclusive"
            }
        )
    }
}

/// Numeric Artemis broker-side destination evidence.
///
/// All three queues are checked because the fixture routes every address to
/// `DLQ` (`dead-letter-address=DLQ`, `match="#"`), so there is no silent-drop
/// route and the terminal kind is distinguishable: a bridge acknowledgement
/// drains the source with no DLQ/expiry add, native dead-lettering adds to
/// `DLQ`, and native expiry adds to `ExpiryQueue`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ArtemisDestination {
    /// Source queue `MESSAGE COUNT` after the run.
    source: QueueStat,
    /// `DLQ` counts after the run.
    dlq: QueueStat,
    /// `ExpiryQueue` counts after the run.
    expiry: QueueStat,
}

impl ArtemisDestination {
    /// True when the numbers prove the terminal acknowledgement removed the
    /// message and neither native path delivered it.
    fn acknowledged_and_removed(&self) -> bool {
        self.source.message_count == 0
            && self.dlq.messages_added == 0
            && self.expiry.messages_added == 0
    }

    /// True when the numbers prove the provider dead-lettered the message.
    fn native_dead_lettered(&self) -> bool {
        self.source.message_count == 0 && self.dlq.messages_added > 0
    }

    /// True when the numbers prove the provider expired the message.
    fn native_expired(&self) -> bool {
        self.source.message_count == 0
            && self.dlq.messages_added == 0
            && self.expiry.messages_added > 0
    }

    fn describe(&self) -> String {
        format!(
            "source MESSAGE_COUNT={} DLQ MESSAGES_ADDED={} ExpiryQueue MESSAGES_ADDED={} ({})",
            self.source.message_count,
            self.dlq.messages_added,
            self.expiry.messages_added,
            if self.acknowledged_and_removed() {
                "acknowledged and removed by the bridge terminal policy; no DLQ or expiry delivery"
            } else if self.native_dead_lettered() {
                "broker-native dead-letter path (DLQ)"
            } else if self.native_expired() {
                "broker-native expiry path (ExpiryQueue)"
            } else {
                "inconclusive: message still on the source queue"
            }
        )
    }
}

/// One parsed native bridge console record (Quarkus JSON console layout).
#[derive(Debug, Clone, PartialEq, Eq)]
struct BridgeLogRecord {
    level: String,
    logger_name: String,
    message: String,
}

/// Counts terminal discard records that satisfy the FULL expected shape, and
/// terminal-prefixed records that do not.
///
/// `matching` requires the exact message (id + count), `level == ERROR`, and
/// `loggerName == org.rustcamel.jms.JmsConsumer`. `mislabeled` counts any
/// record whose message carries the terminal prefix but fails level or logger.
/// A text-only substring match (the previous implementation) would accept a
/// `WARN` record or a differently logged line; parsing the record rejects both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct TerminalLogCheck {
    matching: usize,
    mislabeled: usize,
}

/// Parses every JSON console line, keeping records with a string `message`.
fn parse_bridge_log_records(contents: &str) -> Vec<BridgeLogRecord> {
    contents
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|value| {
            let message = value.get("message")?.as_str()?.to_string();
            Some(BridgeLogRecord {
                level: value
                    .get("level")
                    .and_then(|l| l.as_str())
                    .unwrap_or_default()
                    .to_string(),
                logger_name: value
                    .get("loggerName")
                    .and_then(|l| l.as_str())
                    .unwrap_or_default()
                    .to_string(),
                message,
            })
        })
        .collect()
}

/// Applies [`TerminalLogCheck`] to raw log contents for `expected_message`.
fn check_terminal_records(contents: &str, expected_message: &str) -> TerminalLogCheck {
    let mut check = TerminalLogCheck::default();
    for record in parse_bridge_log_records(contents) {
        if record.message == expected_message
            && record.level == ERROR_LEVEL
            && record.logger_name == TERMINAL_LOGGER
        {
            check.matching += 1;
        } else if record.message.contains(TERMINAL_LOG_PREFIX) {
            check.mislabeled += 1;
        }
    }
    check
}

/// Which terminal fired first, with the evidence it rests on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Precedence {
    /// The bridge cap reached six reservations and its terminal policy ran.
    BridgeCapFirst,
    /// The provider or broker native limit fired first, confirmed by stable
    /// numeric broker evidence.
    NativeLimitFirst { evidence: String },
    /// The disposition cannot be established from stable evidence. Never
    /// inferred from a missing bridge `COMMIT` alone.
    Unknown { reason: String },
}

/// Classifies the observed precedence. A missing bridge `COMMIT` is not
/// sufficient for native-first: Classic needs numeric dead-letter evidence,
/// Artemis needs numeric DLQ or `ExpiryQueue` evidence, and anything else is
/// `Unknown`.
fn classify_precedence(obs: &ThresholdObservation) -> Precedence {
    if obs.commit_frames == 1 && obs.terminal_check.matching == 1 {
        return Precedence::BridgeCapFirst;
    }
    if let Some(destination) = &obs.classic_destination {
        if destination.native_dead_lettered() {
            return Precedence::NativeLimitFirst {
                evidence: format!(
                    "source QueueSize={} (empty) and ActiveMQ.DLQ EnqueueCount={} (dead-lettered)",
                    destination.queue_size, destination.dlq_enqueue
                ),
            };
        }
        return Precedence::Unknown {
            reason: format!(
                "no terminal COMMIT and Classic broker evidence does not confirm the native \
                 dead-letter path: {}",
                destination.describe()
            ),
        };
    }
    if let Some(destination) = &obs.artemis_destination {
        if destination.native_dead_lettered() || destination.native_expired() {
            return Precedence::NativeLimitFirst {
                evidence: destination.describe(),
            };
        }
        return Precedence::Unknown {
            reason: format!(
                "no terminal COMMIT and Artemis broker evidence does not confirm the native \
                 dead-letter path: {}",
                destination.describe()
            ),
        };
    }
    Precedence::Unknown {
        reason: "no terminal COMMIT and the broker-side disposition query returned no evidence"
            .to_string(),
    }
}

/// What the threshold run observed. Every field is read from a durable or
/// broker-side source, never inferred. `Default` exists only so the focused
/// synthetic-data unit cases can build partial observations.
#[derive(Default)]
struct ThresholdObservation {
    known_id: String,
    reserve_frames: usize,
    rollback_frames: usize,
    commit_frames: usize,
    accepted_events: usize,
    terminal_check: TerminalLogCheck,
    bridge_log_path: String,
    payload_in_log: bool,
    classic_destination: Option<ClassicDestination>,
    artemis_destination: Option<ArtemisDestination>,
    destination_note: String,
    /// Bounded-wait failure when the terminal `COMMIT` did not appear. Kept so
    /// the disposition reporter can distinguish a timeout from a confirmed
    /// native-first ordering.
    commit_wait_error: Option<String>,
}

/// Runs the threshold scenario against `broker`: one always-erroring route
/// consumes a known message; the bridge records a reservation per handoff and a
/// terminal `COMMIT` after the cap is reached.
async fn run_threshold(broker: &DispositionBroker, queue: &str) -> ThresholdObservation {
    init_tracing();
    install_crypto_provider();

    let binary = require_jms_bridge_binary();
    let (username, password) = broker.credentials();

    // Test-owned roots: the probe receives XDG_STATE_HOME and the log
    // directory through its own child env, so the parent process never mutates
    // the global environment.
    let xdg_root = tempfile::TempDir::new().expect("threshold XDG_STATE_HOME root");
    let artifact_dir = tempfile::TempDir::new().expect("threshold event dir");
    let accepted_path = artifact_dir.path().join("accepted.log");
    let log_dir = tempfile::TempDir::new().expect("bridge log dir");

    let mut probe = spawn_probe_configured_with_logs(
        &binary,
        &ProbeBroker {
            url: broker.broker_url(),
            broker_type_env: broker.broker_type_env(),
            username,
            password,
        },
        xdg_root.path(),
        queue,
        &accepted_path,
        "threshold",
        Some(log_dir.path()),
    );
    let probe_pid = probe.pgid().expect("probe process-group id");

    broker.wait_consumer_registered(queue).await;

    // Send one known message through a producer bridge process.
    let (producer_proc, producer_channel) = spawn_jms_bridge_configured(
        &binary,
        broker.broker_url(),
        broker.broker_type(),
        username,
        password,
    )
    .await;
    let mut producer = BridgeServiceClient::new(producer_channel);
    await_broker_connected(&mut producer).await;
    let known_id = send_one(&mut producer, queue, THRESHOLD_BODY).await;
    assert!(
        !known_id.is_empty(),
        "the known send must return a JMSMessageID"
    );

    // The durable journal is the exact reservation evidence.
    let broker_dir = journal_broker_dir(
        xdg_root.path(),
        "default",
        &broker.broker_type(),
        broker.broker_url(),
    );
    let journal = wait_for_journal(&broker_dir).await;

    // Wait, bounded, for the terminal acknowledgement (`COMMIT`). A timeout is
    // recorded, not fatal here: the disposition reporter must still see the
    // journal counts, accepted handoffs and broker evidence before deciding.
    let journal_watch = journal.clone();
    let id_watch = known_id.clone();
    let commit_wait_error = wait_until(
        "bridge terminal reject-then-log (COMMIT frame)",
        Duration::from_secs(120),
        Duration::from_millis(500),
        move || {
            let journal = journal_watch.clone();
            let id = id_watch.clone();
            async move {
                let frames = read_frames(&journal)?;
                Ok(commit_observed(&frames, &id))
            }
        },
    )
    .await
    .err();

    let frames = read_frames(&journal).unwrap_or_else(|e| panic!("{e}"));
    let reserve_frames = count_kind(&frames, TYPE_RESERVE, &known_id);
    let rollback_frames = count_kind(&frames, TYPE_ROLLBACK, &known_id);
    let commit_frames = count_kind(&frames, TYPE_COMMIT, &known_id);

    // The route records one accepted-exchange line per handoff; the terminal
    // receipt is never handed off, so exactly the cap number of lines exist.
    let accepted_events = count_accepted_events(&accepted_path);

    // The native bridge terminal log. The Java terminal policy writes the log
    // line BEFORE it clears the reservation (`terminalDiscard`: acknowledge →
    // log → commit), so once the COMMIT frame is visible the record is already
    // on disk. Keep a short bounded wait to absorb any filesystem lag.
    // COMMIT does NOT mean the broker applied the ack: the client acks
    // asynchronously (bd rc-bzqiz), which is why `observe_destination` polls.
    let bridge_log = log_dir.path().join(format!("jms-bridge-{probe_pid}.log"));
    let bridge_log_str = bridge_log.to_string_lossy().into_owned();
    let expected_message = format!(
        "{TERMINAL_LOG_PREFIX} {DEFAULT_MAX_DELIVERY_ATTEMPTS} delivery attempts: {known_id}"
    );
    if commit_wait_error.is_none() {
        let terminal_watch = bridge_log.clone();
        let expected = expected_message.clone();
        let _ = wait_until(
            "native bridge terminal discard record",
            Duration::from_secs(30),
            Duration::from_millis(200),
            move || {
                let path = terminal_watch.clone();
                let expected = expected.clone();
                async move {
                    let contents = std::fs::read_to_string(&path).unwrap_or_default();
                    Ok(check_terminal_records(&contents, &expected).matching >= 1)
                }
            },
        )
        .await;
    }
    let log_contents = std::fs::read_to_string(&bridge_log).unwrap_or_default();
    let terminal_check = check_terminal_records(&log_contents, &expected_message);
    let payload_in_log = log_contents.contains(THRESHOLD_BODY);

    let (classic_destination, artemis_destination, destination_note) =
        broker.observe_destination(queue).await;

    // Cleanup only after every assertion input has been read. The probe holds
    // the journal lock; its bridge dies with the probe's process group.
    probe.kill_and_reap().await;
    drop(producer);
    drop(producer_proc);

    ThresholdObservation {
        known_id,
        reserve_frames,
        rollback_frames,
        commit_frames,
        accepted_events,
        terminal_check,
        bridge_log_path: bridge_log_str,
        payload_in_log,
        classic_destination,
        artemis_destination,
        destination_note,
        commit_wait_error,
    }
}

/// Asserts the exact durable threshold contract shared by all three cases.
fn assert_threshold_contract(obs: &ThresholdObservation) {
    assert_eq!(
        obs.reserve_frames, DEFAULT_MAX_DELIVERY_ATTEMPTS,
        "the durable journal must record exactly {} RESERVE frames for {} (one per handoff)",
        DEFAULT_MAX_DELIVERY_ATTEMPTS, obs.known_id
    );
    assert_eq!(
        obs.rollback_frames, DEFAULT_MAX_DELIVERY_ATTEMPTS,
        "each routed error must record exactly one ROLLBACK keeping the count"
    );
    assert_eq!(
        obs.commit_frames, 1,
        "the terminal policy must acknowledge exactly once (one COMMIT clearing the reservation); \
         journal wait: {:?}",
        obs.commit_wait_error
    );
    assert_eq!(
        obs.accepted_events, DEFAULT_MAX_DELIVERY_ATTEMPTS,
        "exactly the cap number of exchanges must be handed off before the terminal policy"
    );
    assert_eq!(
        obs.terminal_check.matching, 1,
        "exactly one native bridge terminal record must match level={ERROR_LEVEL}, \
         loggerName={TERMINAL_LOGGER}, and the exact message (id + count {})",
        DEFAULT_MAX_DELIVERY_ATTEMPTS
    );
    assert_eq!(
        obs.terminal_check.mislabeled, 0,
        "no terminal-prefixed record may carry a non-ERROR level or another logger"
    );
    assert!(
        !obs.payload_in_log,
        "the terminal log must carry the message id and count only — body bytes leaked into {}",
        obs.bridge_log_path
    );
}

/// One evidence line for the Phase-5 report: broker, exact id, durable frames,
/// accepted handoffs, native terminal record check, and the observed broker
/// destination.
fn log_observation(obs: &ThresholdObservation, label: &str) {
    eprintln!(
        "PROOF 5.4 [{label}]: known_message_id={} reserve_frames={} rollback_frames={} \
         commit_frames={} accepted_events={} terminal_matching={} terminal_mislabeled={} \
         payload_in_log={} commit_wait_error={:?} bridge_log={} destination=[{}]",
        obs.known_id,
        obs.reserve_frames,
        obs.rollback_frames,
        obs.commit_frames,
        obs.accepted_events,
        obs.terminal_check.matching,
        obs.terminal_check.mislabeled,
        obs.payload_in_log,
        obs.commit_wait_error,
        obs.bridge_log_path,
        obs.destination_note,
    );
}

/// Task 5.4 case 1: a route that always errors reaches the six-reservation cap,
/// then the bridge applies the terminal reject-then-log policy — one
/// acknowledgement (one `COMMIT`) and one native `ERROR` record from
/// `org.rustcamel.jms.JmsConsumer` with the message id and count only, never the
/// payload.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn jms_bridge_threshold_reject_then_log_activemq() {
    let broker = DispositionBroker::Classic(Arc::new(ActiveMqBroker::start().await));
    let obs = run_threshold(&broker, "queue.test-threshold-reject-then-log-activemq").await;

    assert_threshold_contract(&obs);

    // Broker-side corroboration (Classic only): the numeric destination proves
    // the terminal acknowledgement drained the source queue and no dead-letter
    // delivery happened. Both numbers are asserted, not a generated label.
    let destination = obs
        .classic_destination
        .as_ref()
        .expect("Classic fixture must expose numeric management evidence");
    assert!(
        destination.acknowledged_and_removed(),
        "the bridge terminal policy must acknowledge and remove the message: \
         source QueueSize={} ActiveMQ.DLQ EnqueueCount={}",
        destination.queue_size,
        destination.dlq_enqueue
    );

    log_observation(&obs, "activemq");
}

/// Task 5.4 case 2: records which terminal fired first and where the message
/// went on ActiveMQ Classic. Native-first is reported only with numeric
/// dead-letter evidence; a missing `COMMIT` alone is an explicit `UNKNOWN` gap.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn jms_bridge_redelivery_disposition_activemq() {
    let broker = DispositionBroker::Classic(Arc::new(ActiveMqBroker::start().await));
    let obs = run_threshold(&broker, "queue.test-redelivery-disposition-activemq").await;
    record_and_report_disposition(&obs, "activemq");
}

/// Task 5.4 case 3: the same disposition recording on Artemis. The broker-side
/// counts are read through the fixture query (`ArtemisBroker::queue_stat`:
/// docker exec + the container's own `artemis queue stat`, pinned image) and
/// the destination must prove the bridge terminal policy acknowledged and
/// removed the message; on a missing bridge `COMMIT`, native evidence is
/// recorded via `classify_precedence`. The case fails when the query is
/// removed, errors, or reports no evidence — an unobserved destination is
/// never a pass.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn jms_bridge_redelivery_disposition_artemis() {
    let broker = DispositionBroker::Artemis(Arc::new(ArtemisBroker::start().await));
    let obs = run_threshold(&broker, "queue.test-redelivery-disposition-artemis").await;
    record_and_report_disposition(&obs, "artemis");
}

/// Records which terminal fired first and where the message went. The bridge
/// cap is the documented terminal where it fires before the provider/broker
/// native limit; a confirmed native-first ordering is reported as a
/// documented-precedence mismatch (never forced to pass); an inconclusive
/// disposition is an explicit `UNKNOWN` gap, never inferred from a missing
/// `COMMIT`.
#[cfg(unix)]
fn record_and_report_disposition(obs: &ThresholdObservation, label: &str) {
    match classify_precedence(obs) {
        Precedence::BridgeCapFirst => {
            // The durable and native contract must hold on this broker.
            assert_threshold_contract(obs);
            log_observation(obs, label);
            if label == "artemis" {
                // The load-bearing rule: a missing broker-side destination is a
                // failure, never an unobserved pass. Removal or error of the
                // query surfaces here as `artemis_destination: None` (the error
                // travels in `destination_note`).
                match obs.artemis_destination.as_ref() {
                    None => panic!(
                        "GAP rc-5c6yv [artemis]: broker-side disposition query returned no \
                         evidence — destination_note: {}, commit_wait_error: {:?}",
                        obs.destination_note, obs.commit_wait_error
                    ),
                    Some(destination) => {
                        assert!(
                            destination.acknowledged_and_removed(),
                            "the Artemis broker-side query must prove the bridge terminal \
                             policy acknowledged and removed the message: {}",
                            destination.describe()
                        );
                        eprintln!(
                            "PROOF rc-5c6yv [artemis]: broker-side disposition observed — {}",
                            destination.describe()
                        );
                    }
                }
            }
        }
        Precedence::NativeLimitFirst { evidence } => panic!(
            "GAP 5.4 [{label}] rc-5c6yv: the provider/broker native limit fired before the bridge \
             cap of {} reservations — {evidence}. This is a documented-precedence mismatch: the \
             docs must name the native limit as taking precedence. Reported, not forced to pass. \
             reserve_frames={} rollback_frames={} accepted_events={} commit_frames={} \
             terminal_matching={} destination=[{}]",
            DEFAULT_MAX_DELIVERY_ATTEMPTS,
            obs.reserve_frames,
            obs.rollback_frames,
            obs.accepted_events,
            obs.commit_frames,
            obs.terminal_check.matching,
            obs.destination_note,
        ),
        Precedence::Unknown { reason } => panic!(
            "GAP 5.4 [{label}] rc-5c6yv: UNKNOWN terminal disposition — no confirmed bridge \
             terminal and no stable broker-side evidence. {reason}. This is NOT read as \
             native-first from a missing COMMIT. commit_frames={} terminal_matching={} \
             reserve_frames={} rollback_frames={} accepted_events={} commit_wait_error={:?} \
             destination=[{}]",
            obs.commit_frames,
            obs.terminal_check.matching,
            obs.reserve_frames,
            obs.rollback_frames,
            obs.accepted_events,
            obs.commit_wait_error,
            obs.destination_note,
        ),
    }
}

/// Counts the accepted-exchange lines the threshold probe appended.
fn count_accepted_events(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
}

/// Waits, bounded, for the bridge to create the destination journal.
async fn wait_for_journal(broker_dir: &Path) -> std::path::PathBuf {
    let dir = broker_dir.to_path_buf();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(path) = find_journal_file(&dir) {
            return path;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "journal file never appeared under {}",
            dir.display()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Focused synthetic-data cases proving the terminal-record assertion is
/// discriminating (it rejects a `WARN` level or another logger, which a
/// text-only match would accept) and that the unknown/native-first diagnostics
/// are reachable. Test-only; no production code changes.
#[cfg(test)]
mod sensitive_assertions {
    use super::*;

    const ID: &str = "ID:synthetic";

    fn expected_message() -> String {
        format!("{TERMINAL_LOG_PREFIX} {DEFAULT_MAX_DELIVERY_ATTEMPTS} delivery attempts: {ID}")
    }

    fn json_record(level: &str, logger: &str, message: &str) -> String {
        serde_json::json!({
            "level": level,
            "loggerName": logger,
            "message": message,
        })
        .to_string()
    }

    fn synthetic(commit_frames: usize, matching: usize) -> ThresholdObservation {
        ThresholdObservation {
            known_id: ID.to_string(),
            commit_frames,
            terminal_check: TerminalLogCheck {
                matching,
                mislabeled: 0,
            },
            ..Default::default()
        }
    }

    /// Sibling builder for Artemis-shaped observations: a missing bridge
    /// `COMMIT` plus explicit broker-side queue counts.
    fn synthetic_artemis(
        source: QueueStat,
        dlq: QueueStat,
        expiry: QueueStat,
    ) -> ThresholdObservation {
        ThresholdObservation {
            commit_frames: 0,
            artemis_destination: Some(ArtemisDestination {
                source,
                dlq,
                expiry,
            }),
            ..Default::default()
        }
    }

    fn stat(message_count: u64, messages_added: u64) -> QueueStat {
        QueueStat {
            message_count,
            messages_added,
        }
    }

    #[test]
    fn exact_error_record_is_accepted() {
        let contents = json_record(ERROR_LEVEL, TERMINAL_LOGGER, &expected_message());
        let check = check_terminal_records(&contents, &expected_message());
        assert_eq!(
            check,
            TerminalLogCheck {
                matching: 1,
                mislabeled: 0
            }
        );
    }

    #[test]
    fn warn_level_terminal_record_is_rejected() {
        let contents = json_record("WARN", TERMINAL_LOGGER, &expected_message());
        let check = check_terminal_records(&contents, &expected_message());
        assert_eq!(
            check,
            TerminalLogCheck {
                matching: 0,
                mislabeled: 1
            },
            "a WARN record carrying the terminal text must not pass"
        );
    }

    #[test]
    fn other_logger_terminal_record_is_rejected() {
        let contents = json_record(ERROR_LEVEL, "org.example.OtherLogger", &expected_message());
        let check = check_terminal_records(&contents, &expected_message());
        assert_eq!(
            check,
            TerminalLogCheck {
                matching: 0,
                mislabeled: 1
            },
            "an ERROR from another logger carrying the terminal text must not pass"
        );
    }

    #[test]
    fn wrong_count_terminal_record_is_rejected() {
        let wrong = format!("{TERMINAL_LOG_PREFIX} 5 delivery attempts: {ID}");
        let contents = json_record(ERROR_LEVEL, TERMINAL_LOGGER, &wrong);
        let check = check_terminal_records(&contents, &expected_message());
        assert_eq!(
            check,
            TerminalLogCheck {
                matching: 0,
                mislabeled: 1
            }
        );
    }

    #[test]
    fn wrong_id_terminal_record_is_rejected() {
        let wrong = format!(
            "{TERMINAL_LOG_PREFIX} {DEFAULT_MAX_DELIVERY_ATTEMPTS} delivery attempts: ID:other"
        );
        let contents = json_record(ERROR_LEVEL, TERMINAL_LOGGER, &wrong);
        let check = check_terminal_records(&contents, &expected_message());
        assert_eq!(
            check,
            TerminalLogCheck {
                matching: 0,
                mislabeled: 1
            }
        );
    }

    #[test]
    fn non_json_terminal_text_is_not_a_record() {
        let check = check_terminal_records(&expected_message(), &expected_message());
        assert_eq!(
            check,
            TerminalLogCheck {
                matching: 0,
                mislabeled: 0
            }
        );
    }

    #[test]
    fn bridge_cap_first_when_commit_and_record_match() {
        assert_eq!(
            classify_precedence(&synthetic(1, 1)),
            Precedence::BridgeCapFirst
        );
    }

    #[test]
    fn missing_commit_without_evidence_is_unknown_not_native_first() {
        // No COMMIT and no broker query (Artemis): must be Unknown, never
        // inferred as native-first.
        assert!(matches!(
            classify_precedence(&synthetic(0, 0)),
            Precedence::Unknown { .. }
        ));
    }

    #[test]
    fn missing_commit_with_classic_dlq_evidence_is_native_first() {
        let obs = ThresholdObservation {
            commit_frames: 0,
            classic_destination: Some(ClassicDestination {
                queue_size: 0,
                dlq_enqueue: 1,
            }),
            ..Default::default()
        };
        assert!(matches!(
            classify_precedence(&obs),
            Precedence::NativeLimitFirst { .. }
        ));
    }

    #[test]
    fn missing_commit_with_classic_inconclusive_evidence_is_unknown() {
        let obs = ThresholdObservation {
            commit_frames: 0,
            classic_destination: Some(ClassicDestination {
                queue_size: 0,
                dlq_enqueue: 0,
            }),
            ..Default::default()
        };
        assert!(
            matches!(classify_precedence(&obs), Precedence::Unknown { .. }),
            "an empty source queue with an empty DLQ does not confirm native-first"
        );
    }

    #[test]
    fn missing_commit_with_artemis_dlq_evidence_is_native_first() {
        let obs = synthetic_artemis(stat(0, 1), stat(0, 1), stat(0, 0));
        assert!(matches!(
            classify_precedence(&obs),
            Precedence::NativeLimitFirst { .. }
        ));
    }

    #[test]
    fn missing_commit_with_artemis_expiry_evidence_is_native_first() {
        let obs = synthetic_artemis(stat(0, 1), stat(0, 0), stat(0, 1));
        assert!(matches!(
            classify_precedence(&obs),
            Precedence::NativeLimitFirst { .. }
        ));
    }

    #[test]
    fn missing_commit_with_artemis_no_evidence_is_unknown() {
        let obs = synthetic_artemis(stat(0, 1), stat(0, 0), stat(0, 0));
        assert!(
            matches!(classify_precedence(&obs), Precedence::Unknown { .. }),
            "an empty source queue with an empty DLQ and ExpiryQueue does not confirm native-first"
        );
    }

    #[test]
    fn artemis_destination_describe_names_each_outcome() {
        let acknowledged = ArtemisDestination {
            source: stat(0, 1),
            dlq: stat(0, 0),
            expiry: stat(0, 0),
        };
        assert!(
            acknowledged.describe().contains(
                "acknowledged and removed by the bridge terminal policy; no DLQ or expiry delivery"
            ),
            "unexpected description: {}",
            acknowledged.describe()
        );

        let dead_lettered = ArtemisDestination {
            source: stat(0, 1),
            dlq: stat(0, 1),
            expiry: stat(0, 0),
        };
        assert!(
            dead_lettered
                .describe()
                .contains("broker-native dead-letter path (DLQ)"),
            "unexpected description: {}",
            dead_lettered.describe()
        );

        let expired = ArtemisDestination {
            source: stat(0, 1),
            dlq: stat(0, 0),
            expiry: stat(0, 1),
        };
        assert!(
            expired
                .describe()
                .contains("broker-native expiry path (ExpiryQueue)"),
            "unexpected description: {}",
            expired.describe()
        );

        let inconclusive = ArtemisDestination {
            source: stat(1, 1),
            dlq: stat(0, 0),
            expiry: stat(0, 0),
        };
        let description = inconclusive.describe();
        assert!(
            description.starts_with(
                "source MESSAGE_COUNT=1 DLQ MESSAGES_ADDED=0 ExpiryQueue MESSAGES_ADDED=0 ("
            ),
            "unexpected description: {description}"
        );
        assert!(
            description.contains("inconclusive: message still on the source queue"),
            "unexpected description: {description}"
        );
    }

    #[test]
    fn artemis_acknowledged_and_removed_requires_all_three_queues() {
        let dead_lettered = ArtemisDestination {
            source: stat(0, 1),
            dlq: stat(0, 1),
            expiry: stat(0, 0),
        };
        assert!(!dead_lettered.acknowledged_and_removed());

        let expired = ArtemisDestination {
            source: stat(0, 1),
            dlq: stat(0, 0),
            expiry: stat(0, 1),
        };
        assert!(!expired.acknowledged_and_removed());
    }

    /// The load-bearing property: a missing Artemis broker-side destination is
    /// not an unobserved pass — the reporter fails the case. The counters are
    /// contract-valid because `assert_threshold_contract` runs before the
    /// destination check, so zeroed counters would panic with the wrong message.
    #[cfg(unix)]
    #[test]
    #[should_panic(expected = "returned no evidence")]
    fn artemis_missing_destination_fails_the_case() {
        let obs = ThresholdObservation {
            known_id: ID.to_string(),
            reserve_frames: DEFAULT_MAX_DELIVERY_ATTEMPTS,
            rollback_frames: DEFAULT_MAX_DELIVERY_ATTEMPTS,
            commit_frames: 1,
            accepted_events: DEFAULT_MAX_DELIVERY_ATTEMPTS,
            terminal_check: TerminalLogCheck {
                matching: 1,
                mislabeled: 0,
            },
            payload_in_log: false,
            artemis_destination: None,
            ..Default::default()
        };
        record_and_report_disposition(&obs, "artemis");
    }
}
