//! Crash probe for the Phase-5 real-broker crash proof.
//!
//! The probe starts a JMS consumer route whose FIRST step writes a
//! handoff-marker file and then blocks forever. The marker is therefore
//! written only after the consumer processor accepts a real exchange, never
//! at startup or readiness; blocking keeps that accepted exchange
//! uncommitted so the test can kill the process before settlement.
//!
//! Configuration is read from the process environment only, so tests pass it
//! through the child `Command` without mutating the parent's global env
//! (`set_var` races under parallel test threads):
//!
//! - argv[1] (required): artifact path. In the `probe` role it is the
//!   handoff-marker file; in the `survivor` role it is the receipt JSON the
//!   survivor writes from its first accepted exchange.
//! - `CAMEL_JMS_BROKER_URL` (required): broker URL (for example
//!   `tcp://127.0.0.1:61616`).
//! - `CAMEL_JMS_BRIDGE_BINARY_PATH` (required): jms-bridge binary the
//!   component launches.
//! - `CAMEL_JMS_BROKER_TYPE` (optional): `activemq` (default) or `artemis`.
//! - `CAMEL_JMS_CRASH_ROLE` (optional): `probe` (default) writes `accepted\n`
//!   to the artifact and blocks; `survivor` writes a receipt JSON with
//!   `message_id`, `body`, `reservation_count`, and the full header map, then
//!   blocks; `threshold` appends one accepted-exchange line to the artifact and
//!   returns a route error on every delivery, so the bridge redelivers until
//!   the terminal reject-then-log policy fires (task 5.4); `gated` writes the
//!   handoff marker, waits for `<artifact>.release`, then completes the
//!   exchange so the bridge settles with COMMIT (rc-47gr5).
//! - `CAMEL_JMS_USERNAME` / `CAMEL_JMS_PASSWORD` (optional): credentials;
//!   default `admin`/`admin` for the Classic test container.
//! - `CAMEL_JMS_CRASH_QUEUE` (optional): queue to consume, default
//!   `queue.test-crash-probe`.
//! - `XDG_STATE_HOME` (inherited): attempt-journal domain root.
//!
//! Any setup failure prints a diagnostic to stderr and exits non-zero.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_jms::{BrokerConfig, BrokerType, JmsBridgePool, JmsComponent, JmsPoolConfig};
use camel_core::CamelContext;

const DEFAULT_QUEUE: &str = "queue.test-crash-probe";
const DEFAULT_USERNAME: &str = "admin";
const DEFAULT_PASSWORD: &str = "admin";
const BRIDGE_START_TIMEOUT_MS: u64 = 90_000;
/// Single broker name shared by every probe role, so journal keys derived by
/// the component stay identical across a crash and its survivor.
const BROKER_NAME: &str = "default";
const ROUTE_ID: &str = "jms-crash-probe";
/// How often the parked probe refreshes the route status projection.
const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(200);

