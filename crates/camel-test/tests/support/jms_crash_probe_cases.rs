//! Task 5.1 crash-probe scenarios, included as a submodule of `jms_test` so the
//! `cargo test ... --test jms_test jms_crash` filter still selects them.
//!
//! The filter matches every `jms_crash_probe_*` case below; run them with the
//! module-qualified path (`jms_crash_cases::jms_crash_probe_writes_marker`) when
//! `--exact` is required.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use camel_bridge::process::BrokerType;
use camel_component_jms::proto::SendRequest;
use camel_component_jms::proto::bridge_service_client::BridgeServiceClient;

use super::support::activemq::ActiveMqBroker;
use super::support::artemis::{ARTEMIS_PASSWORD, ARTEMIS_USER, ArtemisBroker};
use super::support::jms_bridge_procs::{
    require_jms_bridge_binary, spawn_jms_bridge, spawn_jms_bridge_configured,
};
use super::support::jms_crash_probe::{
    ProbeBroker, bridge_handoff_path, live_group_members, read_bridge_handoff, spawn_group_leader,
    spawn_probe, spawn_probe_configured,
};
use super::support::jms_journal::{
    commit_observed, find_journal_file, journal_broker_dir, lock_owner_pid, lock_path, read_frames,
    reservation_count,
};
use super::support::wait::wait_until;
use super::{await_broker_connected, init_tracing, send_one, support::install_crypto_provider};

/// Task 5.1: the probe writes its handoff marker from inside the consumer's
/// first processor, which runs only once an exchange has been accepted. The
/// marker must therefore appear after a delivery — never at startup.
#[tokio::test(flavor = "multi_thread")]
async fn jms_crash_probe_writes_marker() {
    init_tracing();
    install_crypto_provider();

    let broker = std::sync::Arc::new(ActiveMqBroker::start().await);
    let binary = require_jms_bridge_binary();

    let queue = "queue.test-crash-probe-marker";
    // Test-owned roots: the probe receives XDG_STATE_HOME and the marker path
    // through its own argv/env, so the parent test process never mutates the
    // global environment (`set_var` races under parallel test threads).
    let xdg_root = tempfile::TempDir::new().expect("probe XDG_STATE_HOME root");
    let marker_dir = tempfile::TempDir::new().expect("probe marker directory");
    let marker = marker_dir.path().join("handoff.marker");

    let mut probe = spawn_probe(&binary, &broker.broker_url, xdg_root.path(), queue, &marker);

    // Explicit broker-side ready barrier (no arbitrary sleep): wait until the
    // probe's consumer is registered on the queue, then prove the handoff
    // marker does NOT exist yet. The marker is written only from the first
    // processor on an accepted exchange, never at startup/readiness.
    let barrier_broker = broker.clone();
    wait_until(
        "probe consumer registered on the queue",
        Duration::from_secs(60),
        Duration::from_millis(200),
        move || {
            let broker = barrier_broker.clone();
            async move { broker.consumer_count(queue).await.map(|count| count >= 1) }
        },
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        !marker.exists(),
        "probe wrote a handoff marker before any delivery — it must only write \
         from the first processor on an accepted exchange"
    );

    // Deliver one message through the existing producer-process helper. A JMS
    // queue retains it, so the probe may subscribe before or after the send.
    let (producer_proc, producer_channel) = spawn_jms_bridge(&binary, &broker.broker_url).await;
    let mut producer = BridgeServiceClient::new(producer_channel);
    await_broker_connected(&mut producer).await;
    let seed_id = send_one(&mut producer, queue, "crash-probe-seed").await;
    assert!(
        !seed_id.is_empty(),
        "probe seed send must return a JMSMessageID"
    );

    let watch = marker.clone();
    wait_until(
        "jms crash probe handoff marker",
        Duration::from_secs(60),
        Duration::from_millis(200),
        move || {
            let watch = watch.clone();
            async move { Ok(watch.is_file()) }
        },
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));

    let contents = std::fs::read_to_string(&marker).expect("read handoff marker");
    assert_eq!(
        contents, "accepted\n",
        "handoff marker must record an accepted exchange"
    );
    assert!(
        probe.is_alive(),
        "the probe must stay blocked in its first processor until killed"
    );

    // The probe is still blocked inside its first processor. Kill its whole
    // process group so the Java bridge it spawned dies with it — a plain
    // `Child::kill` would orphan that bridge.
    probe.kill_and_reap().await;
    drop(producer);
    drop(producer_proc);
}

