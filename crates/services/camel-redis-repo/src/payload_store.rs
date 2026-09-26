//! Redis-backed `PayloadStore` for the offload decorator (ADR-0065).
//!
//! [`RedisPayloadStore`] stores entry payloads inside the repository's
//! keyspace (`{key_prefix}:{repo}:payload:{blob-name}`) so the index's
//! `clear()` prefix-scoped SCAN reclaims payloads eagerly and the
//! namespace guards stay uniform. Blob names are keys, not namespace
//! tokens, so the charset guard is not applied to them.
//!
//! Every payload is written with an `EXAT` deadline derived from its
//! death epoch — native expiry replaces the disk sweeper, so a payload
//! blob is never stored without a deadline.

use std::sync::Arc;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use camel_api::CamelError;
use camel_api::ComponentMetrics;
use camel_core::cache::PayloadStore;
use redis::SetExpiry;
use redis::SetOptions;

use crate::executor::RepoCommandExecutor;
use crate::executor::execute_retry_safe;
use crate::namespaced;
use crate::validate_namespace_token;

/// Redis-backed payload storage (see module docs).
pub struct RedisPayloadStore {
    executor: Arc<dyn RepoCommandExecutor>,
    metrics: ComponentMetrics,
    key_prefix: String,
    repo_name: String,
}

impl RedisPayloadStore {
    /// Build the store around an existing executor.
    ///
    /// Validates both namespace tokens before any command is issued.
    /// `metrics` — component-operations facade used to observe transient
    /// classifications on the payload paths.
    pub(crate) fn with_executor(
        key_prefix: &str,
        repo_name: &str,
        executor: Arc<dyn RepoCommandExecutor>,
        metrics: ComponentMetrics,
    ) -> Result<Self, CamelError> {
        validate_namespace_token("key_prefix", key_prefix)?;
        validate_namespace_token("repository name", repo_name)?;
        Ok(Self {
            executor,
            metrics,
            key_prefix: key_prefix.to_string(),
            repo_name: repo_name.to_string(),
        })
    }

    fn payload_key(&self, name: &str) -> String {
        namespaced(
            &self.key_prefix,
            &self.repo_name,
            &format!("payload:{name}"),
        )
    }
}

