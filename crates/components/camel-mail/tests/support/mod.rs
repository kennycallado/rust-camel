#![allow(dead_code)]

pub mod docker;
pub mod fake_imap;
pub mod fake_smtp;
pub mod greenmail;
pub mod mailpit;
pub mod tls_fixture;

use std::time::Duration;

/// Receive one routed exchange, bounding both the lock and the `recv`.
pub async fn recv_with_timeout(
    received: &tokio::sync::Mutex<tokio::sync::mpsc::Receiver<camel_api::Exchange>>,
    bound: Duration,
) -> Option<camel_api::Exchange> {
    tokio::time::timeout(bound, async { received.lock().await.recv().await })
        .await
        .ok()
        .flatten()
}

/// Shared buffer receiving formatted log lines.
#[derive(Clone, Default)]
pub struct LogBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl LogBuf {
    pub fn text(&self) -> String {
        let bytes = self.0.lock().unwrap_or_else(|e| e.into_inner());
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl std::io::Write for LogBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuf {
    type Writer = LogBuf;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Capture every event of the current thread at `trace` level.
pub fn capture_logs() -> (tracing::subscriber::DefaultGuard, LogBuf) {
    let buf = LogBuf::default();
    let sub = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(buf.clone())
        .finish();
    (tracing::subscriber::set_default(sub), buf)
}