/// Task 5.1 (reviewer): a probe missing a required input must fail fast with a
/// non-zero exit and no marker. Needs no broker. `kill_on_drop(true)` ensures a
/// timeout cannot leak the subprocess.
#[tokio::test(flavor = "multi_thread")]
async fn jms_crash_probe_missing_required_input_exits_nonzero() {
    let marker_dir = tempfile::TempDir::new().expect("probe marker directory");
    let marker = marker_dir.path().join("handoff.marker");

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_jms_crash_probe"));
    command
        .arg(&marker)
        .env_remove("CAMEL_JMS_BROKER_URL")
        .env_remove("CAMEL_JMS_BRIDGE_BINARY_PATH")
        .kill_on_drop(true);

    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .expect("probe must exit promptly when a required input is missing")
        .expect("run jms_crash_probe");

    assert!(
        !output.status.success(),
        "missing CAMEL_JMS_BROKER_URL must exit non-zero; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!marker.exists(), "a failed probe must not write a marker");
}

/// Task 5.1 (reviewer): a subscribe-time permanent rejection arrives AFTER
/// `start()` returned Ok. A journal lock held by another live owner of the same
/// domain (same XDG root, broker key, and destination) must make the probe
/// observe the terminal route failure, exit non-zero, and write no marker.
#[tokio::test(flavor = "multi_thread")]
async fn jms_crash_probe_async_subscribe_rejection_exits_nonzero() {
    init_tracing();
    install_crypto_provider();

    let broker = ActiveMqBroker::start().await;
    let binary = require_jms_bridge_binary();
    let queue = "queue.test-crash-probe-lock";

    // One test-owned XDG root for BOTH probes: identical journal domain plus
    // identical broker key and destination is what triggers the lock rejection.
    let xdg_root = tempfile::TempDir::new().expect("probe XDG_STATE_HOME root");
    let owner_marker_dir = tempfile::TempDir::new().expect("owner marker directory");
    let owner_marker = owner_marker_dir.path().join("handoff.marker");
    let contender_marker_dir = tempfile::TempDir::new().expect("contender marker directory");
    let contender_marker = contender_marker_dir.path().join("handoff.marker");

    // Owner: subscribes, accepts one message, then blocks holding the journal.
    let mut owner = spawn_probe(
        &binary,
        &broker.broker_url,
        xdg_root.path(),
        queue,
        &owner_marker,
    );
    let (producer_proc, producer_channel) = spawn_jms_bridge(&binary, &broker.broker_url).await;
    let mut producer = BridgeServiceClient::new(producer_channel);
    await_broker_connected(&mut producer).await;
    send_one(&mut producer, queue, "lock-owner-seed").await;

    let watch = owner_marker.clone();
    wait_until(
        "journal owner accepts its seed",
        Duration::from_secs(60),
        Duration::from_millis(200),
        move || {
            let watch = watch.clone();
            async move { Ok(watch.is_file()) }
        },
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));

    // Contender: same domain + destination. Its bridge reaches Ready, then the
    // Subscribe RPC is permanently rejected asynchronously after start().
    let mut contender = spawn_probe(
        &binary,
        &broker.broker_url,
        xdg_root.path(),
        queue,
        &contender_marker,
    );
    let status = contender
        .wait_bounded(Duration::from_secs(120))
        .await
        .expect("contender probe must exit after the permanent subscribe rejection");
    assert!(
        !status.success(),
        "a permanent subscribe rejection must exit the probe non-zero"
    );
    assert!(
        !contender_marker.exists(),
        "the contender never accepted an exchange, so it must not write a marker"
    );

    owner.kill_and_reap().await;
    drop(producer);
    drop(producer_proc);
}

