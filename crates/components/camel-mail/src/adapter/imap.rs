//! async-imap-backed `MailStore`. Third-party types stay in this module.

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio_rustls::rustls;

use async_imap::imap_proto::{Capability as ProtoCapability, Response, Status};
use async_trait::async_trait;
use futures::TryStreamExt;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::config::{
    ImapEndpointConfig, MailAccountConfig, MailServerAddress, SecretString, TlsMode,
};
use crate::error;
use crate::error::MailError;
use crate::store::{MailStore, MailStoreConnector, MessageSize, SelectedFolder, StoreCapabilities};
use crate::tls;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const IO_TIMEOUT: Duration = Duration::from_secs(60);
/// Tag for the pre-login CAPABILITY probe; never collides with the
/// library's own generated tags.
const PROBE_TAG: &str = "CAPPROBE";

/// Plain or TLS-wrapped TCP stream.
#[derive(Debug)]
enum ImapStream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl AsyncRead for ImapStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for ImapStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_flush(cx),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Login for an IMAP session. `Debug` never shows the password.
pub(crate) struct ImapLogin {
    pub(crate) username: String,
    pub(crate) password: SecretString,
}

impl fmt::Debug for ImapLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImapLogin")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Opens authenticated async-imap sessions.
pub(crate) struct AsyncImapConnector {
    server: MailServerAddress,
    tls: TlsMode,
    login: ImapLogin,
    tls_config: Arc<rustls::ClientConfig>,
    connect_timeout: Duration,
    io_timeout: Duration,
}

impl AsyncImapConnector {
    pub(crate) fn new(
        server: MailServerAddress,
        tls: TlsMode,
        login: ImapLogin,
        tls_config: Arc<rustls::ClientConfig>,
    ) -> Self {
        Self {
            server,
            tls,
            login,
            tls_config,
            connect_timeout: CONNECT_TIMEOUT,
            io_timeout: IO_TIMEOUT,
        }
    }

    /// Override the whole-connect and per-operation deadlines.
    #[cfg(test)]
    pub(crate) fn with_timeouts(mut self, connect: Duration, io: Duration) -> Self {
        self.connect_timeout = connect;
        self.io_timeout = io;
        self
    }

    pub(crate) fn from_endpoint(
        cfg: &ImapEndpointConfig,
        account: &MailAccountConfig,
    ) -> Result<Self, MailError> {
        let (Some(username), Some(password)) = (&account.username, &account.password) else {
            return Err(MailError::Config(
                "imap endpoints need an account with username and password in [components.mail.accounts]"
                    .to_string(),
            ));
        };
        let roots = match &account.ca_file {
            Some(path) => tls::load_ca_file(path)?,
            None => Vec::new(),
        };
        Ok(Self::new(
            cfg.server.clone(),
            cfg.tls,
            ImapLogin {
                username: username.clone(),
                password: password.clone(),
            },
            tls::client_config(&roots)?,
        ))
    }

    async fn tls_wrap(&self, tcp: TcpStream) -> Result<ImapStream, MailError> {
        let name = ServerName::try_from(self.server.host.clone())
            .map_err(|e| MailError::Tls(format!("invalid server name: {e}")))?;
        let tls = TlsConnector::from(self.tls_config.clone())
            .connect(name, tcp)
            .await
            .map_err(|e| {
                MailError::Connect(format!(
                    "TLS handshake failed: {}",
                    error::server_text(&e.to_string())
                ))
            })?;
        Ok(ImapStream::Tls(Box::new(tls)))
    }
}

fn connect_err(e: impl fmt::Display) -> MailError {
    MailError::Connect(error::server_text(&e.to_string()))
}

/// Bounded, clean text for an async-imap error. A parse error carries the
/// raw response buffer, so it is reported by a constant instead.
fn describe(e: &async_imap::error::Error) -> String {
    match e {
        async_imap::error::Error::Parse(_) => "unparsable IMAP response".to_string(),
        other => error::server_text(&other.to_string()),
    }
}

type RawClient = async_imap::Client<ImapStream>;

async fn read_greeting(client: &mut RawClient) -> Result<(), MailError> {
    match client.read_response().await.map_err(connect_err)? {
        Some(r) => match r.parsed() {
            Response::Data {
                status: Status::Ok, ..
            } => Ok(()),
            _ => Err(MailError::Connect("unexpected IMAP greeting".to_string())),
        },
        None => Err(MailError::Connect(
            "connection closed before IMAP greeting".to_string(),
        )),
    }
}

