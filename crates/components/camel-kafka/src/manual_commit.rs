//! Manual Kafka offset commit API.
//!
//! When `KafkaEndpointConfig::allow_manual_commit` is true, the consumer creates a
//! `KafkaManualCommit` per message and stores it in `exchange.extensions`
//! under the key `"kafka.manual_commit"`.  User processors retrieve it via
//! `exchange.get_extension::<KafkaManualCommit>("kafka.manual_commit")`.

use camel_api::CamelError;
use tokio::sync::{mpsc, oneshot};

/// Internal message sent from user code to the consumer's commit handler task.
pub(crate) struct CommitRequest {
    pub(crate) topic: String,
    pub(crate) partition: i32,
    pub(crate) offset: i64,
    /// Present for `commit()` (sync); absent for `commit_async()`.
    pub(crate) reply_tx: Option<oneshot::Sender<Result<(), CamelError>>>,
}

/// Handle for explicitly committing a Kafka offset from user processor code.
///
/// Obtained via `exchange.get_extension::<KafkaManualCommit>("kafka.manual_commit")`.
/// Only available when the consumer was created with `allowManualCommit=true`.
///
/// The handle is cheaply cloneable and safe to send across threads.
#[derive(Clone)]
pub struct KafkaManualCommit {
    pub(crate) topic: String,
    pub(crate) partition: i32,
    pub(crate) offset: i64,
    pub(crate) commit_tx: mpsc::Sender<CommitRequest>,
}

impl KafkaManualCommit {
    /// Create a new handle. Called only by the consumer.
    pub(crate) fn new(
        topic: String,
        partition: i32,
        offset: i64,
        commit_tx: mpsc::Sender<CommitRequest>,
    ) -> Self {
        Self {
            topic,
            partition,
            offset,
            commit_tx,
        }
    }

    /// Commit the offset and **wait** for broker acknowledgment.
    ///
    /// Returns `Err` if the consumer loop has shut down or the commit fails.
    pub async fn commit(&self) -> Result<(), CamelError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.commit_tx
            .send(CommitRequest {
                topic: self.topic.clone(),
                partition: self.partition,
                offset: self.offset,
                reply_tx: Some(reply_tx),
            })
            .await
            .map_err(|_| CamelError::ProcessorError("Commit channel closed".into()))?;
        reply_rx
            .await
            .map_err(|_| CamelError::ProcessorError("Commit response lost".into()))?
    }

    /// Enqueue a commit and **return immediately** without waiting.
    ///
    /// Returns `Err` only if the consumer loop has already shut down.
    /// Note: commits already queued but not yet processed will be discarded
    /// if the consumer shuts down (at-least-once semantics — messages will be
    /// reprocessed after restart).
    pub async fn commit_async(&self) -> Result<(), CamelError> {
        self.commit_tx
            .send(CommitRequest {
                topic: self.topic.clone(),
                partition: self.partition,
                offset: self.offset,
                reply_tx: None,
            })
            .await
            .map_err(|_| CamelError::ProcessorError("Commit channel closed".into()))?;
        Ok(())
    }

    /// The topic this handle commits on.
    pub fn topic(&self) -> &str {
        &self.topic
    }
    /// The partition this handle commits on.
    pub fn partition(&self) -> i32 {
        self.partition
    }
    /// The offset that will be committed (the offset of the message, not offset+1;
    /// the consumer adds +1 when submitting to rdkafka).
    pub fn offset(&self) -> i64 {
        self.offset
    }
}

impl std::fmt::Debug for KafkaManualCommit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KafkaManualCommit")
            .field("topic", &self.topic)
            .field("partition", &self.partition)
            .field("offset", &self.offset)
            .finish_non_exhaustive() // omits commit_tx (not Debug)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[test]
    fn test_accessors() {
        let (tx, _rx) = mpsc::channel(1);
        let mc = KafkaManualCommit::new("my-topic".into(), 2, 100, tx);
        assert_eq!(mc.topic(), "my-topic");
        assert_eq!(mc.partition(), 2);
        assert_eq!(mc.offset(), 100);
    }

    #[test]
    fn test_debug_does_not_panic() {
        let (tx, _rx) = mpsc::channel(1);
        let mc = KafkaManualCommit::new("t".into(), 0, 0, tx);
        let _ = format!("{mc:?}");
    }

    #[tokio::test]
    async fn test_commit_async_returns_ok_when_channel_open() {
        let (tx, mut rx) = mpsc::channel(1);
        let mc = KafkaManualCommit::new("t".into(), 0, 5, tx);
        mc.commit_async().await.unwrap();
        let req = rx.recv().await.unwrap();
        assert_eq!(req.topic, "t");
        assert_eq!(req.offset, 5);
        assert!(req.reply_tx.is_none());
    }

    #[tokio::test]
    async fn test_commit_async_returns_err_when_channel_closed() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let mc = KafkaManualCommit::new("t".into(), 0, 0, tx);
        assert!(mc.commit_async().await.is_err());
    }

    #[tokio::test]
    async fn test_commit_roundtrip() {
        let (tx, mut rx) = mpsc::channel(1);
        let mc = KafkaManualCommit::new("t".into(), 0, 10, tx);

        // Spawn a fake commit handler
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                assert_eq!(req.offset, 10);
                if let Some(reply_tx) = req.reply_tx {
                    let _ = reply_tx.send(Ok(()));
                }
            }
        });

        mc.commit().await.unwrap();
    }

    #[tokio::test]
    async fn test_commit_returns_err_when_channel_closed() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let mc = KafkaManualCommit::new("t".into(), 0, 0, tx);
        assert!(mc.commit().await.is_err());
    }

    #[tokio::test]
    async fn test_commit_returns_err_when_reply_dropped() {
        let (tx, mut rx) = mpsc::channel(1);
        let mc = KafkaManualCommit::new("t".into(), 0, 10, tx);

        // Handler receives but drops reply without responding
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                // Intentionally drop reply_tx without sending
                drop(req.reply_tx);
            }
        });

        let result = mc.commit().await;
        assert!(result.is_err());
    }
}