#[tokio::main]
async fn main() -> ExitCode {
    // Fresh process: the JMS bridge client uses rustls, and rustls 0.23 needs
    // an explicitly installed process-level CryptoProvider. The test process
    // installs its own for its bridge helpers; a spawned probe cannot inherit
    // that, so it installs here before any route starts.
    let _ = rustls::crypto::ring::default_provider().install_default();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("jms_crash_probe: setup failed: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let artifact = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| "usage: jms_crash_probe <handoff-marker-or-receipt-path>".to_string())?;

    let broker_url = required_env("CAMEL_JMS_BROKER_URL")?;
    // The component resolves this itself when it launches the bridge; requiring
    // it here turns a missing native binary into a clean non-zero setup exit.
    let _bridge_binary = required_env("CAMEL_JMS_BRIDGE_BINARY_PATH")?;
    let broker_type = parse_broker_type(optional_env("CAMEL_JMS_BROKER_TYPE").as_deref())?;
    let role = parse_role(optional_env("CAMEL_JMS_CRASH_ROLE").as_deref())?;
    let username =
        optional_env("CAMEL_JMS_USERNAME").unwrap_or_else(|| DEFAULT_USERNAME.to_string());
    let password =
        optional_env("CAMEL_JMS_PASSWORD").unwrap_or_else(|| DEFAULT_PASSWORD.to_string());
    let queue = optional_env("CAMEL_JMS_CRASH_QUEUE").unwrap_or_else(|| DEFAULT_QUEUE.to_string());

    let pool_config = JmsPoolConfig {
        max_bridges: 1,
        brokers: HashMap::from([(
            BROKER_NAME.to_string(),
            BrokerConfig {
                broker_url,
                broker_type,
                username: Some(username),
                password: Some(password),
            },
        )]),
        bridge_start_timeout_ms: BRIDGE_START_TIMEOUT_MS,
        ..JmsPoolConfig::default()
    };
    let pool = JmsBridgePool::from_config(pool_config)
        .map_err(|e| format!("invalid JMS pool config: {e}"))?;
    let component = JmsComponent::with_scheme("jms", std::sync::Arc::new(pool));

    let mut ctx = CamelContext::builder()
        .build()
        .await
        .map_err(|e| format!("cannot build Camel context: {e}"))?;
    ctx.register_component(component);

    let from_uri = format!("jms:queue:{queue}");
    let route = RouteBuilder::from(from_uri.as_str())
        .process(move |exchange: camel_api::Exchange| {
            let artifact = artifact.clone();
            // `ProbeRole` is `Copy`, so the `async move` block copies `role`
            // and the outer capture stays usable across invocations.
            async move {
                match role {
                    ProbeRole::Probe => write_probe_marker(&artifact)?,
                    ProbeRole::Survivor => write_survivor_receipt(&artifact, &exchange).await?,
                    ProbeRole::Threshold => {
                        // Record one accepted-exchange line, then fail the
                        // route so the Rust consumer sends ROLLBACK and the
                        // bridge redelivers. Never include the body or a header
                        // value in the route error: the terminal bridge log
                        // must be the only payload-free disposition record.
                        append_threshold_event(&artifact, &exchange)?;
                        return Err(camel_api::CamelError::ProcessorError(
                            THRESHOLD_ROUTE_ERROR.to_string(),
                        ));
                    }
                    ProbeRole::Gated => {
                        write_probe_marker(&artifact)?;
                        let mut release = artifact.as_os_str().to_owned();
                        release.push(".release");
                        let release = PathBuf::from(release);
                        while !release.is_file() {
                            tokio::time::sleep(STATUS_POLL_INTERVAL).await;
                        }
                        return Ok(exchange);
                    }
                }
                // Hold the accepted exchange uncommitted until the test kills
                // this process. Deliberately unbounded: the kill is the test's
                // barrier, not a route timeout.
                std::future::pending::<()>().await;
                Ok(exchange)
            }
        })
        .route_id(ROUTE_ID)
        .build()
        .map_err(|e| format!("cannot build crash-probe route: {e}"))?;

    ctx.add_route_definition(route)
        .await
        .map_err(|e| format!("cannot register crash-probe route: {e}"))?;
    ctx.start()
        .await
        .map_err(|e| format!("cannot start crash-probe context: {e}"))?;

    // `start()` is not a subscribe barrier: the JMS consumer marks its startup
    // signal ready right after spawning its worker tasks, before the
    // asynchronous `Subscribe` RPC (consumer.rs `ctx.mark_ready()`). A permanent
    // subscribe rejection therefore lands after `start()` returned Ok and
    // surfaces as a terminal `Failed` route status. Poll the route status
    // projection while parked: a `Failed` route is a setup failure and must
    // exit non-zero instead of hanging. A healthy route stays `Started`, and
    // its first processor holds any accepted exchange until the test kills the
    // process.
    loop {
        match ctx.runtime_route_status(ROUTE_ID).await {
            Ok(Some(status)) if status == "Failed" => {
                return Err(format!(
                    "crash-probe route '{ROUTE_ID}' failed after start \
                     (asynchronous subscribe/startup rejection)"
                ));
            }
            Ok(Some(_)) => {}
            Ok(None) => {
                return Err(format!("crash-probe route '{ROUTE_ID}' is not registered"));
            }
            Err(e) => {
                return Err(format!("cannot read crash-probe route status: {e}"));
            }
        }
        tokio::time::sleep(STATUS_POLL_INTERVAL).await;
    }
}

fn required_env(name: &str) -> Result<String, String> {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => Ok(value),
        _ => Err(format!("{name} is required and must be non-empty")),
    }
}