/// Regression (reviewer): a group leader can exit while its descendant stays
/// alive. The guard captured the group id at spawn, so `Drop` still reaps the
/// descendant after the leader was reaped (`Child::id()` is `None` by then).
/// No readiness sleep: the leader writes the descendant pid before exiting.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn jms_crash_probe_guard_reaps_descendant_after_leader_exit() {
    let dir = tempfile::TempDir::new().expect("descendant pid directory");
    let pid_file = dir.path().join("descendant.pid");

    // The leader backgrounds `sleep` (same process group) and exits; the
    // descendant outlives it.
    let mut command = tokio::process::Command::new("sh");
    command
        .arg("-c")
        .arg("sleep 300 & echo $! > \"$1\"")
        .arg("sh")
        .arg(&pid_file);
    let mut guard = spawn_group_leader(command);

    let status = guard
        .wait_bounded(Duration::from_secs(30))
        .await
        .expect("group leader must exit on its own");
    assert!(status.success(), "leader should exit successfully");

    let descendant: i32 = std::fs::read_to_string(&pid_file)
        .expect("leader wrote descendant pid")
        .trim()
        .parse()
        .expect("parse descendant pid");
    assert!(
        process_alive(descendant),
        "the descendant must outlive the leader"
    );

    // Reaping the leader set `Child::id()` to `None`; the guard must still
    // signal the process group it owns.
    drop(guard);
    let wait_pid = descendant;
    wait_until(
        "descendant reaped by guard",
        Duration::from_secs(10),
        Duration::from_millis(100),
        move || async move { Ok::<bool, String>(!process_alive(wait_pid)) },
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
}

/// Regression: the handoff reader must reject an absent artifact and a stale
/// one, naming BOTH process groups in the error so a topology failure is
/// actionable. No broker and no runtime.
#[cfg(unix)]
#[test]
fn jms_crash_probe_handoff_missing_or_stale_names_both_groups() {
    let dir = tempfile::TempDir::new().expect("handoff directory");
    let exe = Path::new("/bin/sh");

    let absent = read_bridge_handoff(&bridge_handoff_path(dir.path(), 4242), 4242, exe)
        .expect_err("an absent handoff must be an error");
    assert!(
        absent.contains("probe group 4242"),
        "absent error must name probe group 4242: {absent}"
    );
    assert!(
        absent.contains("supervisor group"),
        "absent error must name the unknown supervisor group: {absent}"
    );

    let stale_path = bridge_handoff_path(dir.path(), 7777);
    std::fs::write(
        &stale_path,
        r#"{"probe_pid":7777,"bridge_pid":1,"supervisor_pgid":1}"#,
    )
    .expect("write stale handoff");
    let stale =
        read_bridge_handoff(&stale_path, 4242, exe).expect_err("a stale handoff must be an error");
    assert!(
        stale.contains("stale"),
        "stale error must say the handoff is stale: {stale}"
    );
    assert!(
        stale.contains("probe group 4242"),
        "stale error must name the expected probe group 4242: {stale}"
    );
    assert!(
        stale.contains("supervisor group 1"),
        "stale error must name the artifact's supervisor group 1: {stale}"
    );
}

/// True while `pid` is a live (non-zombie) process. Reads `/proc/<pid>/stat`
/// so a reaped-but-not-yet-cleaned zombie does not count as alive.
#[cfg(unix)]
fn process_alive(pid: i32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // The comm field is parenthesized and may contain spaces; the process state
    // is the first token after the closing paren.
    let Some(rest) = stat.rsplit_once(')').map(|(_, rest)| rest) else {
        return true;
    };
    rest.split_whitespace()
        .next()
        .is_some_and(|state| state != "Z")
}

// ── Task 5.2/5.3: crash-before-completion redelivery proof ───────────────────

const CRASH_BODY: &str = "crash-before-completion-body";
const CRASH_HEADER: &str = "x-crash-proof";
const CRASH_HEADER_VALUE: &str = "crash-before-completion-header";
/// Artemis-correct user-header name for the same contract.
///
/// JMS requires a property name to be a valid message-selector identifier
/// (Java identifier: letters/digits/`_`/`$`, first char a letter). The Classic
/// fixture's `x-crash-proof` has a hyphen and is therefore not a legal JMS
/// property; ActiveMQ Classic tolerates it, but the Artemis JMS client rejects
/// `setStringProperty` and the bridge's producer swallows the exception and
/// drops the header. A legal identifier proves the same "header inherited"
/// contract on Artemis without relying on lenient Classic behavior.
const ARTEMIS_CRASH_HEADER: &str = "x_crash_proof";

/// The dedicated broker a crash proof runs against. Classic and Artemis share
/// one contract; only the broker type, credentials, and readiness barrier
/// differ, so the proof body is parameterized over this rather than copied.
enum CrashBroker {
    Classic(std::sync::Arc<ActiveMqBroker>),
    Artemis(std::sync::Arc<ArtemisBroker>),
}

impl CrashBroker {
    fn broker_url(&self) -> &str {
        match self {
            CrashBroker::Classic(broker) => &broker.broker_url,
            CrashBroker::Artemis(broker) => &broker.broker_url,
        }
    }

