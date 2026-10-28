//! Scripted fake IMAP server (test support only).

use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_rustls::rustls;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

pub const PROBE_EML: &[u8] =
    b"From: a@corp.test\r\nSubject: probe\r\nMessage-ID: <p1@corp.test>\r\n\r\nhello\r\n";

/// How the fake answers `LOGIN`.
#[derive(Clone, Copy)]
pub enum Login {
    Accept,
    /// Tagged `NO`, as for bad credentials.
    Refuse,
}

#[derive(Clone)]
pub enum ImapTls {
    None,
    Implicit(Arc<rustls::ServerConfig>),
    StartTls(Arc<rustls::ServerConfig>),
}

pub struct FakeImapServer {
    pub port: u16,
    lines: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

/// Per-iteration deadline: a stalled client ends the session instead of hanging.
const IDLE: Duration = Duration::from_secs(30);

impl FakeImapServer {
    pub async fn start(tls: ImapTls, capabilities: &str, message: Vec<u8>) -> Self {
        Self::start_with(tls, capabilities, message, Login::Accept).await
    }

    /// Like [`FakeImapServer::start`], answering `LOGIN` as `login` says.
    pub async fn start_with(
        tls: ImapTls,
        capabilities: &str,
        message: Vec<u8>,
        login: Login,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let rec = lines.clone();
        let caps = capabilities.to_string();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (rec, tls, caps, message) =
                    (rec.clone(), tls.clone(), caps.clone(), message.clone());
                tokio::spawn(async move {
                    let _ = serve(stream, tls, &caps, &message, login, rec).await;
                });
            }
        });
        FakeImapServer { port, lines, task }
    }

    pub fn lines(&self) -> Vec<String> {
        lock(&self.lines).clone()
    }
}

impl Drop for FakeImapServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn lock(m: &Mutex<Vec<String>>) -> std::sync::MutexGuard<'_, Vec<String>> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

type Io = std::io::Result<()>;

async fn serve(
    stream: TcpStream,
    tls: ImapTls,
    caps: &str,
    message: &[u8],
    login: Login,
    rec: Arc<Mutex<Vec<String>>>,
) -> Io {
    let script = Script {
        caps,
        message,
        login,
        rec: &rec,
    };
    match tls {
        ImapTls::None => {
            session(BufReader::new(stream), false, true, &script).await?;
        }
        ImapTls::Implicit(cfg) => {
            let s = TlsAcceptor::from(cfg).accept(stream).await?;
            session(BufReader::new(s), false, true, &script).await?;
        }
        ImapTls::StartTls(cfg) => {
            let upgraded = session(BufReader::new(stream), true, true, &script).await?;
            if let Some(raw) = upgraded {
                let s = TlsAcceptor::from(cfg).accept(raw).await?;
                // no second greeting after STARTTLS
                session(BufReader::new(s), false, false, &script).await?;
            }
        }
    }
    Ok(())
}

/// What one connection answers.
struct Script<'a> {
    caps: &'a str,
    message: &'a [u8],
    login: Login,
    rec: &'a Arc<Mutex<Vec<String>>>,
}

