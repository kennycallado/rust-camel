//! Scripted fake SMTP server (test support only).

use std::sync::{Arc, Mutex};
use tokio_rustls::rustls;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

#[derive(Clone, Copy, Debug)]
pub enum SmtpBehavior {
    Accept,
    RejectRcpt550,
    DropAfterData,
}

#[derive(Clone)]
pub enum SmtpTls {
    None,
    StartTls(Arc<rustls::ServerConfig>),
    Implicit(Arc<rustls::ServerConfig>),
}

pub struct FakeSmtpOptions {
    pub behavior: SmtpBehavior,
    pub tls: SmtpTls,
}

#[derive(Clone, Default)]
pub struct SmtpRecording {
    pub connections: usize,
    pub commands: Vec<String>,
    pub data_commands: usize,
    pub messages: Vec<RecordedMessage>,
}

#[derive(Clone)]
pub struct RecordedMessage {
    pub mail_from: String,
    pub rcpt_to: Vec<String>,
    pub data: String,
}

impl RecordedMessage {
    /// Header value (unfolded) for `name`, matched ASCII-case-insensitively.
    pub fn header(&self, name: &str) -> Option<String> {
        let block = self
            .data
            .split("\r\n\r\n")
            .next()
            .and_then(|b| b.split("\n\n").next())
            .unwrap_or("");
        let mut headers: Vec<String> = Vec::new();
        for line in block.lines() {
            if line.starts_with([' ', '\t']) {
                if let Some(last) = headers.last_mut() {
                    last.push(' ');
                    last.push_str(line.trim());
                }
            } else {
                headers.push(line.to_string());
            }
        }
        headers.iter().find_map(|h| {
            let (n, v) = h.split_once(':')?;
            n.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_string())
        })
    }
}

pub struct FakeSmtpServer {
    pub port: u16,
    recording: Arc<Mutex<SmtpRecording>>,
    task: tokio::task::JoinHandle<()>,
}

impl FakeSmtpServer {
    pub async fn start(opts: FakeSmtpOptions) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let recording = Arc::new(Mutex::new(SmtpRecording::default()));
        let rec = recording.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                lock(&rec).connections += 1;
                let rec = rec.clone();
                let behavior = opts.behavior;
                let tls = opts.tls.clone();
                tokio::spawn(async move {
                    let _ = serve(stream, behavior, tls, rec).await;
                });
            }
        });
        FakeSmtpServer {
            port,
            recording,
            task,
        }
    }

    pub fn recording(&self) -> SmtpRecording {
        lock(&self.recording).clone()
    }
}

impl Drop for FakeSmtpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn lock(m: &Mutex<SmtpRecording>) -> std::sync::MutexGuard<'_, SmtpRecording> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

type Io = std::io::Result<()>;

async fn serve(
    stream: TcpStream,
    behavior: SmtpBehavior,
    tls: SmtpTls,
    rec: Arc<Mutex<SmtpRecording>>,
) -> Io {
    match tls {
        SmtpTls::None => {
            session(BufReader::new(stream), behavior, None, true, &rec).await?;
        }
        SmtpTls::Implicit(cfg) => {
            let s = TlsAcceptor::from(cfg).accept(stream).await?;
            session(BufReader::new(s), behavior, None, true, &rec).await?;
        }
        SmtpTls::StartTls(cfg) => {
            let upgraded = session(
                BufReader::new(stream),
                behavior,
                Some(cfg.clone()),
                true,
                &rec,
            )
            .await?;
            if let Some(raw) = upgraded {
                let s = TlsAcceptor::from(cfg).accept(raw).await?;
                // greeting is not repeated after STARTTLS
                session(BufReader::new(s), behavior, None, false, &rec).await?;
            }
        }
    }
    Ok(())
}