    /// Component/`BrokerType` for the journal broker-key derivation.
    fn broker_type(&self) -> BrokerType {
        match self {
            CrashBroker::Classic(_) => BrokerType::ActiveMq,
            CrashBroker::Artemis(_) => BrokerType::Artemis,
        }
    }

    /// `CAMEL_JMS_BROKER_TYPE` value carried to the probe child.
    fn broker_type_env(&self) -> &'static str {
        match self {
            CrashBroker::Classic(_) => "activemq",
            CrashBroker::Artemis(_) => "artemis",
        }
    }

    /// Explicit credentials from the dedicated fixture (never ambient).
    fn credentials(&self) -> (&'static str, &'static str) {
        match self {
            CrashBroker::Classic(_) => (
                super::support::activemq::ACTIVEMQ_USER,
                super::support::activemq::ACTIVEMQ_PASSWORD,
            ),
            CrashBroker::Artemis(_) => (ARTEMIS_USER, ARTEMIS_PASSWORD),
        }
    }

    fn label(&self) -> &'static str {
        self.broker_type_env()
    }

    /// The user-header name the proof sends and asserts. Classic's hyphenated
    /// fixture name is not a legal JMS property; Artemis uses a legal
    /// identifier so the header actually round-trips (see [`ARTEMIS_CRASH_HEADER`]).
    fn crash_header(&self) -> &'static str {
        match self {
            CrashBroker::Classic(_) => CRASH_HEADER,
            CrashBroker::Artemis(_) => ARTEMIS_CRASH_HEADER,
        }
    }

    /// Broker-side readiness barrier before the send. Classic can prove the
    /// consumer is registered through its Jolokia web console; the Artemis
    /// fixture exposes no equivalent console, so the durable queue retains the
    /// send and the handoff marker remains the delivery barrier (the marker is
    /// written only from the first processor of an accepted exchange, so a
    /// send-before-subscribe race cannot produce a false positive).
    async fn wait_consumer_registered(&self, queue: &str) {
        let CrashBroker::Classic(broker) = self else {
            return;
        };
        let broker = broker.clone();
        let queue = queue.to_string();
        wait_until(
            "probe consumer registered on the queue",
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
}

/// Tasks 5.2 (Classic) and 5.3 (Artemis): a kill at the handoff barrier leaves
/// the message unacknowledged, the broker redelivers it, and the survivor
/// inherits the durable reservation count recorded by the killed probe.
///
/// Proof contract (spec `JMS bridge crash before route completion preserves the
/// message`):
/// - the probe's first accepted exchange writes the handoff marker and blocks;
/// - the durable journal has exactly one `RESERVE` frame for the message and no
///   `COMMIT` before the kill;
/// - `SIGKILL` reaches ONLY the probe; its death closes the liveness pipe, so
///   the watchdog kills the supervisor group (bridge included) before the
///   survivor asserts;
/// - the survivor receives the same `JMSMessageID`, body, and user header, and
///   its Java bridge inherits the on-disk count and reserves a second time
///   (`CamelJmsReservationCount == 2`), which a fresh journal could not show;
/// - the journal then holds two `RESERVE` frames and still no `COMMIT`;
/// - the harness terminates the survivor after the assertions and its watchdog
///   kills the survivor's bridge.
#[cfg(unix)]
async fn run_crash_before_completion_proof(broker: CrashBroker, queue: &str) {
    init_tracing();
    install_crypto_provider();

    let binary = require_jms_bridge_binary();
    let (username, password) = broker.credentials();
    let probe_broker = || ProbeBroker {
        url: broker.broker_url(),
        broker_type_env: broker.broker_type_env(),
        username,
        password,
    };

    // ONE stable test-owned journal domain shared by probe and survivor. The
    // per-spawn `TempDir` used elsewhere would lose the recorded count and the
    // survivor could not acquire the same journal (Task 5.2 step 4).
    let xdg_root = tempfile::TempDir::new().expect("shared XDG_STATE_HOME root");
    let probe_dir = tempfile::TempDir::new().expect("probe artifact dir");
    let probe_marker = probe_dir.path().join("handoff.marker");
    let survivor_dir = tempfile::TempDir::new().expect("survivor artifact dir");
    let survivor_receipt = survivor_dir.path().join("receipt.json");

    // Probe: subscribes, accepts one exchange, writes the marker, blocks.
    let mut probe = spawn_probe_configured(
        &binary,
        &probe_broker(),
        xdg_root.path(),
        queue,
        &probe_marker,
        "probe",
    );
    let probe_pgid = probe.pgid().expect("probe process-group id");

    // Broker-side readiness barrier: the consumer is registered before the send,
    // and the marker must not exist yet (it is written only from the first
    // processor of an accepted exchange).
    broker.wait_consumer_registered(queue).await;
    assert!(
        !probe_marker.exists(),
        "probe must not write a marker before any delivery"
    );

    // Send one known message (body plus a user header) through the existing
    // producer-process helper.
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
    let known_id = send_one_with_header(
        &mut producer,
        queue,
        CRASH_BODY,
        broker.crash_header(),
        CRASH_HEADER_VALUE,
    )
    .await;
    assert!(
        !known_id.is_empty(),
        "the known send must return a JMSMessageID"
    );

    // Marker appears only from the accepted exchange; the probe is still
    // blocked inside its first processor.
    let marker_watch = probe_marker.clone();
    wait_until(
        "probe handoff marker",
        Duration::from_secs(60),
        Duration::from_millis(200),
        move || {
            let watch = marker_watch.clone();
            async move { Ok(watch.is_file()) }
        },
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        std::fs::read_to_string(&probe_marker)
            .expect("read probe handoff marker")
            .trim(),
        "accepted",
        "handoff marker must record an accepted exchange"
    );
    assert!(
        probe.is_alive(),
        "the probe must stay blocked in its first processor until killed"
    );

    // Durable journal BEFORE the kill: exactly one reservation recorded for the
    // known id and no COMMIT. This is the inherited count a survivor must see.
    let broker_dir = journal_broker_dir(
        xdg_root.path(),
        "default",
        &broker.broker_type(),
        broker.broker_url(),
    );
    let journal = wait_for_journal(&broker_dir).await;
    let frames = read_frames(&journal).unwrap_or_else(|e| panic!("{e}"));
    let probe_reserves = reservation_count(&frames, &known_id);
    assert_eq!(
        probe_reserves, 1,
        "the probe must record exactly one durable reservation before the handoff"
    );
    assert!(
        !commit_observed(&frames, &known_id),
        "no COMMIT may be observed before the kill"
    );

    let lock = lock_path(&journal);

    // The probe published its bridge topology before the marker, so the handoff
    // is guaranteed visible here. Validate it instead of scanning the probe
    // group: the bridge lives in the watchdog-led supervisor group, which a
    // probe-group scan can never see.
    let handoff = read_bridge_handoff(
        &bridge_handoff_path(xdg_root.path(), probe_pgid as u32),
        probe_pgid as u32,
        &binary,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let bridge_pid = handoff.bridge_pid;
    let supervisor_pgid = handoff.supervisor_pgid;
    let owner_before = lock_owner_pid(&lock)
        .unwrap_or_else(|e| panic!("lock owner query before kill failed: {e}"));
    assert_eq!(
        owner_before,
        Some(bridge_pid as u32),
        "the destination lock must be owned by the probe's exact bridge process {bridge_pid}"
    );

    // Kill ONLY the probe. The bridge is not in the probe group, so it is not
    // signalled here; the probe's death closes the liveness pipe instead.
    probe.kill_leader_only();
    let probe_pid = probe_pgid;
    wait_until(
        "probe leader death",
        Duration::from_secs(30),
        Duration::from_millis(100),
        move || async move { Ok(!process_alive(probe_pid)) },
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    // Death phase: the probe's death closed the liveness pipe, so the watchdog
    // must SIGKILL the supervisor group (watchdog + exact owner bridge). A
    // surviving bridge is a hard failure that names the watchdog.
    wait_until(
        "watchdog killed the exact owner bridge",
        Duration::from_secs(30),
        Duration::from_millis(100),
        move || async move { Ok(!process_alive(bridge_pid)) },
    )
    .await
    .unwrap_or_else(|e| {
        panic!(
            "{e}: the liveness-pipe watchdog did not kill bridge {bridge_pid} after probe group \
             {probe_pgid} died; supervisor group {supervisor_pgid} still holds it — a bridge \
             surviving the probe means the watchdog is broken"
        )
    });
    wait_until(
        "supervisor group empty after probe death",
        Duration::from_secs(30),
        Duration::from_millis(100),
        move || async move { Ok(live_group_members(supervisor_pgid).is_empty()) },
    )
    .await
    .unwrap_or_else(|e| {
        panic!(
            "{e}: supervisor group {supervisor_pgid} still holds live members after probe group \
             {probe_pgid} died — the liveness-pipe watchdog is broken"
        )
    });

    // The journal lock is an OS lock: the kernel releases it when the bridge
    // process exits. Observe that release through the bounded poll.
    let lock_watch = lock.clone();
    wait_until(
        "destination lock released at bridge exit",
        Duration::from_secs(30),
        Duration::from_millis(250),
        move || {
            let lock = lock_watch.clone();
            async move {
                match lock_owner_pid(&lock) {
                    Ok(None) => Ok(true),
                    Ok(Some(owner)) => Ok(owner != bridge_pid as u32),
                    Err(_) => Ok(false),
                }
            }
        },
    )
    .await
    .unwrap_or_else(|e| {
        let observed = match lock_owner_pid(&lock) {
            Ok(owner) => format!("{owner:?}"),
            Err(err) => format!("<query error: {err}>"),
        };
        panic!(
            "{e}: destination lock {} is still observed owned by {observed} after the \
             liveness-pipe watchdog killed bridge {bridge_pid} (probe group {probe_pgid}, \
             supervisor group {supervisor_pgid}) — the watchdog is broken",
            lock.display()
        )
    });
    let lock_owner_after_release = lock_owner_pid(&lock);

    // Survivor: same queue and journal domain. It writes a receipt from its
    // first accepted exchange and blocks, so no COMMIT is ever observed.
    let mut survivor = spawn_probe_configured(
        &binary,
        &probe_broker(),
        xdg_root.path(),
        queue,
        &survivor_receipt,
        "survivor",
    );
    let receipt_watch = survivor_receipt.clone();
    wait_until(
        "survivor receipt",
        Duration::from_secs(120),
        Duration::from_millis(200),
        move || {
            let watch = receipt_watch.clone();
            async move { Ok(watch.is_file()) }
        },
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));

    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&survivor_receipt).expect("read survivor receipt"))
            .expect("survivor receipt must be JSON");

    assert_eq!(
        receipt["message_id"].as_str().unwrap_or_default(),
        known_id,
        "the survivor must receive the same JMSMessageID"
    );
    assert_eq!(
        receipt["body"].as_str().unwrap_or_default(),
        CRASH_BODY,
        "the survivor must receive the same body bytes"
    );
    assert_eq!(
        receipt["headers"][broker.crash_header()].as_str(),
        Some(CRASH_HEADER_VALUE),
        "the survivor must receive the same user header"
    );
    // 2, not 1: the survivor's Java bridge replayed the killed probe's durable
    // reservation and recorded a second attempt. A fresh journal (the per-spawn
    // TempDir used elsewhere) would report 1, indistinguishable from a first
    // delivery if only `JMSXDeliveryCount` were checked.
    assert_eq!(
        receipt["reservation_count"].as_u64(),
        Some(2),
        "the survivor must inherit the durable reservation count (1) then reserve again (2)"
    );

    // Durable journal AFTER the survivor: two reservations, still no COMMIT.
    let frames = read_frames(&journal).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        reservation_count(&frames, &known_id),
        2,
        "the durable journal must hold both reservations (killed probe + survivor)"
    );
    assert!(
        !commit_observed(&frames, &known_id),
        "no COMMIT may ever be observed for the unacknowledged message"
    );

    // One auditable evidence line for the Phase-5 report: broker, exact ids,
    // lock owners, durable reservation counts, and the supervisor topology.
    eprintln!(
        "PROOF 5.2/5.3 [{}]: known_message_id={known_id} lock_owner_before={owner_before:?} \
         lock_owner_after_release={lock_owner_after_release:?} probe_journal_reserves={probe_reserves} \
         survivor_receipt_reservation_count={} survivor_journal_reserves={} commit_observed={} \
         body_ok={} header_ok={} bridge_pid={bridge_pid} supervisor_pgid={supervisor_pgid} \
         bridge_died_after_probe=true supervisor_group_empty=true lock_owner_released=true",
        broker.label(),
        receipt["reservation_count"].as_u64().unwrap_or_default(),
        reservation_count(&frames, &known_id),
        commit_observed(&frames, &known_id),
        receipt["body"].as_str() == Some(CRASH_BODY),
        receipt["headers"][broker.crash_header()].as_str() == Some(CRASH_HEADER_VALUE),
    );

    // The survivor is still blocked (no COMMIT): only now do the assertions end
    // and cleanup begin.
    assert!(
        survivor.is_alive(),
        "the survivor must stay blocked so no COMMIT is observed"
    );

    // Read the survivor's own topology while it is alive: it published at its
    // accepted exchange, and `kill_and_reap` disarms the pgid needed to locate
    // the artifact.
    let survivor_pgid = survivor.pgid().expect("survivor pgid");
    let survivor_handoff = read_bridge_handoff(
        &bridge_handoff_path(xdg_root.path(), survivor_pgid as u32),
        survivor_pgid as u32,
        &binary,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let survivor_bridge_pid = survivor_handoff.bridge_pid;

    // Terminate the survivor; its death closes its liveness pipe, so its
    // watchdog kills the survivor's bridge. The probe is already dead and its
    // group is empty, so `kill_and_reap` only reaps the leader.
    survivor.kill_and_reap().await;
    probe.kill_and_reap().await;
    drop(producer);
    drop(producer_proc);
    wait_until(
        "owner bridge terminated after assertions",
        Duration::from_secs(30),
        Duration::from_millis(100),
        move || async move { Ok(!process_alive(bridge_pid)) },
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    wait_until(
        "survivor bridge reaped by its watchdog",
        Duration::from_secs(30),
        Duration::from_millis(100),
        move || async move { Ok(!process_alive(survivor_bridge_pid)) },
    )
    .await
    .unwrap_or_else(|e| {
        panic!(
            "{e}: survivor bridge {survivor_bridge_pid} (supervisor group {}) outlived the \
             survivor (probe group {survivor_pgid}) — its liveness-pipe watchdog is broken",
            survivor_handoff.supervisor_pgid
        )
    });
}

/// Task 5.2: Classic crash redelivery proof. Runs against a dedicated,
/// non-shared Classic broker so broker-side state is exact.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn jms_bridge_crash_before_completion_redelivers_activemq() {
    let broker = CrashBroker::Classic(std::sync::Arc::new(ActiveMqBroker::start().await));
    run_crash_before_completion_proof(broker, "queue.test-crash-before-completion-activemq").await;
}