/// Runs the command loop. Returns the raw stream when a STARTTLS upgrade
/// was accepted (only possible when `starttls` is true).
async fn session<S>(
    mut io: BufReader<S>,
    starttls: bool,
    greet: bool,
    script: &Script<'_>,
) -> std::io::Result<Option<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Script {
        caps,
        message,
        login,
        rec,
    } = *script;
    if greet {
        write(
            &mut io,
            format!("* OK [CAPABILITY IMAP4rev1 {caps}] fake ready\r\n").as_bytes(),
        )
        .await?;
    }
    let mut line = String::new();
    loop {
        // allow-test-wait: client reads and reply writes in the loop are each bounded by timeout(IDLE)
        line.clear();
        if tokio::time::timeout(IDLE, io.read_line(&mut line))
            .await
            .map_err(|_| std::io::ErrorKind::TimedOut)??
            == 0
        {
            return Ok(None);
        }
        let cmd = line.trim_end_matches(['\r', '\n']).to_string();
        lock(rec).push(cmd.clone());
        let mut parts = cmd.splitn(2, ' ');
        let tag = parts.next().unwrap_or("").to_string();
        let rest = parts.next().unwrap_or("");
        let upper = rest.to_ascii_uppercase();
        if upper == "CAPABILITY" {
            let r = format!("* CAPABILITY IMAP4rev1 {caps}\r\n{tag} OK CAPABILITY completed\r\n");
            write(&mut io, r.as_bytes()).await?;
        } else if upper == "STARTTLS" {
            if starttls {
                write(&mut io, format!("{tag} OK begin TLS\r\n").as_bytes()).await?;
                return Ok(Some(io.into_inner()));
            }
            write(
                &mut io,
                format!("{tag} BAD STARTTLS unavailable\r\n").as_bytes(),
            )
            .await?;
        } else if upper.starts_with("LOGIN ") {
            let reply = match login {
                Login::Accept => format!("{tag} OK LOGIN completed\r\n"),
                Login::Refuse => format!("{tag} NO [AUTHENTICATIONFAILED] invalid credentials\r\n"),
            };
            write(&mut io, reply.as_bytes()).await?;
        } else if upper.starts_with("SELECT ") {
            let r = format!(
                "* FLAGS (\\Seen \\Deleted)\r\n* 1 EXISTS\r\n* OK [UIDVALIDITY 7] ok\r\n{tag} OK [READ-WRITE] SELECT completed\r\n"
            );
            write(&mut io, r.as_bytes()).await?;
        } else if upper == "UID SEARCH UNSEEN" {
            let r = format!("* SEARCH 42\r\n{tag} OK SEARCH completed\r\n");
            write(&mut io, r.as_bytes()).await?;
        } else if upper.starts_with("UID FETCH 42 ") && upper.contains("RFC822.SIZE") {
            let r = format!(
                "* 1 FETCH (UID 42 RFC822.SIZE {})\r\n{tag} OK FETCH completed\r\n",
                message.len()
            );
            write(&mut io, r.as_bytes()).await?;
        } else if upper.starts_with("UID FETCH 42 ") && upper.contains("BODY.PEEK[]") {
            let mut r = format!("* 1 FETCH (UID 42 BODY[] {{{}}}\r\n", message.len()).into_bytes();
            r.extend_from_slice(message);
            r.extend_from_slice(format!(")\r\n{tag} OK FETCH completed\r\n").as_bytes());
            write(&mut io, &r).await?;
        } else if upper.starts_with("UID COPY 42 ") {
            write(&mut io, format!("{tag} OK COPY completed\r\n").as_bytes()).await?;
        } else if upper.starts_with("UID MOVE 42 ") {
            let r = format!("* 1 EXPUNGE\r\n{tag} OK MOVE completed\r\n");
            write(&mut io, r.as_bytes()).await?;
        } else if upper.starts_with("UID STORE 42 +FLAGS.SILENT") {
            write(&mut io, format!("{tag} OK STORE completed\r\n").as_bytes()).await?;
        } else if upper == "UID EXPUNGE 42" {
            let r = format!("* 1 EXPUNGE\r\n{tag} OK EXPUNGE completed\r\n");
            write(&mut io, r.as_bytes()).await?;
        } else if upper == "EXPUNGE" {
            // recorded above; answered so a test can prove it never happens
            write(
                &mut io,
                format!("{tag} OK EXPUNGE completed\r\n").as_bytes(),
            )
            .await?;
        } else if upper.starts_with("UID STORE 42 +FLAGS") {
            let r = format!("* 1 FETCH (UID 42 FLAGS (\\Seen))\r\n{tag} OK STORE completed\r\n");
            write(&mut io, r.as_bytes()).await?;
        } else if upper == "LOGOUT" {
            let r = format!("* BYE bye\r\n{tag} OK LOGOUT completed\r\n");
            write(&mut io, r.as_bytes()).await?;
            return Ok(None);
        } else {
            write(&mut io, format!("{tag} BAD unknown command\r\n").as_bytes()).await?;
        }
    }
}

async fn write<S: AsyncRead + AsyncWrite + Unpin>(io: &mut BufReader<S>, b: &[u8]) -> Io {
    tokio::time::timeout(IDLE, async {
        io.get_mut().write_all(b).await?;
        io.get_mut().flush().await
    })
    .await
    .map_err(|_| std::io::ErrorKind::TimedOut)?
}
