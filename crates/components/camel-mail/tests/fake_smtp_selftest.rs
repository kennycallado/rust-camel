mod support;

use std::time::Duration;

use support::fake_smtp::{FakeSmtpOptions, FakeSmtpServer, SmtpBehavior, SmtpTls};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::time::timeout;

const BOUND: Duration = Duration::from_secs(5);

struct Client {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

impl Client {
    async fn connect(port: u16) -> Self {
        let stream = timeout(BOUND, TcpStream::connect(("127.0.0.1", port)))
            .await
            .expect("connect timeout")
            .expect("connect");
        let (r, w) = stream.into_split();
        Client {
            reader: BufReader::new(r),
            writer: w,
        }
    }

    async fn line(&mut self) -> String {
        let mut s = String::new();
        timeout(BOUND, self.reader.read_line(&mut s))
            .await
            .expect("read timeout")
            .expect("read");
        s
    }

    async fn send(&mut self, cmd: &str) {
        let data = format!("{cmd}\r\n");
        timeout(BOUND, self.writer.write_all(data.as_bytes()))
            .await
            .expect("write timeout")
            .expect("write");
    }

    /// Sends EHLO and returns all reply lines up to the final one.
    async fn ehlo(&mut self) -> Vec<String> {
        self.send("EHLO t").await;
        let mut lines = Vec::new();
        loop {
            let l = self.line().await;
            let done = l.len() >= 4 && l.as_bytes()[3] == b' ';
            lines.push(l);
            if done || lines.len() > 20 {
                return lines;
            }
        }
    }
}

fn opts(behavior: SmtpBehavior) -> FakeSmtpOptions {
    FakeSmtpOptions {
        behavior,
        tls: SmtpTls::None,
    }
}

#[tokio::test]
async fn fake_smtp_accepts_one_message() {
    let server = FakeSmtpServer::start(opts(SmtpBehavior::Accept)).await;
    let mut c = Client::connect(server.port).await;
    assert!(c.line().await.starts_with("220"));
    let ehlo = c.ehlo().await;
    assert_eq!(ehlo.last().map(String::as_str), Some("250 SMTPUTF8\r\n"));
    c.send("MAIL FROM:<a@x.test>").await;
    assert!(c.line().await.starts_with("250"));
    c.send("RCPT TO:<b@x.test>").await;
    assert!(c.line().await.starts_with("250"));
    c.send("DATA").await;
    assert!(c.line().await.starts_with("354"));
    for l in ["Subject: s", "", "body", "."] {
        c.send(l).await;
    }
    assert!(c.line().await.starts_with("250 2.0.0 queued as FAKE1"));
    c.send("QUIT").await;
    assert!(c.line().await.starts_with("221"));

    let rec = server.recording();
    assert_eq!(rec.connections, 1);
    assert_eq!(rec.data_commands, 1);
    assert_eq!(rec.messages.len(), 1);
    let m = &rec.messages[0];
    assert_eq!(m.mail_from, "a@x.test");
    assert_eq!(m.rcpt_to, vec!["b@x.test".to_string()]);
    assert_eq!(m.header("Subject"), Some("s".to_string()));
}

#[tokio::test]
async fn fake_smtp_rejects_rcpt_550() {
    let server = FakeSmtpServer::start(opts(SmtpBehavior::RejectRcpt550)).await;
    let mut c = Client::connect(server.port).await;
    c.line().await;
    c.ehlo().await;
    c.send("MAIL FROM:<a@x.test>").await;
    c.line().await;
    c.send("RCPT TO:<b@x.test>").await;
    assert!(c.line().await.starts_with("550"));
}

#[tokio::test]
async fn fake_smtp_drops_after_data() {
    let server = FakeSmtpServer::start(opts(SmtpBehavior::DropAfterData)).await;
    let mut c = Client::connect(server.port).await;
    c.line().await;
    c.ehlo().await;
    c.send("MAIL FROM:<a@x.test>").await;
    c.line().await;
    c.send("RCPT TO:<b@x.test>").await;
    c.line().await;
    c.send("DATA").await;
    assert!(c.line().await.starts_with("354"));
    for l in ["Subject: s", "", "body", "."] {
        c.send(l).await;
    }
    assert_eq!(c.line().await, "");
    assert_eq!(server.recording().data_commands, 1);
}

#[tokio::test]
async fn fake_smtp_unknown_command() {
    let server = FakeSmtpServer::start(opts(SmtpBehavior::Accept)).await;
    let mut c = Client::connect(server.port).await;
    c.line().await;
    c.send("FOO").await;
    assert!(c.line().await.starts_with("502"));
}

#[tokio::test]
async fn fake_smtp_advertises_starttls_only_when_configured() {
    let server = FakeSmtpServer::start(opts(SmtpBehavior::Accept)).await;
    let mut c = Client::connect(server.port).await;
    c.line().await;
    let ehlo = c.ehlo().await;
    assert!(ehlo.iter().all(|l| !l.contains("STARTTLS")));
}