/// Task 5.3: Artemis crash redelivery proof. Mirrors the Classic proof against
/// a dedicated, mandatory-auth Artemis broker with explicit fixture
/// credentials (no ANONYMOUS_LOGIN).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn jms_bridge_crash_before_completion_redelivers_artemis() {
    let broker = CrashBroker::Artemis(std::sync::Arc::new(ArtemisBroker::start().await));
    run_crash_before_completion_proof(broker, "queue.test-crash-before-completion-artemis").await;
}

/// Freezes a broker container with `docker pause` and unfreezes it on drop, so
/// a failed assertion can never leave the container paused.
struct PausedBroker {
    container_id: String,
    paused: bool,
}

impl PausedBroker {
    fn pause(container_id: &str) -> Self {
        docker(&["pause", container_id]);
        Self {
            container_id: container_id.to_string(),
            paused: true,
        }
    }

    fn unpause(&mut self) {
        if self.paused {
            docker(&["unpause", &self.container_id]);
            self.paused = false;
        }
    }
}

impl Drop for PausedBroker {
    fn drop(&mut self) {
        if self.paused {
            let _ = std::process::Command::new("docker")
                .args(["unpause", &self.container_id])
                .status();
        }
    }
}

fn docker(args: &[&str]) {
    let status = std::process::Command::new("docker")
        .args(args)
        .status()
        .unwrap_or_else(|e| panic!("cannot run docker {args:?}: {e}"));
    assert!(status.success(), "docker {args:?} failed: {status}");
}

