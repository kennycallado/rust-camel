#![allow(dead_code)]

//! Artemis fixture for the JMS disposition cases: see the `## Artemis
//! broker-side disposition query` section of `crates/camel-test/README.md` for
//! the `docker exec`/`artemis queue stat` mechanism, its non-proofs, and the
//! terminal-kind distinction. Stability is pin-scoped to
//! `apache/activemq-artemis:2.36.0-alpine` only.

use std::process::Stdio;
use std::time::Duration;

use testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{ContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::net::TcpStream;
use tokio::sync::OnceCell;
use tokio::time::{Instant, sleep};

pub const ARTEMIS_PORT: u16 = 61616;
/// Explicit credentials for the dedicated Artemis test fixture (mandatory
/// auth). The Phase-5 crash proof passes these to both the probe and the
/// producer bridge; it never relies on ambient ANONYMOUS_LOGIN.
pub const ARTEMIS_USER: &str = "artemis";
pub const ARTEMIS_PASSWORD: &str = "artemis";

/// Point-in-time management counts for one Artemis queue.
///
/// Mechanism: [`ArtemisBroker::queue_stat`] runs `docker exec` into the
/// dedicated fixture container and invokes the container's own
/// `/var/lib/artemis-instance/bin/artemis queue stat` CLI, then
/// [`parse_queue_stat`] reads the pinned table. Stability is pin-scoped: it is
/// verified on `apache/activemq-artemis:2.36.0-alpine` only. The CLI ships
/// inside the image (version-matched to the broker), the image deploys no
/// console wars — so there is no Jolokia endpoint to query instead — and no
/// extra port has to be exposed. The test-only `artemis`/`artemis` credentials
/// are passed on the command line and are therefore visible in the container's
/// `ps` output.
///
/// These counts are a management-plane snapshot. They do not prove journal
/// layout or byte-level persistence, they are not a universal proof against
/// message loss, and `messages_added` accumulates over the broker's lifetime
/// (exact in these fixtures only because each disposition case owns a fresh
/// container).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueStat {
    /// `MESSAGE COUNT` — messages currently on the queue.
    pub message_count: u64,
    /// `MESSAGES ADDED` — messages ever added to the queue since broker start.
    pub messages_added: u64,
}

/// Runs `command` synchronously under a wall-clock `deadline`.
///
/// `stdin` is null and both output streams are piped. The child is polled with
/// `try_wait` (50 ms sleeps) rather than blocking in `wait`, so an expired
/// deadline can still kill and reap it. On a normal exit the piped output is
/// collected with `Child::wait_with_output`, which returns immediately for an
/// already-exited child and drains both pipes; `Command::output()` is never
/// called on the spawned command because that would re-execute the program.
/// Error strings interpolate the program and its arguments only — stderr is
/// deliberately not spliced in here (the Artemis CLI prints broker-side text).
fn run_with_deadline(
    command: &mut std::process::Command,
    deadline: std::time::Duration,
) -> Result<std::process::Output, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let args: Vec<String> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|err| format!("failed to spawn {program}: {err}"))?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|err| format!("failed to collect output of {program}: {err}"));
            }
            Ok(None) => {}
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("failed to poll {program}: {err}"));
            }
        }
        if started.elapsed() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("deadline expired: {program} {}", args.join(" ")));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Parses `artemis queue stat` output for a single queue.
