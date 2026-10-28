//! Mailpit docker fixture (STARTTLS-required SMTP + HTTP API).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use camel_component_api::test_support::tls::gen_server_cert;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use super::docker::{docker_exec, nanos};

pub const MAILPIT_IMAGE: &str = "axllent/mailpit:v1.20.0";
const IO_BOUND: Duration = Duration::from_secs(10);
const READY_BOUND: Duration = Duration::from_secs(60);

pub struct MailpitFixture {
    container: String,
    pub smtp_port: u16,
    pub http_port: u16,
    pub ca_path: PathBuf,
    _certs: tempfile::TempDir,
}

fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, content).expect("write cert file");
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).expect("chmod file");
    p
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

impl MailpitFixture {
    pub async fn start() -> Self {
        let (ca, cert, key) = gen_server_cert();
        let certs = tempfile::tempdir().expect("tempdir");
        std::fs::set_permissions(certs.path(), std::fs::Permissions::from_mode(0o755))
            .expect("chmod dir");
        let ca_path = write_file(certs.path(), "ca.pem", &ca);
        write_file(certs.path(), "server.pem", &cert);
        write_file(certs.path(), "server.key", &key);

        let name = format!("mail-itest-{}-{}", std::process::id(), nanos());
        let volume = format!("{}:/certs:ro", certs.path().display());
        if let Err(e) = docker_exec(&[
            "run",
            "-d",
            "--name",
            &name,
            "-p",
            "127.0.0.1::1025",
            "-p",
            "127.0.0.1::8025",
            "-v",
            &volume,
            "-e",
            "MP_SMTP_TLS_CERT=/certs/server.pem",
            "-e",
            "MP_SMTP_TLS_KEY=/certs/server.key",
            "-e",
            "MP_SMTP_REQUIRE_STARTTLS=true",
            "-e",
            "MP_SMTP_AUTH_ACCEPT_ANY=true",
            MAILPIT_IMAGE,
        ]) {
            let _ = docker_exec(&["rm", "-f", &name]);
            panic!("infra-unavailable: docker run failed: {e}");
        }
        let smtp_port = mapped_port(&name, "1025/tcp");
        let http_port = mapped_port(&name, "8025/tcp");
        // From here the Drop impl owns cleanup.
        let fx = Self {
            container: name,
            smtp_port,
            http_port,
            ca_path,
            _certs: certs,
        };
        let deadline = Instant::now() + READY_BOUND;
        loop {
            if let Ok((200, _)) = http_get(fx.http_port, "/api/v1/info").await {
                break;
            }
            if Instant::now() >= deadline {
                panic!("infra-unavailable: mailpit not ready within 60s");
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        fx
    }

    pub async fn subjects(&self) -> Vec<String> {
        let Ok((200, body)) = http_get(self.http_port, "/api/v1/messages").await else {
            return Vec::new();
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
            return Vec::new();
        };
        v["messages"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| m["Subject"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Drop for MailpitFixture {
    fn drop(&mut self) {
        if let Err(e) = docker_exec(&["rm", "-f", &self.container]) {
            eprintln!("mailpit fixture cleanup failed ({}): {e}", self.container);
        }
    }
}

/// Minimal HTTP/1.0 client; status parsed from the status line.
pub async fn http_get(port: u16, path: &str) -> Result<(u16, String), String> {
    let mut stream = timeout(IO_BOUND, TcpStream::connect(("127.0.0.1", port)))
        .await
        .map_err(|_| "connect timeout".to_string())?
        .map_err(|e| format!("connect: {e}"))?;
    let req = format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n");
    timeout(IO_BOUND, stream.write_all(req.as_bytes()))
        .await
        .map_err(|_| "write timeout".to_string())?
        .map_err(|e| format!("write: {e}"))?;
    let mut raw = Vec::new();
    timeout(IO_BOUND, stream.read_to_end(&mut raw))
        .await
        .map_err(|_| "read timeout".to_string())?
        .map_err(|e| format!("read: {e}"))?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| "malformed response".to_string())?;
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_ascii_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| "bad status line".to_string())?;
    Ok((status, body.to_string()))
}

pub async fn wait_for_subject(fx: &MailpitFixture, subject: &str, bound: Duration) -> bool {
    let deadline = Instant::now() + bound;
    loop {
        if fx.subjects().await.iter().any(|s| s == subject) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