/// rc-47gr5: a journal COMMIT implies the broker applied the ack.
///
/// The broker is frozen (`docker pause`) after the exchange is accepted and
/// before the route completes. The bridge's `acknowledge()` cannot be answered
/// while the broker is frozen, so no COMMIT frame may reach the journal. With
/// asynchronous acks `acknowledge()` returns as soon as the packet is queued and
/// the COMMIT lands immediately; a crash right then would redeliver a message
/// whose reservation was already cleared, with a fresh attempt budget. After the
/// unfreeze the COMMIT must appear, proving it was pending behind the ack.
async fn run_commit_implies_broker_ack_proof(broker: CrashBroker, container_id: &str, queue: &str) {
    init_tracing();
    install_crypto_provider();

    let binary = require_jms_bridge_binary();
    let (username, password) = broker.credentials();
    let probe_broker = ProbeBroker {
        url: broker.broker_url(),
        broker_type_env: broker.broker_type_env(),
        username,
        password,
    };
    let xdg_root = tempfile::TempDir::new().expect("XDG_STATE_HOME root");
    let artifact_dir = tempfile::TempDir::new().expect("probe artifact dir");
    let marker = artifact_dir.path().join("handoff.marker");
    let release = artifact_dir.path().join("handoff.marker.release");

    let mut probe = spawn_probe_configured(
        &binary,
        &probe_broker,
        xdg_root.path(),
        queue,
        &marker,
        "gated",
    );
    broker.wait_consumer_registered(queue).await;

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
    let known_id = send_one_with_header(
        &mut producer,
        queue,
        CRASH_BODY,
        broker.crash_header(),
        CRASH_HEADER_VALUE,
    )
    .await;
    assert!(!known_id.is_empty(), "the send must return a JMSMessageID");
    // The producer is not needed past the send; stop it so the frozen broker
    // cannot stall it.
    drop(producer);
    drop(producer_proc);

    let marker_watch = marker.clone();
    wait_until(
        "gated probe handoff marker",
        Duration::from_secs(60),
        Duration::from_millis(200),
        move || {
            let watch = marker_watch.clone();
            async move { Ok(watch.is_file()) }
        },
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));

    let broker_dir = journal_broker_dir(
        xdg_root.path(),
        "default",
        &broker.broker_type(),
        broker.broker_url(),
    );
    let journal = wait_for_journal(&broker_dir).await;

    // Freeze the broker, then let the route complete: the bridge now tries to
    // acknowledge against a broker that cannot answer.
    let mut frozen = PausedBroker::pause(container_id);
    std::fs::write(&release, b"go\n").expect("write release file");

    let observed = {
        let journal = journal.clone();
        let known_id = known_id.clone();
        wait_until(
            "journal COMMIT while the broker is frozen",
            Duration::from_secs(5),
            Duration::from_millis(100),
            move || {
                let journal = journal.clone();
                let known_id = known_id.clone();
                async move {
                    let frames = read_frames(&journal)?;
                    Ok(commit_observed(&frames, &known_id))
                }
            },
        )
        .await
    };
    assert!(
        observed.is_err(),
        "{}: the journal recorded COMMIT while the broker could not have applied the ack",
        broker.label()
    );

    frozen.unpause();
    let committed = journal.clone();
    let committed_id = known_id.clone();
    wait_until(
        "journal COMMIT after the broker resumed",
        Duration::from_secs(60),
        Duration::from_millis(200),
        move || {
            let journal = committed.clone();
            let known_id = committed_id.clone();
            async move {
                let frames = read_frames(&journal)?;
                Ok(commit_observed(&frames, &known_id))
            }
        },
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));

    probe.kill_and_reap().await;
}