///
/// Mechanism and pin-scoped stability are documented on [`QueueStat`].
/// Non-`|` lines are ignored (the CLI prints a `Connection brokerURL = ...`
/// preamble). For `|` lines the enclosing delimiters are stripped from both
/// ends before splitting on `|`, and every cell is trimmed — otherwise cell 0
/// is an empty string. The default 2.36.0 output wraps the column names across
/// two header lines, so the pair is validated positionally: header line 1 cell
/// 3 `MESSAGE` with header line 2 cell 3 `COUNT`, and header line 1 cell 4
/// `MESSAGES` with header line 2 cell 4 `ADDED`. A missing or damaged header
/// pair is an error (never `Ok(None)`), so a silent CLI format change fails
/// closed. The pinned CLI also caps column width near 25 chars, so longer
/// queue names wrap onto a second physical row: the first row carries the
/// truncated name and the counts, the next carries the name remainder with
/// empty count cells and is merged back into the pending row. A continuation
/// row with no preceding numeric row is an error. A validated header with no
/// row for `queue` yields `Ok(None)`.
pub fn parse_queue_stat(stdout: &str, queue: &str) -> Result<Option<QueueStat>, String> {
    let rows: Vec<(&str, Vec<&str>)> = stdout
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('|'))
        .map(|line| {
            let inner = line.strip_prefix('|').unwrap_or(line);
            let inner = inner.strip_suffix('|').unwrap_or(inner);
            (line, inner.split('|').map(str::trim).collect())
        })
        .collect();

    let Some(header_1) = rows
        .iter()
        .position(|(_, cells)| cells.first() == Some(&"NAME"))
    else {
        return Err("no queue stat header found".to_string());
    };
    let Some((_, header_2_cells)) = rows.get(header_1 + 1) else {
        return Err("no queue stat header found".to_string());
    };
    let header_1_cells = &rows[header_1].1;

    let check_header = |cells: &[&str], index: usize, expected: &str, which: &str| {
        let found = cells.get(index).copied().unwrap_or("<missing>");
        if found == expected {
            Ok(())
        } else {
            Err(format!(
                "unexpected queue stat header: {which} header line cell {index} is \
                 '{found}', expected '{expected}'"
            ))
        }
    };
    check_header(header_1_cells, 3, "MESSAGE", "first")?;
    check_header(header_1_cells, 4, "MESSAGES", "first")?;
    check_header(header_2_cells, 3, "COUNT", "second")?;
    check_header(header_2_cells, 4, "ADDED", "second")?;

    struct PendingRow {
        name: String,
        message_count: u64,
        messages_added: u64,
    }
    let mut pending: Option<PendingRow> = None;

    for (line, cells) in rows.iter().skip(header_1 + 2) {
        if cells.len() < 10 {
            return Err(format!("malformed queue stat row: {line}"));
        }

        // The pinned CLI caps column width near 25 chars and wraps longer
        // queue names across two physical rows: the first carries the
        // truncated name plus the counts, the next carries the name remainder
        // with empty count cells. The ADDRESS column wraps identically, but
        // only NAME identifies the queue.
        if cells[3].is_empty() && cells[4].is_empty() {
            let Some(row) = pending.as_mut() else {
                return Err(format!(
                    "malformed queue stat continuation row without a preceding numeric row: {line}"
                ));
            };
            row.name.push_str(cells[0]);
            if row.name == queue {
                return Ok(Some(QueueStat {
                    message_count: row.message_count,
                    messages_added: row.messages_added,
                }));
            }
            continue;
        }

        if cells[0] == queue {
            let message_count = cells[3].trim().parse::<u64>().map_err(|_| {
                format!(
                    "non-numeric queue stat MESSAGE COUNT cell '{}' in row: {line}",
                    cells[3]
                )
            })?;
            let messages_added = cells[4].trim().parse::<u64>().map_err(|_| {
                format!(
                    "non-numeric queue stat MESSAGES ADDED cell '{}' in row: {line}",
                    cells[4]
                )
            })?;
            return Ok(Some(QueueStat {
                message_count,
                messages_added,
            }));
        }

        // A numeric row for another queue may still be the first physical
        // segment of a wrapped name. It replaces any pending row: that one was
        // a complete row which did not match `queue`.
        if let (Ok(message_count), Ok(messages_added)) = (
            cells[3].trim().parse::<u64>(),
            cells[4].trim().parse::<u64>(),
        ) {
            pending = Some(PendingRow {
                name: cells[0].to_string(),
                message_count,
                messages_added,
            });
        }
    }

    Ok(None)
}

