//! Store seam between the IMAP consumer and the IMAP library.

use async_trait::async_trait;

use crate::error::MailError;

#[cfg(test)]
pub(crate) mod fake;

/// Server extensions the consumer cares about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StoreCapabilities {
    pub(crate) uidplus: bool,
    pub(crate) move_ext: bool,
}

/// Result of selecting a folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SelectedFolder {
    pub(crate) uid_validity: u32,
}

/// Size of one message as reported by the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MessageSize {
    pub(crate) uid: u32,
    pub(crate) size: u64,
}

/// One authenticated IMAP session.
///
/// An error for which [`MailError::is_retryable`] holds means the session
/// is broken and must be dropped; [`MailError::Refused`] leaves it usable.
#[async_trait]
pub(crate) trait MailStore: Send {
    async fn capabilities(&mut self) -> Result<StoreCapabilities, MailError>;
    async fn select(&mut self, folder: &str) -> Result<SelectedFolder, MailError>;
    /// Unseen UIDs in ascending order.
    async fn search_unseen(&mut self) -> Result<Vec<u32>, MailError>;
    async fn fetch_sizes(&mut self, uids: &[u32]) -> Result<Vec<MessageSize>, MailError>;
    /// Raw message; `None` when the server returns no body because the
    /// message was expunged after the search.
    async fn fetch_body(&mut self, uid: u32) -> Result<Option<Vec<u8>>, MailError>;
    async fn mark_seen(&mut self, uid: u32) -> Result<(), MailError>;
    /// Copy the message to `folder`, leaving the original in place.
    async fn copy(&mut self, uid: u32, folder: &str) -> Result<(), MailError>;
    /// Move the message to `folder` with the server `MOVE` extension.
    async fn move_to(&mut self, uid: u32, folder: &str) -> Result<(), MailError>;
    /// Flag the message `\Deleted` and `UID EXPUNGE` it; never a plain `EXPUNGE`.
    async fn delete_uid(&mut self, uid: u32) -> Result<(), MailError>;
    async fn logout(&mut self) -> Result<(), MailError>;
}

/// Opens authenticated sessions.
#[async_trait]
pub(crate) trait MailStoreConnector: Send + Sync {
    async fn connect(&self) -> Result<Box<dyn MailStore>, MailError>;
}