#[async_trait]
impl PayloadStore for RedisPayloadStore {
    /// `SET key bytes` with `EXAT death_epoch_secs`.
    ///
    /// The EXAT seconds come from a checked `duration_since(UNIX_EPOCH)`;
    /// an unusable death epoch fails the put BEFORE any command is issued
    /// so the decorator's inline fallback applies — a payload blob is
    /// never stored without its deadline.
    async fn put(
        &self,
        name: &str,
        bytes: &[u8],
        death_epoch: SystemTime,
    ) -> Result<(), CamelError> {
        // Trait contract (ADR-0065): a payload name is a bare single
        // path component — the decorator's content-addressed blob name.
        // Reject anything else before it becomes part of a key.
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(CamelError::Config(format!(
                "cache payload name must be a bare file name, got '{name}'"
            )));
        }
        let exat_secs = death_epoch
            .duration_since(UNIX_EPOCH)
            .map_err(|e| CamelError::Io(format!("payload death epoch before the Unix epoch: {e}")))?
            .as_secs();
        let mut cmd = redis::Cmd::new();
        cmd.arg("SET")
            .arg(self.payload_key(name))
            .arg(bytes)
            .arg(SetOptions::default().with_expiration(SetExpiry::EXAT(exat_secs)));
        execute_retry_safe(&self.executor, cmd, &self.metrics, "set")
            .await
            .map(|_| ())
    }

    /// `GET key`; a nil reply is `Ok(None)` (the payload is gone — the
    /// decorator degrades to a MISS). A transport failure surfaces as
    /// `Err` (mapped to `Io` by the executor), never a silent `None`.
    async fn read(&self, name: &str) -> Result<Option<Vec<u8>>, CamelError> {
        let mut cmd = redis::Cmd::new();
        cmd.arg("GET").arg(self.payload_key(name));
        match execute_retry_safe(&self.executor, cmd, &self.metrics, "get").await? {
            redis::Value::Nil => Ok(None),
            redis::Value::BulkString(bytes) => Ok(Some(bytes)),
            other => Err(CamelError::Io(format!(
                "unexpected reply for payload GET: {other:?}"
            ))),
        }
    }

    /// `UNLINK key`; a nil reply (already gone — a concurrent reclaimer
    /// won the race) counts as success.
    async fn unlink(&self, name: &str) -> Result<(), CamelError> {
        let mut cmd = redis::Cmd::new();
        cmd.arg("UNLINK").arg(self.payload_key(name));
        execute_retry_safe(&self.executor, cmd, &self.metrics, "unlink")
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::RedisPayloadStore;
    use crate::executor::FakeRepoExecutor;
    use crate::executor::test_support::arg_after;
    use crate::executor::test_support::cmd_args;
    use camel_api::CamelError;
    use camel_api::ComponentMetrics;
    use camel_core::cache::PayloadStore;
    use std::sync::Arc;
    use std::time::Duration;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    fn store(executor: Arc<FakeRepoExecutor>) -> RedisPayloadStore {
        RedisPayloadStore::with_executor(
            "camel:cache",
            "default",
            executor,
            ComponentMetrics::new(Arc::new(camel_api::metrics::MetricsHandle::new()), false),
        )
        .expect("valid constructor arguments")
    }

    /// Deterministic death epoch: base test epoch + 100s.
    fn death_epoch() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_100)
    }

    fn transient(message: &str) -> CamelError {
        CamelError::Io(message.into())
    }

    #[tokio::test]
    async fn payload_put_sets_namespaced_key_with_exat() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let store = store(fake.clone());
        fake.push_result(Ok(redis::Value::Okay));

        store
            .put("abc.blob", b"payload-bytes", death_epoch())
            .await
            .expect("put must succeed");

        let commands = fake.commands();
        assert_eq!(commands.len(), 1, "put is ONE command");
        let args = cmd_args(&commands[0]);
        assert_eq!(args[0], b"SET".to_vec());
        assert_eq!(
            args[1],
            b"camel:cache:default:payload:abc.blob".to_vec(),
            "payload key lives inside the repository namespace"
        );
        assert_eq!(args[2], b"payload-bytes".to_vec());
        let expected_exat = death_epoch()
            .duration_since(UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs();
        assert_eq!(
            arg_after(&args, b"EXAT"),
            expected_exat.to_string().into_bytes(),
            "SET carries the death epoch as EXAT seconds"
        );
    }

    #[tokio::test]
    async fn payload_put_rejects_non_bare_name() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let store = store(fake.clone());

        let err = store
            .put("dir/escape.blob", b"payload-bytes", death_epoch())
            .await
            .expect_err("a name with a separator must fail the put");

        assert!(
            matches!(err, CamelError::Config(_)),
            "expected CamelError::Config, got: {err:?}"
        );
        assert!(
            fake.commands().is_empty(),
            "the rejected put must not reach the transport"
        );
    }

    #[tokio::test]
    async fn payload_read_returns_bytes() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let store = store(fake.clone());
        fake.push_result(Ok(redis::Value::BulkString(b"payload-bytes".to_vec())));

        let got = store
            .read("abc.blob")
            .await
            .expect("read must succeed on a stored payload");

        assert_eq!(got, Some(b"payload-bytes".to_vec()));
    }

    #[tokio::test]
    async fn payload_read_nil_is_none() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let store = store(fake.clone());
        fake.push_result(Ok(redis::Value::Nil));

        let got = store
            .read("abc.blob")
            .await
            .expect("a nil reply is a miss, not an error");

        assert_eq!(got, None, "nil reply degrades to a miss");
    }

    #[tokio::test]
    async fn payload_unlink_uses_unlink_and_tolerates_nil() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let store = store(fake.clone());
        fake.push_result(Ok(redis::Value::Int(1)));
        fake.push_result(Ok(redis::Value::Nil));

        store.unlink("abc.blob").await.expect("unlink must succeed");
        store
            .unlink("abc.blob")
            .await
            .expect("a nil reply means the payload is already gone — success");

        let commands = fake.commands();
        assert_eq!(commands.len(), 2, "each unlink is ONE command");
        for cmd in &commands {
            assert_eq!(
                cmd_args(cmd),
                vec![
                    b"UNLINK".to_vec(),
                    b"camel:cache:default:payload:abc.blob".to_vec()
                ],
                "must UNLINK the namespaced payload key"
            );
        }
    }

    #[tokio::test]
    async fn payload_put_transient_retries_once() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let store = store(fake.clone());
        fake.push_result(Err(transient("connection reset by peer")));
        fake.push_result(Ok(redis::Value::Okay));

        store
            .put("abc.blob", b"payload-bytes", death_epoch())
            .await
            .expect("put must succeed after one transient failure");

        assert_eq!(fake.execute_count(), 2, "command executed twice");
        assert_eq!(fake.refresh_count(), 1, "connection refreshed once");
    }

    // C1: a payload that cannot be read surfaces as Err — never a silent
    // miss that the decorator would report as reclamation.
    #[tokio::test]
    async fn payload_read_transient_error_is_err_not_none() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let store = store(fake.clone());
        // Persistent failure: transient on both the first attempt and the
        // post-refresh retry, so the error still surfaces.
        fake.push_result(Err(transient("connection refused")));
        fake.push_result(Err(transient("connection refused")));

        match store.read("abc.blob").await {
            Err(CamelError::Io(_)) => {}
            other => panic!("transport failure must surface as Err, got: {other:?}"),
        }
        assert_eq!(
            fake.execute_count(),
            2,
            "one refresh-and-retry, then give up"
        );
    }

    #[tokio::test]
    async fn payload_put_overflow_epoch_is_err() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let store = store(fake.clone());
        let before_epoch = UNIX_EPOCH - Duration::from_secs(10);

        assert!(
            store
                .put("abc.blob", b"payload-bytes", before_epoch)
                .await
                .is_err(),
            "a death epoch before the Unix epoch must fail the put"
        );
        assert!(
            fake.commands().is_empty(),
            "no EXAT-less SET is ever issued"
        );
        assert_eq!(fake.execute_count(), 0, "no command reached the transport");
    }
}