async fn wait_for_broker_port(port: u16, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        // Per-attempt connect bound (lintwiden D4.3): a timed-out attempt is
        // treated like any failed connect — the loop deadline still governs
        // the overall wait.
        let attempt = tokio::time::timeout(
            Duration::from_secs(5),
            TcpStream::connect(("127.0.0.1", port)),
        )
        .await
        .unwrap_or_else(|_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "tcp connect attempt exceeded 5s",
            ))
        });
        match attempt {
            Ok(stream) => {
                drop(stream);
                return Ok(());
            }
            Err(err) => {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "Artemis broker port {port} did not become reachable within {:?}: {err}",
                        timeout
                    ));
                }
                sleep(Duration::from_millis(250)).await;
            }
        }
    }
}

/// Shared Artemis container — started once and reused across all tests.
/// Uses `ANONYMOUS_LOGIN=true` for broad compatibility.
static ARTEMIS: OnceCell<(ContainerAsync<GenericImage>, String)> = OnceCell::const_new();

/// Shared Artemis container with **mandatory auth** (no ANONYMOUS_LOGIN).
/// Used to test that the bridge authenticates correctly and that the health
/// check does not time out under real auth conditions.
static ARTEMIS_AUTH: OnceCell<(ContainerAsync<GenericImage>, String)> = OnceCell::const_new();

/// Artemis startup is relatively heavy and can be flaky when two containers
/// (anonymous + mandatory-auth variants) initialize at the same time.
/// Serialize startup to reduce CI nondeterminism while keeping test execution
/// itself parallel.
static ARTEMIS_STARTUP_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Return a reference to the shared Artemis container and its broker URL.
pub async fn shared_artemis() -> &'static (ContainerAsync<GenericImage>, String) {
    ARTEMIS
        .get_or_init(|| async {
            let _startup_guard = ARTEMIS_STARTUP_LOCK.lock().await;
            let image = GenericImage::new("apache/activemq-artemis", "2.36.0-alpine")
                .with_exposed_port(ContainerPort::Tcp(ARTEMIS_PORT))
                .with_wait_for(WaitFor::message_on_stdout("AMQ221020"))
                .with_env_var("ARTEMIS_USER", ARTEMIS_USER)
                .with_env_var("ARTEMIS_PASSWORD", ARTEMIS_PASSWORD)
                .with_env_var("ANONYMOUS_LOGIN", "true")
                .with_startup_timeout(Duration::from_secs(120));

            let container = image
                .start()
                .await
                .expect("Artemis container failed to start");
            let port = container
                .get_host_port_ipv4(ARTEMIS_PORT)
                .await
                .expect("Artemis port not available");
            wait_for_broker_port(port, Duration::from_secs(30))
                .await
                .expect("Artemis broker did not become reachable");

            let broker_url = format!("tcp://127.0.0.1:{port}");
            eprintln!("Artemis ready at: {broker_url}");
            (container, broker_url)
        })
        .await
}

/// Return a reference to an Artemis container with **mandatory auth**.
/// ANONYMOUS_LOGIN is intentionally absent — the broker rejects unauthenticated
/// connections. Credentials: artemis / artemis.
pub async fn shared_artemis_auth() -> &'static (ContainerAsync<GenericImage>, String) {
    ARTEMIS_AUTH
        .get_or_init(|| async {
            let _startup_guard = ARTEMIS_STARTUP_LOCK.lock().await;
            let image = GenericImage::new("apache/activemq-artemis", "2.36.0-alpine")
                .with_exposed_port(ContainerPort::Tcp(ARTEMIS_PORT))
                .with_wait_for(WaitFor::message_on_stdout("AMQ221020"))
                .with_env_var("ARTEMIS_USER", ARTEMIS_USER)
                .with_env_var("ARTEMIS_PASSWORD", ARTEMIS_PASSWORD)
                // ANONYMOUS_LOGIN intentionally not set → mandatory auth
                .with_startup_timeout(Duration::from_secs(120));

            let container = image
                .start()
                .await
                .expect("Artemis auth container failed to start");
            let port = container
                .get_host_port_ipv4(ARTEMIS_PORT)
                .await
                .expect("Artemis auth port not available");
            wait_for_broker_port(port, Duration::from_secs(30))
                .await
                .expect("Artemis auth broker did not become reachable");

            let broker_url = format!("tcp://127.0.0.1:{port}");
            eprintln!("Artemis (auth) ready at: {broker_url}");
            (container, broker_url)
        })
        .await
}