/// Optional env value: unset or empty both mean "not provided".
fn optional_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn parse_broker_type(raw: Option<&str>) -> Result<BrokerType, String> {
    match raw {
        None | Some("activemq") | Some("activemq-classic") => Ok(BrokerType::ActiveMq),
        Some("artemis") => Ok(BrokerType::Artemis),
        Some(other) => Err(format!(
            "unknown CAMEL_JMS_BROKER_TYPE '{other}': expected activemq or artemis"
        )),
    }
}

/// Which artifact the first accepted exchange produces.
#[derive(Clone, Copy)]
enum ProbeRole {
    /// Write the handoff marker. The exchange is held uncommitted.
    Probe,
    /// Write the survivor receipt. The exchange is held uncommitted.
    Survivor,
    /// Append one accepted-exchange line and fail the route on every delivery,
    /// so the bridge redelivers until its terminal reject-then-log policy fires.
    Threshold,
    /// Write the handoff marker, wait for `<artifact>.release`, then complete the
    /// exchange normally so the bridge settles with COMMIT (rc-47gr5).
    Gated,
}

/// The route error for the `threshold` role. Deliberately payload-free: the
/// bridge's terminal discard line is the only disposition record, and the test
/// asserts no body bytes appear in it.
const THRESHOLD_ROUTE_ERROR: &str = "jms-threshold-always-errors";

fn parse_role(raw: Option<&str>) -> Result<ProbeRole, String> {
    match raw {
        None | Some("probe") => Ok(ProbeRole::Probe),
        Some("survivor") => Ok(ProbeRole::Survivor),
        Some("threshold") => Ok(ProbeRole::Threshold),
        Some("gated") => Ok(ProbeRole::Gated),
        Some(other) => Err(format!(
            "unknown CAMEL_JMS_CRASH_ROLE '{other}': expected probe, survivor, threshold, or gated"
        )),
    }
}

/// Publish `bytes` to `path` with atomic visibility.
///
/// A concurrent reader that observes `path` (the parent test polls `is_file()`
/// and then reads/parses immediately) must see either no file or the COMPLETE
/// artifact, never a partially written one. A sibling temp file on the same
/// filesystem is written first and then renamed over `path`; rename is atomic,
/// so the artifact becomes visible in a single step. The temp path is derived
/// from the target's `OsString`, so a non-UTF-8 target path is preserved.
///
/// No `fsync`: this proves publish atomicity against a concurrent reader, not
/// durability against a machine crash (the probe is `SIGKILL`ed; the page cache
/// survives).
fn write_artifact_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_artifact_atomic_with(path, bytes, |_| {})
}

/// Test seam for [`write_artifact_atomic`]: invokes `on_staged(path)` after the
/// staged temp file exists but before the artifact is published, so a test can
/// prove the target stays invisible while the write is in progress. Production
/// passes a no-op.
fn write_artifact_atomic_with(
    path: &Path,
    bytes: &[u8],
    on_staged: impl FnOnce(&Path),
) -> std::io::Result<()> {
    let temp_path = atomic_temp_sibling(path);
    // Stage the complete artifact in the sibling temp file. Clean it up on any
    // failure so a failed publish never leaves a stale temp behind.
    let staged = (|| {
        let mut file = std::fs::File::create(&temp_path)?;
        on_staged(path);
        file.write_all(bytes)
    })();
    if let Err(e) = staged {
        let _ = std::fs::remove_file(&temp_path);
        return Err(e);
    }
    // Atomic publish: rename replaces the target in one step, so a concurrent
    // reader observes either the old artifact or the complete new one.
    if let Err(e) = std::fs::rename(&temp_path, path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(e);
    }
    Ok(())
}

/// The sibling temp path for `path`: the target's `OsString` plus a `.tmp`
/// suffix. Appending to the `OsString` inserts no separator, so the temp file
/// stays in the target's directory and the rename is same-filesystem (atomic).
/// Working on `OsString` keeps a non-UTF-8 target path intact.
fn atomic_temp_sibling(path: &Path) -> PathBuf {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    PathBuf::from(temp)
}

