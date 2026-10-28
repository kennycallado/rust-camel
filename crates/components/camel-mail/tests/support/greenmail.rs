//! GreenMail docker fixture (plain SMTP + IMAP, one fixture user).

use std::sync::Arc;
use std::time::{Duration, Instant};

use camel_api::{Exchange, Message};
use camel_component_api::{
    BoxProcessor, Component, NoOpComponentContext, NoopRuntimeObservability, ProducerContext,
};
use camel_component_mail::{MailComponent, MailComponentConfig, MailScheme};
use futures::TryStreamExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tower::Service;

use super::docker::{docker_exec, nanos};

pub const GREENMAIL_IMAGE: &str = "greenmail/standalone:2.1.2";
const READY_BOUND: Duration = Duration::from_secs(60);
const ATTEMPT_BOUND: Duration = Duration::from_secs(5);
const IO_BOUND: Duration = Duration::from_secs(15);

pub struct GreenMailFixture {
    container: String,
    pub smtp_port: u16,
    pub imap_port: u16,
}

fn mapped_port(container: &str, spec: &str) -> u16 {
    let out = docker_exec(&["port", container, spec]).unwrap_or_else(|e| {
        let _ = docker_exec(&["rm", "-f", container]);
        panic!("infra-unavailable: docker port failed: {e}");
    });
    out.lines()
        .filter_map(|l| l.rsplit(':').next())
        .find_map(|p| p.trim().parse::<u16>().ok())
        .unwrap_or_else(|| {
            let _ = docker_exec(&["rm", "-f", container]);
            panic!("infra-unavailable: cannot parse mapped port from {out:?}");
        })
}

/// One attempt: connect and read the greeting line.
async fn imap_greeting_ok(port: u16) -> bool {
    let Ok(Ok(stream)) = timeout(ATTEMPT_BOUND, TcpStream::connect(("127.0.0.1", port))).await
    else {
        return false;
    };
    let mut line = String::new();
    let mut reader = BufReader::new(stream);
    matches!(
        timeout(ATTEMPT_BOUND, reader.read_line(&mut line)).await,
        Ok(Ok(n)) if n > 0 && line.starts_with("* OK")
    )
}