/// A dedicated Artemis broker (its own container, mandatory auth).
///
/// Mirrors [`super::activemq::ActiveMqBroker`]: tests that must assert exact
/// broker-side/destination state for a single queue use an isolated broker so a
/// shared container's other connections cannot perturb the observation. The
/// fixture requires authentication (`ARTEMIS_USER`/`ARTEMIS_PASSWORD`), so the
/// bridge must present these credentials explicitly — there is no
/// `ANONYMOUS_LOGIN` fallback.
pub struct ArtemisBroker {
    // Held only to keep the container alive for the test's lifetime.
    _container: ContainerAsync<GenericImage>,
    pub broker_url: String,
}

impl ArtemisBroker {
    /// Docker container id, for freezing the broker with `docker pause`.
    pub fn container_id(&self) -> &str {
        self._container.id()
    }

    /// Reads the broker-side management counts for `queue` via `docker exec`.
    ///
    /// Mechanism: runs the dedicated container's own
    /// `/var/lib/artemis-instance/bin/artemis queue stat` CLI against the
    /// in-container broker and parses the pinned 2.36.0 table (see
    /// [`QueueStat`] and [`parse_queue_stat`]). Stability is pin-scoped to
    /// `apache/activemq-artemis:2.36.0-alpine`; the image deploys no console
    /// wars, so no Jolokia endpoint exists. The test-only `artemis`/`artemis`
    /// credentials are passed on the command line and are therefore visible in
    /// the container's `ps` output.
    ///
    /// A non-zero exit, a missing/damaged header pair, or no row for `queue`
    /// is an error — absence is never reported as zero counts. The counts are
    /// a point-in-time management snapshot; they do not prove journal layout,
    /// byte-level persistence, or crash safety.
    pub async fn queue_stat(&self, queue: &str) -> Result<QueueStat, String> {
        let container_id = self.container_id().to_owned();
        let queue = queue.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut command = std::process::Command::new("docker");
            command
                .arg("exec")
                .arg(&container_id)
                .arg("/var/lib/artemis-instance/bin/artemis")
                .arg("queue")
                .arg("stat")
                .arg("--url")
                .arg("tcp://localhost:61616")
                .arg("--user")
                .arg(ARTEMIS_USER)
                .arg("--password")
                .arg(ARTEMIS_PASSWORD)
                .arg("--queueName")
                .arg(&queue);

            let output = run_with_deadline(&mut command, Duration::from_secs(60))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let head: String = stderr.trim().chars().take(200).collect();
                return Err(format!(
                    "artemis queue stat failed for queue {queue}: {head}"
                ));
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            parse_queue_stat(&stdout, &queue)?
                .ok_or_else(|| format!("queue stat returned no row for queue {queue}"))
        })
        .await
        .map_err(|err| format!("queue stat blocking task failed: {err}"))?
    }

    /// Starts an isolated Artemis container and blocks until the broker port
    /// accepts connections.
    pub async fn start() -> Self {
        let image = GenericImage::new("apache/activemq-artemis", "2.36.0-alpine")
            .with_exposed_port(ContainerPort::Tcp(ARTEMIS_PORT))
            .with_wait_for(WaitFor::message_on_stdout("AMQ221020"))
            .with_env_var("ARTEMIS_USER", ARTEMIS_USER)
            .with_env_var("ARTEMIS_PASSWORD", ARTEMIS_PASSWORD)
            .with_startup_timeout(Duration::from_secs(120));

        let container = image
            .start()
            .await
            .expect("dedicated Artemis container failed to start");
        let port = container
            .get_host_port_ipv4(ARTEMIS_PORT)
            .await
            .expect("dedicated Artemis broker port not available");
        wait_for_broker_port(port, Duration::from_secs(30))
            .await
            .expect("dedicated Artemis broker did not become reachable");

        let broker_url = format!("tcp://127.0.0.1:{port}");
        eprintln!("dedicated Artemis ready at: {broker_url}");
        Self {
            _container: container,
            broker_url,
        }
    }
}