/// Write `accepted\n` only from the first processor of a real accepted
/// exchange, never at startup or readiness.
fn write_probe_marker(path: &Path) -> Result<(), camel_api::CamelError> {
    write_artifact_atomic(path, b"accepted\n").map_err(|e| {
        camel_api::CamelError::ProcessorError(format!(
            "cannot write handoff marker {}: {e}",
            path.display()
        ))
    })
}

/// Append one accepted-exchange line for the `threshold` role: the provider
/// `JMSMessageID` and the `CamelJmsReservationCount` property the bridge
/// produced from the durable journal. Payload-free by construction. The test
/// counts these lines to prove exactly the configured number of handoffs
/// happened before the terminal policy fired.
fn append_threshold_event(
    path: &Path,
    exchange: &camel_api::Exchange,
) -> Result<(), camel_api::CamelError> {
    let message_id = exchange
        .input
        .header("JMSMessageID")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let reservation_count = exchange
        .property("CamelJmsReservationCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let line = format!("accepted id={message_id} reservation_count={reservation_count}\n");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| {
            camel_api::CamelError::ProcessorError(format!(
                "cannot open threshold event file {}: {e}",
                path.display()
            ))
        })?;
    file.write_all(line.as_bytes()).map_err(|e| {
        camel_api::CamelError::ProcessorError(format!(
            "cannot append threshold event to {}: {e}",
            path.display()
        ))
    })
}

/// Write the survivor receipt from the first accepted exchange: the provider
/// `JMSMessageID`, the body bytes, the `CamelJmsReservationCount` property the
/// Java bridge produced from the durable journal, and the full header map. The
/// processor then blocks, so no `COMMIT` is ever observed for this delivery.
async fn write_survivor_receipt(
    path: &Path,
    exchange: &camel_api::Exchange,
) -> Result<(), camel_api::CamelError> {
    let body_bytes = exchange
        .input
        .body
        .clone()
        .into_bytes(1_048_576)
        .await
        .map_err(|e| {
            camel_api::CamelError::ProcessorError(format!("cannot read survivor body: {e}"))
        })?;
    let body = String::from_utf8_lossy(&body_bytes).into_owned();
    let message_id = exchange
        .input
        .header("JMSMessageID")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let reservation_count = exchange
        .property("CamelJmsReservationCount")
        .and_then(|v| v.as_u64());
    let receipt = serde_json::json!({
        "message_id": message_id,
        "body": body,
        "reservation_count": reservation_count,
        "headers": exchange.input.headers,
    });
    let json = serde_json::to_vec_pretty(&receipt).map_err(|e| {
        camel_api::CamelError::ProcessorError(format!("cannot serialize survivor receipt: {e}"))
    })?;
    write_artifact_atomic(path, &json).map_err(|e| {
        camel_api::CamelError::ProcessorError(format!(
            "cannot write survivor receipt {}: {e}",
            path.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: the parent test polls `is_file()` and then reads/parses the
    /// artifact immediately. A direct `fs::write` publishes the target path
    /// before its bytes exist, so a reader can observe an empty/partial file
    /// and falsely fail. Publishing must be atomic: the target appears, and an
    /// existing complete artifact is replaced, only once the write is done.
    #[test]
    fn artifact_not_visible_before_complete_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("receipt.json");

        // A previous complete artifact must stay intact while the replacement
        // is staged; the atomic rename replaces it only at publish time.
        write_artifact_atomic(&target, b"{\"generation\":1}").expect("write first artifact");

        let observed = std::cell::RefCell::new(None::<Vec<u8>>);
        write_artifact_atomic_with(&target, b"{\"generation\":2}", |target| {
            // Paused after staging, before publish: the target must still be
            // the previous complete artifact, never a partial/empty new one.
            *observed.borrow_mut() = Some(std::fs::read(target).expect("read target while staged"));
        })
        .expect("write replacement artifact");

        assert_eq!(
            observed.into_inner().as_deref(),
            Some(&b"{\"generation\":1}"[..]),
            "the target must not change until the complete artifact is published"
        );
        assert_eq!(
            std::fs::read(&target)
                .expect("read final artifact")
                .as_slice(),
            b"{\"generation\":2}",
            "the complete replacement must be visible after publish"
        );
    }
}