impl GreenMailFixture {
    pub async fn start() -> Self {
        let name = format!("greenmail-itest-{}-{}", std::process::id(), nanos());
        // FIXTURE-ONLY login `user`/`pass`.
        let opts = "-Dgreenmail.setup.test.smtp -Dgreenmail.setup.test.imap \
                    -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.users=user:pass@local.test";
        let env = format!("GREENMAIL_OPTS={opts}");
        if let Err(e) = docker_exec(&[
            "run",
            "-d",
            "--name",
            &name,
            "-p",
            "127.0.0.1::3025",
            "-p",
            "127.0.0.1::3143",
            "-e",
            &env,
            GREENMAIL_IMAGE,
        ]) {
            let _ = docker_exec(&["rm", "-f", &name]);
            panic!("infra-unavailable: docker run failed: {e}");
        }
        let smtp_port = mapped_port(&name, "3025/tcp");
        let imap_port = mapped_port(&name, "3143/tcp");
        // From here the Drop impl owns cleanup.
        let fx = Self {
            container: name,
            smtp_port,
            imap_port,
        };
        let deadline = Instant::now() + READY_BOUND;
        loop {
            if imap_greeting_ok(fx.imap_port).await {
                break;
            }
            if Instant::now() >= deadline {
                panic!("infra-unavailable: greenmail not ready within 60s");
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        fx
    }

    /// Deliver one message to `user@local.test` through the crate's SMTP producer.
    pub async fn seed(&self, subject: &str) {
        let cfg: MailComponentConfig = toml::from_str("").expect("empty mail config");
        let uri = format!(
            "smtp://127.0.0.1:{}?from=sender@local.test&to=user@local.test&subject={subject}&allowCleartext=true",
            self.smtp_port
        );
        let component = MailComponent::new(MailScheme::Smtp, Arc::new(cfg));
        let endpoint = component
            .create_endpoint(&uri, &NoOpComponentContext)
            .expect("smtp endpoint");
        let mut p: BoxProcessor = endpoint
            .create_producer(
                Arc::new(NoopRuntimeObservability),
                &ProducerContext::default(),
            )
            .expect("producer");
        timeout(IO_BOUND, p.call(Exchange::new(Message::new("hello"))))
            .await
            .expect("seed timeout")
            .expect("seed send");
    }

    async fn login(&self) -> async_imap::Session<TcpStream> {
        let stream = timeout(IO_BOUND, TcpStream::connect(("127.0.0.1", self.imap_port)))
            .await
            .expect("imap connect timeout")
            .expect("imap connect");
        let client = async_imap::Client::new(stream);
        timeout(IO_BOUND, client.login("user", "pass"))
            .await
            .expect("login timeout")
            .map_err(|(e, _)| e)
            .expect("login")
    }

    /// `(uidplus, move)` as advertised by the server.
    pub async fn capabilities(&self) -> (bool, bool) {
        let mut session = self.login().await;
        let caps = timeout(IO_BOUND, session.capabilities())
            .await
            .expect("capability timeout")
            .expect("capability");
        let out = (caps.has_str("UIDPLUS"), caps.has_str("MOVE"));
        let _ = timeout(IO_BOUND, session.logout()).await;
        out
    }

    pub async fn create_folder(&self, name: &str) {
        let mut session = self.login().await;
        timeout(IO_BOUND, session.create(name))
            .await
            .expect("create timeout")
            .expect("create");
        let _ = timeout(IO_BOUND, session.logout()).await;
    }

    /// Number of messages in `folder` whose subject matches `subject`.
    pub async fn count_subject(&self, folder: &str, subject: &str) -> usize {
        let mut session = self.login().await;
        timeout(IO_BOUND, session.select(folder))
            .await
            .expect("select timeout")
            .expect("select");
        let query = format!("SUBJECT \"{subject}\"");
        let uids = timeout(IO_BOUND, session.uid_search(&query))
            .await
            .expect("search timeout")
            .expect("search");
        let _ = timeout(IO_BOUND, session.logout()).await;
        uids.len()
    }

    /// Poll `count_subject` until it equals `expected` (bounded); returns the
    /// last observed count. Disposition runs after the route reply, so tests
    /// wait for it before cancelling the consumer.
    pub async fn wait_count(&self, folder: &str, subject: &str, expected: usize) -> usize {
        let deadline = Instant::now() + READY_BOUND / 4;
        loop {
            let n = self.count_subject(folder, subject).await;
            if n == expected || Instant::now() >= deadline {
                return n;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Poll `seen_flags` until `Some(true)` (bounded); returns the last value.
    pub async fn wait_seen(&self, subject: &str) -> Option<bool> {
        let deadline = Instant::now() + READY_BOUND / 4;
        loop {
            let v = self.seen_flags(subject).await;
            if v == Some(true) || Instant::now() >= deadline {
                return v;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// `Some(true)` when the message with `subject` carries `\Seen`;
    /// `None` when no such message exists.
    pub async fn seen_flags(&self, subject: &str) -> Option<bool> {
        let stream = timeout(IO_BOUND, TcpStream::connect(("127.0.0.1", self.imap_port)))
            .await
            .expect("imap connect timeout")
            .expect("imap connect");
        let client = async_imap::Client::new(stream);
        let mut session = timeout(IO_BOUND, client.login("user", "pass"))
            .await
            .expect("login timeout")
            .map_err(|(e, _)| e)
            .expect("login");
        timeout(IO_BOUND, session.select("INBOX"))
            .await
            .expect("select timeout")
            .expect("select");
        let query = format!("SUBJECT \"{subject}\"");
        let uids = timeout(IO_BOUND, session.uid_search(&query))
            .await
            .expect("search timeout")
            .expect("search");
        let result = match uids.iter().next() {
            None => None,
            Some(uid) => {
                let fetched: Vec<_> = timeout(IO_BOUND, async {
                    session
                        .uid_fetch(uid.to_string(), "FLAGS")
                        .await?
                        .try_collect()
                        .await
                })
                .await
                .expect("fetch timeout")
                .expect("fetch");
                fetched.first().map(|f| {
                    f.flags()
                        .any(|fl| matches!(fl, async_imap::types::Flag::Seen))
                })
            }
        };
        let _ = timeout(IO_BOUND, session.logout()).await;
        result
    }
}

impl Drop for GreenMailFixture {
    fn drop(&mut self) {
        if let Err(e) = docker_exec(&["rm", "-f", &self.container]) {
            eprintln!("greenmail fixture cleanup failed ({}): {e}", self.container);
        }
    }
}