#[cfg(test)]
mod queue_stat_tests {
    use std::process::Command;
    use std::time::{Duration, Instant};

    use super::{QueueStat, parse_queue_stat, run_with_deadline};

    /// Verbatim `artemis queue stat --queueName DLQ` output observed on
    /// `apache/activemq-artemis:2.36.0-alpine`: one connection preamble line,
    /// the wrapped two-line header, and one data row.
    const PINNED_DLQ_OUTPUT: &str = r#"Connection brokerURL = tcp://localhost:61616
|NAME              |ADDRESS           |CONSUMER|MESSAGE|MESSAGES|DELIVERING|MESSAGES|SCHEDULED|ROUTING|INTERNAL|
|                  |                  | COUNT  | COUNT | ADDED  |  COUNT   | ACKED  |  COUNT  | TYPE  |        |
|DLQ               |DLQ               |   0    |   0   |   0    |    0     |   0    |    0    |ANYCAST|  false  |
"#;

    const HEADER_LINE_1: &str = "|NAME              |ADDRESS           |CONSUMER|MESSAGE|MESSAGES|DELIVERING|MESSAGES|SCHEDULED|ROUTING|INTERNAL|";
    const HEADER_LINE_2: &str = "|                  |                  | COUNT  | COUNT | ADDED  |  COUNT   | ACKED  |  COUNT  | TYPE  |        |";

    #[test]
    fn parse_queue_stat_reads_pinned_row() {
        assert_eq!(
            parse_queue_stat(PINNED_DLQ_OUTPUT, "DLQ"),
            Ok(Some(QueueStat {
                message_count: 0,
                messages_added: 0,
            })),
            "the pinned 2.36.0 row must parse to zero counts"
        );

        let synthetic = format!(
            "Connection brokerURL = tcp://localhost:61616\n{HEADER_LINE_1}\n{HEADER_LINE_2}\n\
             |DLQ               |DLQ               |   0    |   7   |   9    |    0     |   0    |    0    |ANYCAST|  false  |\n"
        );
        assert_eq!(
            parse_queue_stat(&synthetic, "DLQ"),
            Ok(Some(QueueStat {
                message_count: 7,
                messages_added: 9,
            })),
            "synthetic non-zero counts must land in the right fields"
        );
    }

    #[test]
    fn parse_queue_stat_absent_row_is_none() {
        let stdout = format!(
            "Connection brokerURL = tcp://localhost:61616\n{HEADER_LINE_1}\n{HEADER_LINE_2}\n"
        );
        assert_eq!(parse_queue_stat(&stdout, "DLQ"), Ok(None));
    }

    #[test]
    fn parse_queue_stat_merges_wrapped_name_rows() {
        // Verbatim shape observed on the pinned image: the CLI caps column
        // width near 25 chars, so a 41-char queue name is split across two
        // physical rows (counts only on the first, name remainder on the
        // second).
        let stdout = r#"Connection brokerURL = tcp://localhost:61616
|NAME                     |ADDRESS                  |CONSUMER|MESSAGE|MESSAGES|DELIVERING|MESSAGES|SCHEDULED|ROUTING|INTERNAL|
|                         |                         | COUNT  | COUNT | ADDED  |  COUNT   | ACKED  |  COUNT  | TYPE  |        |
|queue.test-redelivery-dis|queue.test-redelivery-dis|   0    |   2   |   5    |    0     |   0    |    0    |ANYCAST| false  |
|  position-artemis       |  position-artemis       |        |       |        |          |        |         |       |        |
"#;
        assert_eq!(
            parse_queue_stat(stdout, "queue.test-redelivery-disposition-artemis"),
            Ok(Some(QueueStat {
                message_count: 2,
                messages_added: 5,
            })),
            "a wrapped name must merge across physical rows"
        );
    }

