//! Test-only probe hooks (feature `test-support`); never enable in production.

use crate::adapter::imap::AsyncImapConnector;
use crate::config::{ImapEndpointConfig, MailAccountConfig};
use crate::error::MailError;
use crate::store::MailStoreConnector;

/// Observations from one IMAP probe session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    /// Unseen UIDs in `INBOX`.
    pub uids: Vec<u32>,
    /// `(uid, size)` of the first unseen message.
    pub sizes: Vec<(u32, u64)>,
    /// Body length of the first unseen message.
    pub first_body_len: usize,
}

/// Connect, select `INBOX`, search unseen, fetch the first message, mark it
/// seen and log out.
pub async fn imap_connect_probe(
    uri: &str,
    account: MailAccountConfig,
) -> Result<ProbeResult, MailError> {
    let cfg = ImapEndpointConfig::from_uri(uri).map_err(|e| MailError::Config(e.to_string()))?;
    let connector = AsyncImapConnector::from_endpoint(&cfg, &account)?;
    let mut store = connector.connect().await?;
    store.select("INBOX").await?;
    let uids = store.search_unseen().await?;
    let mut sizes = Vec::new();
    let mut first_body_len = 0;
    if let Some(&first) = uids.first() {
        sizes = store
            .fetch_sizes(&[first])
            .await?
            .into_iter()
            .map(|s| (s.uid, s.size))
            .collect();
        first_body_len = store.fetch_body(first).await?.map_or(0, |body| body.len());
        store.mark_seen(first).await?;
    }
    store.logout().await?;
    Ok(ProbeResult {
        uids,
        sizes,
        first_body_len,
    })
}

/// Connect, select `INBOX`, then run `op` (`copy`, `move` or `delete`) on
/// UID 42 and log out.
pub async fn imap_mutation_probe(
    cfg_uri: &str,
    account: MailAccountConfig,
    op: &str,
    folder: &str,
) -> Result<(), MailError> {
    let cfg =
        ImapEndpointConfig::from_uri(cfg_uri).map_err(|e| MailError::Config(e.to_string()))?;
    let connector = AsyncImapConnector::from_endpoint(&cfg, &account)?;
    let mut store = connector.connect().await?;
    store.select("INBOX").await?;
    match op {
        "copy" => store.copy(42, folder).await?,
        "move" => store.move_to(42, folder).await?,
        "delete" => store.delete_uid(42).await?,
        other => return Err(MailError::Config(format!("unknown mutation op {other}"))),
    }
    store.logout().await
}