/// Send `CAPABILITY` before login and return the advertised atoms as a
/// synthetic `* CAPABILITY ...` line. async-imap's `Client` exposes no
/// public way to read command data pre-login, so the command is written to
/// the raw stream and the replies are read through the client.
async fn probe_capabilities(client: &mut RawClient) -> Result<Vec<u8>, MailError> {
    let io = client.get_mut();
    io.write_all(format!("{PROBE_TAG} CAPABILITY\r\n").as_bytes())
        .await
        .map_err(connect_err)?;
    io.flush().await.map_err(connect_err)?;
    let mut atoms: Vec<String> = Vec::new();
    loop {
        let Some(r) = client.read_response().await.map_err(connect_err)? else {
            return Err(MailError::Connect(
                "connection closed during CAPABILITY".to_string(),
            ));
        };
        match r.parsed() {
            Response::Capabilities(caps) => {
                for c in caps {
                    atoms.push(match c {
                        ProtoCapability::Imap4rev1 => "IMAP4rev1".to_string(),
                        ProtoCapability::Auth(a) => format!("AUTH={a}"),
                        ProtoCapability::Atom(a) => a.to_string(),
                    });
                }
            }
            Response::Done { tag, status, .. } if tag.0 == PROBE_TAG => {
                if *status != Status::Ok {
                    return Err(MailError::Connect("CAPABILITY was refused".to_string()));
                }
                break;
            }
            _ => {}
        }
    }
    Ok(format!("* CAPABILITY {}\r\n", atoms.join(" ")).into_bytes())
}

/// True when `response` contains the `STARTTLS` token (ASCII
/// case-insensitive) bounded by a space, `[` or line end.
pub(crate) fn advertises_starttls(response: &[u8]) -> bool {
    const TOKEN: &[u8] = b"STARTTLS";
    response.windows(TOKEN.len()).enumerate().any(|(i, w)| {
        w.eq_ignore_ascii_case(TOKEN)
            && (i == 0 || matches!(response[i - 1], b' ' | b'['))
            && matches!(
                response.get(i + TOKEN.len()),
                None | Some(b' ' | b'\r' | b'\n' | b']')
            )
    })
}

#[async_trait]
impl MailStoreConnector for AsyncImapConnector {
    async fn connect(&self) -> Result<Box<dyn MailStore>, MailError> {
        tokio::time::timeout(self.connect_timeout, self.connect_inner())
            .await
            .map_err(|_| MailError::Connect("connect timed out".to_string()))?
    }
}

impl AsyncImapConnector {
    async fn connect_inner(&self) -> Result<Box<dyn MailStore>, MailError> {
        let tcp = TcpStream::connect((self.server.host.as_str(), self.server.port))
            .await
            .map_err(connect_err)?;

        let client = match self.tls {
            TlsMode::Implicit => {
                let mut c = RawClient::new(self.tls_wrap(tcp).await?);
                read_greeting(&mut c).await?;
                c
            }
            TlsMode::StartTls => {
                let mut c = RawClient::new(ImapStream::Plain(tcp));
                read_greeting(&mut c).await?;
                let caps = probe_capabilities(&mut c).await?;
                if !advertises_starttls(&caps) {
                    return Err(MailError::Tls(
                        "IMAP server does not advertise STARTTLS; set allowCleartext=true only for trusted links"
                            .to_string(),
                    ));
                }
                c.run_command_and_check_ok("STARTTLS", None)
                    .await
                    .map_err(|e| {
                        MailError::Connect(format!("STARTTLS refused: {}", describe(&e)))
                    })?;
                let ImapStream::Plain(plain) = c.into_inner() else {
                    return Err(MailError::Connect("unexpected stream state".to_string()));
                };
                // The greeting is not repeated after the upgrade.
                RawClient::new(self.tls_wrap(plain).await?)
            }
            TlsMode::Cleartext => {
                let mut c = RawClient::new(ImapStream::Plain(tcp));
                read_greeting(&mut c).await?;
                c
            }
        };

        // Constant messages only: the error must not echo the login line.
        let session = client
            .login(&self.login.username, self.login.password.expose())
            .await
            .map_err(|(e, _client)| match e {
                async_imap::error::Error::No(_) | async_imap::error::Error::Bad(_) => {
                    MailError::LoginRefused
                }
                async_imap::error::Error::Validate(_) => MailError::Config(
                    "the account username or password holds a character IMAP cannot send"
                        .to_string(),
                ),
                _ => MailError::Connect("IMAP login failed".to_string()),
            })?;
        Ok(Box::new(AsyncImapStore {
            session,
            io_timeout: self.io_timeout,
        }))
    }
}