    #[test]
    fn parse_queue_stat_orphan_continuation_is_err() {
        let stdout = format!(
            "Connection brokerURL = tcp://localhost:61616\n{HEADER_LINE_1}\n{HEADER_LINE_2}\n\
             |  position-artemis       |  position-artemis       |        |       |        |          |        |         |       |        |\n"
        );
        parse_queue_stat(&stdout, "queue.test-redelivery-disposition-artemis")
            .expect_err("a continuation row with no preceding numeric row must fail");
    }

    #[test]
    fn parse_queue_stat_empty_output_is_err() {
        let err = parse_queue_stat("", "DLQ").expect_err("empty output must never be Ok(None)");
        assert!(
            err.contains("no queue stat header found"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_queue_stat_missing_second_header_is_err() {
        let stdout = format!(
            "Connection brokerURL = tcp://localhost:61616\n{HEADER_LINE_1}\n\
             |DLQ               |DLQ               |   0    |   0   |   0    |    0     |   0    |    0    |ANYCAST|  false  |\n"
        );
        let err = parse_queue_stat(&stdout, "DLQ").expect_err("a truncated header pair must fail");
        assert!(err.contains("header"), "unexpected error: {err}");
        assert!(
            err.contains("COUNT"),
            "the error must name the missing second header qualifier: {err}"
        );
    }

    #[test]
    fn parse_queue_stat_swapped_columns_is_err() {
        let stdout = format!(
            "Connection brokerURL = tcp://localhost:61616\n\
             |NAME              |ADDRESS           |CONSUMER|MESSAGES|MESSAGE|DELIVERING|MESSAGES|SCHEDULED|ROUTING|INTERNAL|\n\
             {HEADER_LINE_2}\n\
             |DLQ               |DLQ               |   0    |   0   |   0    |    0     |   0    |    0    |ANYCAST|  false  |\n"
        );
        let err = parse_queue_stat(&stdout, "DLQ").expect_err("swapped columns must fail");
        assert!(
            err.contains("unexpected queue stat header"),
            "unexpected error: {err}"
        );
        assert!(
            err.contains("'MESSAGES'"),
            "the error must name the unexpected header cell: {err}"
        );
    }

    #[test]
    fn parse_queue_stat_malformed_row_is_err() {
        let stdout = format!(
            "Connection brokerURL = tcp://localhost:61616\n{HEADER_LINE_1}\n{HEADER_LINE_2}\n\
             |DLQ|DLQ|0|0|\n"
        );
        let err = parse_queue_stat(&stdout, "DLQ").expect_err("a short row must fail");
        assert!(
            err.contains("malformed queue stat row"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_queue_stat_non_numeric_count_is_err() {
        let stdout = format!(
            "Connection brokerURL = tcp://localhost:61616\n{HEADER_LINE_1}\n{HEADER_LINE_2}\n\
             |DLQ               |DLQ               |   0    | many  |   9    |    0     |   0    |    0    |ANYCAST|  false  |\n"
        );
        let err = parse_queue_stat(&stdout, "DLQ").expect_err("a non-numeric count must fail");
        assert!(err.contains("non-numeric"), "unexpected error: {err}");
        assert!(
            err.contains("many"),
            "the error must name the offending cell: {err}"
        );
    }

    #[test]
    fn run_with_deadline_kills_sleeping_child() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("sleep 30");

        let started = Instant::now();
        let err = run_with_deadline(&mut command, Duration::from_millis(300))
            .expect_err("a sleeping child must hit the deadline");
        let elapsed = started.elapsed();

        assert!(err.contains("deadline expired"), "unexpected error: {err}");
        assert!(
            elapsed < Duration::from_secs(10),
            "kill + reap took {elapsed:?}"
        );
    }

    #[test]
    fn run_with_deadline_returns_output_in_time() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("echo stat-ok");

        let output = run_with_deadline(&mut command, Duration::from_secs(5))
            .expect("a fast child must return its output");
        assert!(output.status.success(), "child status: {}", output.status);
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("stat-ok"),
            "stdout: {:?}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