/// rc-47gr5: ActiveMQ Classic (`sendAcksAsync=false`).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn jms_bridge_commit_implies_broker_ack_activemq() {
    let broker = std::sync::Arc::new(ActiveMqBroker::start().await);
    let container_id = broker.container_id().to_string();
    run_commit_implies_broker_ack_proof(
        CrashBroker::Classic(broker),
        &container_id,
        "queue.test-commit-implies-ack-activemq",
    )
    .await;
}

/// rc-47gr5: Artemis (`blockOnAcknowledge=true`).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn jms_bridge_commit_implies_broker_ack_artemis() {
    let broker = std::sync::Arc::new(ArtemisBroker::start().await);
    let container_id = broker.container_id().to_string();
    run_commit_implies_broker_ack_proof(
        CrashBroker::Artemis(broker),
        &container_id,
        "queue.test-commit-implies-ack-artemis",
    )
    .await;
}

/// Sends one text message carrying a single user header and returns the
/// provider message id. Mirrors `send_one`, but the crash proof needs a header
/// that must be forwarded unchanged on redelivery.
async fn send_one_with_header(
    client: &mut BridgeServiceClient<tonic::transport::Channel>,
    queue: &str,
    body: &str,
    header: &str,
    value: &str,
) -> String {
    let response = tokio::time::timeout(
        Duration::from_secs(30),
        client.send(SendRequest {
            destination: queue.to_string(),
            body: body.as_bytes().to_vec(),
            headers: HashMap::from([(header.to_string(), value.to_string())]),
            content_type: "text/plain".to_string(),
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("timed out sending body {body:?}"))
    .unwrap_or_else(|e| panic!("send of body {body:?} failed: {e}"));
    response.into_inner().message_id
}

/// Waits, bounded, for the bridge to create the destination journal, then
/// returns its path.
async fn wait_for_journal(broker_dir: &Path) -> PathBuf {
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