/// A tagged `NO` or `BAD` leaves the session usable; anything else (I/O,
/// lost connection, unparsable reply) breaks it.
fn store_err(e: async_imap::error::Error) -> MailError {
    match &e {
        async_imap::error::Error::No(text) | async_imap::error::Error::Bad(text) => {
            MailError::Refused(error::server_text(text))
        }
        other => MailError::Store(describe(other)),
    }
}

/// Bound one store operation. A timeout abandons a command mid-flight, so
/// it maps to a retryable error and the session is dropped.
async fn with_timeout<T>(
    limit: Duration,
    fut: impl std::future::Future<Output = Result<T, MailError>>,
) -> Result<T, MailError> {
    tokio::time::timeout(limit, fut)
        .await
        .map_err(|_| MailError::Store("IMAP operation timed out".to_string()))?
}

struct AsyncImapStore {
    session: async_imap::Session<ImapStream>,
    io_timeout: Duration,
}

#[async_trait]
impl MailStore for AsyncImapStore {
    async fn capabilities(&mut self) -> Result<StoreCapabilities, MailError> {
        let limit = self.io_timeout;
        with_timeout(limit, async {
            let caps = self.session.capabilities().await.map_err(store_err)?;
            Ok(StoreCapabilities {
                uidplus: caps.has_str("UIDPLUS"),
                move_ext: caps.has_str("MOVE"),
            })
        })
        .await
    }

    async fn select(&mut self, folder: &str) -> Result<SelectedFolder, MailError> {
        let limit = self.io_timeout;
        with_timeout(limit, async {
            let mailbox = self.session.select(folder).await.map_err(store_err)?;
            let uid_validity = mailbox
                .uid_validity
                .ok_or_else(|| MailError::Store("server sent no UIDVALIDITY".to_string()))?;
            Ok(SelectedFolder { uid_validity })
        })
        .await
    }

    async fn search_unseen(&mut self) -> Result<Vec<u32>, MailError> {
        let limit = self.io_timeout;
        with_timeout(limit, async {
            let found = self.session.uid_search("UNSEEN").await.map_err(store_err)?;
            let mut uids: Vec<u32> = found.into_iter().collect();
            uids.sort_unstable();
            Ok(uids)
        })
        .await
    }

    async fn fetch_sizes(&mut self, uids: &[u32]) -> Result<Vec<MessageSize>, MailError> {
        let limit = self.io_timeout;
        with_timeout(limit, async {
            if uids.is_empty() {
                return Ok(Vec::new());
            }
            let set = uids
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let fetches: Vec<_> = self
                .session
                .uid_fetch(set, "RFC822.SIZE")
                .await
                .map_err(store_err)?
                .try_collect()
                .await
                .map_err(store_err)?;
            Ok(fetches
                .iter()
                .filter_map(|f| {
                    Some(MessageSize {
                        uid: f.uid?,
                        size: u64::from(f.size?),
                    })
                })
                .collect())
        })
        .await
    }

    async fn fetch_body(&mut self, uid: u32) -> Result<Option<Vec<u8>>, MailError> {
        let limit = self.io_timeout;
        with_timeout(limit, async {
            let fetches: Vec<_> = self
                .session
                .uid_fetch(uid.to_string(), "BODY.PEEK[]")
                .await
                .map_err(store_err)?
                .try_collect()
                .await
                .map_err(store_err)?;
            Ok(fetches.iter().find_map(|f| f.body().map(<[u8]>::to_vec)))
        })
        .await
    }

    async fn mark_seen(&mut self, uid: u32) -> Result<(), MailError> {
        let limit = self.io_timeout;
        with_timeout(limit, async {
            let _: Vec<_> = self
                .session
                .uid_store(uid.to_string(), "+FLAGS (\\Seen)")
                .await
                .map_err(store_err)?
                .try_collect()
                .await
                .map_err(store_err)?;
            Ok(())
        })
        .await
    }