/// Runs the command loop. Returns the raw stream when a STARTTLS upgrade
/// was accepted (only possible when `starttls` is `Some`).
async fn session<S>(
    mut io: BufReader<S>,
    behavior: SmtpBehavior,
    starttls: Option<Arc<rustls::ServerConfig>>,
    greet: bool,
    rec: &Arc<Mutex<SmtpRecording>>,
) -> std::io::Result<Option<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if greet {
        reply(&mut io, "220 fake.test ESMTP ready\r\n").await?;
    }
    let mut mail_from = String::new();
    let mut rcpt_to: Vec<String> = Vec::new();
    let mut msg_no = 0usize;
    let mut line = String::new();
    loop {
        // allow-test-wait: client reads and reply writes in the loop are each bounded by timeout(IDLE)
        line.clear();
        if tokio::time::timeout(IDLE, read_line(&mut io, &mut line))
            .await
            .map_err(|_| std::io::ErrorKind::TimedOut)??
            == 0
        {
            return Ok(None);
        }
        let cmd = line.trim_end_matches(['\r', '\n']).to_string();
        lock(rec).commands.push(cmd.clone());
        let upper = cmd.to_ascii_uppercase();
        let verb = upper.split_whitespace().next().unwrap_or("");
        if verb == "EHLO" {
            let mut r = String::from("250-fake.test\r\n250-AUTH PLAIN LOGIN\r\n250-8BITMIME\r\n");
            if starttls.is_some() {
                r.push_str("250-STARTTLS\r\n");
            }
            r.push_str("250 SMTPUTF8\r\n");
            reply(&mut io, &r).await?;
        } else if verb == "HELO" {
            reply(&mut io, "250 fake.test\r\n").await?;
        } else if upper == "STARTTLS" {
            if starttls.is_some() {
                reply(&mut io, "220 2.0.0 ready\r\n").await?;
                return Ok(Some(io.into_inner()));
            }
            reply(&mut io, "454 4.7.0 TLS not available\r\n").await?;
        } else if upper.starts_with("AUTH PLAIN") {
            reply(&mut io, "235 2.7.0 authenticated\r\n").await?;
        } else if upper.starts_with("AUTH LOGIN") {
            reply(&mut io, "334 VXNlcm5hbWU6\r\n").await?;
            line.clear();
            if tokio::time::timeout(IDLE, read_line(&mut io, &mut line))
                .await
                .map_err(|_| std::io::ErrorKind::TimedOut)??
                == 0
            {
                return Ok(None);
            }
            lock(rec)
                .commands
                .push(line.trim_end_matches(['\r', '\n']).to_string());
            reply(&mut io, "334 UGFzc3dvcmQ6\r\n").await?;
            line.clear();
            if tokio::time::timeout(IDLE, read_line(&mut io, &mut line))
                .await
                .map_err(|_| std::io::ErrorKind::TimedOut)??
                == 0
            {
                return Ok(None);
            }
            lock(rec)
                .commands
                .push(line.trim_end_matches(['\r', '\n']).to_string());
            reply(&mut io, "235 2.7.0 authenticated\r\n").await?;
        } else if upper.starts_with("MAIL FROM:") {
            mail_from = addr(&cmd["MAIL FROM:".len()..]);
            rcpt_to.clear();
            reply(&mut io, "250 2.1.0 ok\r\n").await?;
        } else if upper.starts_with("RCPT TO:") {
            if matches!(behavior, SmtpBehavior::RejectRcpt550) {
                reply(&mut io, "550 5.1.1 mailbox unavailable\r\n").await?;
            } else {
                rcpt_to.push(addr(&cmd["RCPT TO:".len()..]));
                reply(&mut io, "250 2.1.5 ok\r\n").await?;
            }
        } else if upper == "DATA" {
            lock(rec).data_commands += 1;
            reply(&mut io, "354 end data with <CR><LF>.<CR><LF>\r\n").await?;
            let mut data = String::new();
            loop {
                line.clear();
                if tokio::time::timeout(IDLE, read_line(&mut io, &mut line))
                    .await
                    .map_err(|_| std::io::ErrorKind::TimedOut)??
                    == 0
                {
                    return Ok(None);
                }
                let l = line.trim_end_matches(['\r', '\n']);
                if l == "." {
                    break;
                }
                data.push_str(
                    l.strip_prefix('.')
                        .filter(|_| l.starts_with(".."))
                        .unwrap_or(l),
                );
                data.push_str("\r\n");
            }
            msg_no += 1;
            lock(rec).messages.push(RecordedMessage {
                mail_from: mail_from.clone(),
                rcpt_to: std::mem::take(&mut rcpt_to),
                data,
            });
            if matches!(behavior, SmtpBehavior::DropAfterData) {
                return Ok(None);
            }
            reply(&mut io, &format!("250 2.0.0 queued as FAKE{msg_no}\r\n")).await?;
        } else if upper == "RSET" || upper == "NOOP" {
            reply(&mut io, "250 2.0.0 ok\r\n").await?;
        } else if upper == "QUIT" {
            reply(&mut io, "221 2.0.0 bye\r\n").await?;
            return Ok(None);
        } else {
            reply(&mut io, "502 5.5.2 command not recognized\r\n").await?;
        }
    }
}

async fn reply<S: AsyncRead + AsyncWrite + Unpin>(io: &mut BufReader<S>, s: &str) -> Io {
    tokio::time::timeout(IDLE, async {
        io.get_mut().write_all(s.as_bytes()).await?;
        io.get_mut().flush().await
    })
    .await
    .map_err(|_| std::io::ErrorKind::TimedOut)?
}

/// Extracts `addr` from `<addr>[ params]`.
fn addr(rest: &str) -> String {
    let rest = rest.trim();
    match (rest.find('<'), rest.find('>')) {
        (Some(a), Some(b)) if b > a => rest[a + 1..b].to_string(),
        _ => rest.split_whitespace().next().unwrap_or("").to_string(),
    }
}

/// Per-iteration deadline: a stalled client ends the session instead of hanging.
const IDLE: std::time::Duration = std::time::Duration::from_secs(30);

/// Line read; callers wrap it in `timeout(IDLE, ..)`.
async fn read_line<S: AsyncRead + Unpin>(
    io: &mut BufReader<S>,
    buf: &mut String,
) -> std::io::Result<usize> {
    io.read_line(buf).await
}