    async fn copy(&mut self, uid: u32, folder: &str) -> Result<(), MailError> {
        let limit = self.io_timeout;
        with_timeout(limit, async {
            self.session
                .uid_copy(uid.to_string(), folder)
                .await
                .map_err(store_err)
        })
        .await
    }

    async fn move_to(&mut self, uid: u32, folder: &str) -> Result<(), MailError> {
        let limit = self.io_timeout;
        with_timeout(limit, async {
            self.session
                .uid_mv(uid.to_string(), folder)
                .await
                .map_err(store_err)
        })
        .await
    }

    async fn delete_uid(&mut self, uid: u32) -> Result<(), MailError> {
        let limit = self.io_timeout;
        with_timeout(limit, async {
            let _: Vec<_> = self
                .session
                .uid_store(uid.to_string(), "+FLAGS.SILENT (\\Deleted)")
                .await
                .map_err(store_err)?
                .try_collect()
                .await
                .map_err(store_err)?;
            let _: Vec<_> = self
                .session
                .uid_expunge(uid.to_string())
                .await
                .map_err(store_err)?
                .try_collect()
                .await
                .map_err(store_err)?;
            Ok(())
        })
        .await
    }

    async fn logout(&mut self) -> Result<(), MailError> {
        let limit = self.io_timeout;
        with_timeout(limit, async {
            self.session.logout().await.map_err(store_err)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::test_support::tls::{gen_server_cert, write_pem_tmp};
    use tokio::time::timeout;

    fn account(ca: Option<std::path::PathBuf>, with_login: bool) -> MailAccountConfig {
        MailAccountConfig {
            username: with_login.then(|| "u".to_string()),
            password: with_login.then(|| SecretString::from("hunter2")),
            ca_file: ca,
        }
    }

    #[test]
    fn from_endpoint_requires_login() {
        let cfg = ImapEndpointConfig::from_uri("imap://h").expect("uri");
        let err = AsyncImapConnector::from_endpoint(&cfg, &account(None, false))
            .err()
            .expect("must fail");
        assert!(matches!(err, MailError::Config(_)));
        assert!(err.to_string().contains("[components.mail.accounts]"));
    }

    #[test]
    fn from_endpoint_builds_with_ca_file() {
        let cfg = ImapEndpointConfig::from_uri("imap://h").expect("uri");
        let ca = write_pem_tmp("imap-ca.pem", &gen_server_cert().0);
        assert!(AsyncImapConnector::from_endpoint(&cfg, &account(Some(ca), true)).is_ok());
    }

    #[test]
    fn imap_login_debug_redacts() {
        let login = ImapLogin {
            username: "u".into(),
            password: SecretString::from("hunter2"),
        };
        let out = format!("{login:?}");
        assert!(!out.contains("hunter2"), "{out}");
    }

    #[test]
    fn starttls_token_detection() {
        assert!(advertises_starttls(
            b"* CAPABILITY IMAP4rev1 STARTTLS AUTH=PLAIN\r\n"
        ));
        assert!(!advertises_starttls(
            b"* CAPABILITY IMAP4rev1 AUTH=PLAIN\r\n"
        ));
        assert!(!advertises_starttls(
            b"* CAPABILITY IMAP4rev1 XSTARTTLSX\r\n"
        ));
        assert!(advertises_starttls(b"* CAPABILITY IMAP4rev1 starttls\r\n"));
    }

    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::net::TcpListener;

    const BOUND: Duration = Duration::from_secs(10);

    /// Accepts one connection, sends `greeting` (if any), then answers
    /// LOGIN with `login_reply`, SELECT with `select_reply` (if any) and
    /// stays silent on everything else. The listener stays bound for the
    /// whole test.
    async fn scripted(
        greeting: Option<&'static str>,
        login_reply: &'static str,
        select_reply: Option<String>,
    ) -> (u16, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let task = tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let mut io = BufReader::new(stream);
            if let Some(g) = greeting {
                let _ = io.get_mut().write_all(g.as_bytes()).await;
            }
            let mut line = String::new();
            // allow-test-wait: ends when the client disconnects or the test aborts the task
            while io.read_line(&mut line).await.unwrap_or(0) > 0 {
                if let Some((tag, rest)) = line.trim_end().split_once(' ') {
                    let reply = if rest.starts_with("LOGIN ") {
                        Some(login_reply)
                    } else if rest.starts_with("SELECT ") {
                        select_reply.as_deref()
                    } else {
                        None
                    };
                    if let Some(reply) = reply {
                        let _ = io
                            .get_mut()
                            .write_all(format!("{tag} {reply}\r\n").as_bytes())
                            .await;
                    }
                }
                line.clear();
            }
            drop(listener);
        });
        (port, task)
    }

    fn cleartext(port: u16) -> AsyncImapConnector {
        let login = ImapLogin {
            username: "u".into(),
            password: SecretString::from("hunter2"),
        };
        AsyncImapConnector::new(
            MailServerAddress {
                host: "127.0.0.1".to_string(),
                port,
            },
            TlsMode::Cleartext,
            login,
            tls::client_config(&[]).expect("tls config"),
        )
        .with_timeouts(Duration::from_millis(300), Duration::from_millis(300))
    }

    #[tokio::test]
    async fn silent_server_times_out() {
        let (port, task) = scripted(None, "OK done", None).await;
        let r = timeout(BOUND, cleartext(port).connect())
            .await
            .expect("bounded");
        assert!(matches!(r, Err(MailError::Connect(_))));
        task.abort();
    }

    #[tokio::test]
    async fn silent_server_after_login_times_out_on_io() {
        let (port, task) = scripted(Some("* OK ready\r\n"), "OK done", None).await;
        let mut store = timeout(BOUND, cleartext(port).connect())
            .await
            .expect("bounded")
            .expect("connect");
        let r = timeout(BOUND, store.select("INBOX"))
            .await
            .expect("bounded");
        assert!(matches!(r, Err(MailError::Store(_))));
        task.abort();
    }

    #[tokio::test]
    async fn preauth_greeting_is_rejected() {
        let (port, task) = scripted(Some("* PREAUTH welcome\r\n"), "OK done", None).await;
        let r = timeout(BOUND, cleartext(port).connect())
            .await
            .expect("bounded");
        assert!(matches!(r, Err(MailError::Connect(_))));
        task.abort();
    }

    #[tokio::test]
    async fn tagged_no_maps_to_refused() {
        let (port, task) = scripted(
            Some("* OK ready\r\n"),
            "OK done",
            Some("NO [NONEXISTENT] no such mailbox".to_string()),
        )
        .await;
        let mut store = timeout(BOUND, cleartext(port).connect())
            .await
            .expect("bounded")
            .expect("connect");
        let r = timeout(BOUND, store.select("Missing"))
            .await
            .expect("bounded");
        assert!(
            matches!(&r, Err(e @ MailError::Refused(_)) if !e.is_retryable()),
            "{r:?}"
        );
        task.abort();
    }

    #[tokio::test]
    async fn refused_login_is_permanent_and_constant() {
        let (port, task) = scripted(
            Some("* OK ready\r\n"),
            "NO [AUTHENTICATIONFAILED] hunter2 is wrong",
            None,
        )
        .await;
        let r = timeout(BOUND, cleartext(port).connect())
            .await
            .expect("bounded");
        let err = r.err().expect("login must fail");
        assert!(matches!(err, MailError::LoginRefused), "{err:?}");
        assert!(!err.is_retryable(), "{err}");
        assert!(!err.to_string().contains("hunter2"), "{err}");
        task.abort();
    }

    async fn select_error(reply: String) -> MailError {
        let (port, task) = scripted(Some("* OK ready\r\n"), "OK done", Some(reply)).await;
        let mut store = timeout(BOUND, cleartext(port).connect())
            .await
            .expect("bounded")
            .expect("connect");
        let r = timeout(BOUND, store.select("INBOX"))
            .await
            .expect("bounded");
        task.abort();
        r.expect_err("select must fail")
    }

    fn assert_bounded(err: &MailError) {
        let text = err.to_string();
        assert!(text.chars().count() < 300, "unbounded error text: {text}");
        assert!(!text.chars().any(char::is_control), "{text:?}");
    }

    #[tokio::test]
    async fn oversized_refusal_text_is_bounded() {
        let err = select_error(format!("NO {}", "A".repeat(5000))).await;
        assert!(matches!(err, MailError::Refused(_)), "{err:?}");
        assert_bounded(&err);
    }

    #[tokio::test]
    async fn garbage_response_is_not_echoed() {
        let err = select_error(format!("\u{1}\u{7}GARBAGE {}", "Z".repeat(5000))).await;
        assert!(err.is_retryable(), "{err:?}");
        assert_bounded(&err);
        assert!(!err.to_string().contains("ZZZZ"), "{err}");
    }
}
