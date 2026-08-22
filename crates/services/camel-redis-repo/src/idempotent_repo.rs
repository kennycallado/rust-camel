//! Redis-backed `IdempotentRepository` for the Idempotent Consumer EIP.
//!
//! [`RedisIdempotentRepository`] owns the namespaced key layout
//! (`{key_prefix}:{name}:{key}`) and the [`RepoCommandExecutor`] transport
//! seam. `add` is a single non-retried `SET NX` (Contract C1 — see
//! [`RedisIdempotentRepository::add`]); `contains`/`remove` go through
//! [`execute_retry_safe`] because they are safe to re-issue; `clear` is a
//! scoped `SCAN` + `UNLINK` walk that never touches other repositories'
//! keys or issues FLUSH commands.

use crate::connection;
use crate::executor::RepoCommandExecutor;
use crate::executor::execute_retry_safe;
use crate::executor::scan_unlink_pattern;
use crate::is_transient_redis_error;
use crate::namespaced;
use crate::validate_namespace_token;
use async_trait::async_trait;
use camel_api::CamelError;
use camel_api::IdempotentRepository;
use camel_component_redis::RedisEndpointConfig;
use std::sync::Arc;

/// Redis-backed idempotent repository (see module docs).
pub struct RedisIdempotentRepository {
    name: String,
    key_prefix: String,
    executor: Arc<dyn RepoCommandExecutor>,
}

impl RedisIdempotentRepository {
    /// Connect to `endpoint` and build the repository.
    ///
    /// Validates `name` and `key_prefix` before any network I/O, then
    /// eagerly connects (one topology resolution — see [`connection`]).
    pub async fn connect(
        name: &str,
        endpoint: &RedisEndpointConfig,
        key_prefix: &str,
    ) -> Result<Self, CamelError> {
        validate_namespace_token("repository name", name)?;
        validate_namespace_token("key_prefix", key_prefix)?;
        let executor = connection::connect_executor(endpoint).await?;
        Self::with_executor(name, key_prefix, Arc::new(executor))
    }

    /// Test seam: build the repository around an injected executor.
    ///
    /// Sync and network-free; validates both namespace tokens first — the
    /// name is part of every SCAN pattern, so glob metacharacters in it
    /// would break `clear` scoping.
    pub(crate) fn with_executor(
        name: &str,
        key_prefix: &str,
        executor: Arc<dyn RepoCommandExecutor>,
    ) -> Result<Self, CamelError> {
        validate_namespace_token("repository name", name)?;
        validate_namespace_token("key_prefix", key_prefix)?;
        Ok(Self {
            name: name.to_string(),
            key_prefix: key_prefix.to_string(),
            executor,
        })
    }
}

#[async_trait]
impl IdempotentRepository for RedisIdempotentRepository {
    fn name(&self) -> &str {
        &self.name
    }

    /// Insert `key` if absent — ONE `SET <key> 1 NX`, never retried.
    ///
    /// # Contract C1 — an unknown outcome surfaces as `Err`
    ///
    /// A transient transport failure (for example a response lost during a
    /// sentinel failover) leaves the outcome of the `SET NX` unknown: the
    /// write may already have been applied when the connection dropped.
    /// Re-issuing the `SET NX` is forbidden — the retry would hit the key
    /// inserted by the first attempt and reply Nil, so the caller would
    /// receive `Ok(false)` ("already processed") for a message whose first
    /// insert actually succeeded, and the Idempotent Consumer would
    /// silently skip a message it never processed.
    ///
    /// Therefore the command is issued exactly once per call: on a
    /// transient failure `add` returns `Err(CamelError::Io)` immediately
    /// and refreshes the connection (best-effort, ignoring refresh
    /// errors) so the NEXT call starts healthy. `Ok(true)`/`Ok(false)`
    /// are returned only when the server's answer is actually known.
    async fn add(&self, key: &str) -> Result<bool, CamelError> {
        let mut cmd = redis::Cmd::new();
        cmd.arg("SET")
            .arg(namespaced(&self.key_prefix, &self.name, key))
            .arg(1)
            .arg("NX");
        match self.executor.execute(cmd).await {
            // RESP2 +OK arrives as the dedicated `Value::Okay` status
            // variant (redis 1.x never maps +OK to SimpleString); RESP3
            // may return it as a bulk string.
            Ok(redis::Value::Okay) => Ok(true),
            Ok(redis::Value::SimpleString(s)) if s == "OK" => Ok(true),
            Ok(redis::Value::BulkString(ref bytes)) if bytes.as_slice() == b"OK" => Ok(true),
            // NX not applied: the key already existed.
            Ok(redis::Value::Nil) => Ok(false),
            Ok(other) => Err(CamelError::Io(format!(
                "unexpected SET NX reply: {other:?}"
            ))),
            Err(err) if is_transient_redis_error(&err) => {
                // Refresh for the NEXT call only — the outcome-bearing
                // SET NX is never re-issued (see the doc comment above).
                let _ = self.executor.refresh().await;
                Err(err)
            }
            Err(err) => Err(err),
        }
    }

    async fn contains(&self, key: &str) -> Result<bool, CamelError> {
        let mut cmd = redis::Cmd::new();
        cmd.arg("EXISTS")
            .arg(namespaced(&self.key_prefix, &self.name, key));
        // EXISTS is a safe-to-re-issue read, so the retry-safe path is
        // correct here — unlike add (see add's doc comment).
        match execute_retry_safe(&self.executor, cmd).await? {
            redis::Value::Int(n) => Ok(n > 0),
            other => Err(CamelError::Io(format!(
                "unexpected EXISTS reply: {other:?}"
            ))),
        }
    }

    async fn remove(&self, key: &str) -> Result<(), CamelError> {
        let mut cmd = redis::Cmd::new();
        cmd.arg("UNLINK")
            .arg(namespaced(&self.key_prefix, &self.name, key));
        // UNLINK is idempotent, so the retry-safe path is correct here.
        match execute_retry_safe(&self.executor, cmd).await? {
            redis::Value::Nil | redis::Value::Int(_) => Ok(()),
            other => Err(CamelError::Io(format!(
                "unexpected UNLINK reply: {other:?}"
            ))),
        }
    }

    async fn clear(&self) -> Result<(), CamelError> {
        scan_unlink_pattern(
            &self.executor,
            &format!("{}:{}:*", self.key_prefix, self.name),
        )
        .await?;
        Ok(())
    }
}

// The IdempotentRepository trait requires Debug, but the executor is not
// Debug (and must never leak transport details into logs).
impl std::fmt::Debug for RedisIdempotentRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisIdempotentRepository")
            .field("name", &self.name)
            .field("key_prefix", &self.key_prefix)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::RedisIdempotentRepository;
    use crate::executor::FakeRepoExecutor;
    use crate::executor::test_support::any_arg_contains;
    use crate::executor::test_support::arg_after;
    use crate::executor::test_support::cmd_args;
    use crate::executor::test_support::scan_reply;
    use camel_api::CamelError;
    use camel_api::IdempotentRepository;
    use std::sync::Arc;

    fn repo(executor: Arc<FakeRepoExecutor>) -> RedisIdempotentRepository {
        RedisIdempotentRepository::with_executor("default", "camel:idem", executor)
            .expect("valid constructor arguments")
    }

    #[tokio::test]
    async fn add_is_atomic_insert_if_absent() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(fake.clone());
        fake.push_result(Ok(redis::Value::SimpleString("OK".into())));
        fake.push_result(Ok(redis::Value::Nil));

        assert!(
            repo.add("msg-1").await.expect("first add inserts the key"),
            "applied SET NX means newly added"
        );
        assert!(
            !repo
                .add("msg-1")
                .await
                .expect("second add observes the key"),
            "Nil means already present"
        );

        let commands = fake.commands();
        assert_eq!(commands.len(), 2, "one SET NX per add call");
        assert_eq!(
            cmd_args(&commands[0]),
            vec![
                b"SET".to_vec(),
                b"camel:idem:default:msg-1".to_vec(),
                b"1".to_vec(),
                b"NX".to_vec(),
            ],
            "add must issue SET <namespaced key> 1 NX"
        );
    }

    /// redis 1.x maps the RESP2 `+OK` status reply to the dedicated
    /// `Value::Okay` variant — the shape a live connection produces.
    /// Found by the camel-test live suite.
    #[tokio::test]
    async fn add_accepts_resp2_okay_variant() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(fake.clone());
        fake.push_result(Ok(redis::Value::Okay));

        assert!(
            repo.add("msg-1").await.expect("Okay variant means applied"),
            "RESP2 +OK (Value::Okay) must report newly added"
        );
    }

    #[tokio::test]
    async fn contains_remove_roundtrip() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(fake.clone());
        fake.push_result(Ok(redis::Value::Int(1)));
        fake.push_result(Ok(redis::Value::Int(1)));
        fake.push_result(Ok(redis::Value::Int(0)));

        assert!(
            repo.contains("msg-1").await.expect("contains must succeed"),
            "EXISTS 1 means present"
        );
        repo.remove("msg-1")
            .await
            .expect("remove must succeed even via UNLINK");
        assert!(
            !repo
                .contains("msg-1")
                .await
                .expect("re-contains must succeed"),
            "EXISTS 0 means absent after remove"
        );
    }

    #[tokio::test]
    async fn clear_scoped_to_idempotent_prefix() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(fake.clone());
        // SCAN reply shaped like a real server response (see
        // `test_support::scan_reply` for why it is hand-built).
        fake.push_result(Ok(scan_reply(0, &["camel:idem:default:a"])));
        fake.push_result(Ok(redis::Value::Int(1)));

        repo.clear().await.expect("clear must succeed");

        let commands = fake.commands();
        assert_eq!(commands.len(), 2, "one SCAN then one UNLINK");
        let scan_args = cmd_args(&commands[0]);
        assert_eq!(scan_args[0], b"SCAN".to_vec());
        assert_eq!(
            arg_after(&scan_args, b"MATCH"),
            b"camel:idem:default:*".to_vec(),
            "clear must SCAN only this repository's keyspace"
        );
        let unlink_args = cmd_args(&commands[1]);
        assert_eq!(unlink_args[0], b"UNLINK".to_vec());
        assert_eq!(
            unlink_args,
            vec![b"UNLINK".to_vec(), b"camel:idem:default:a".to_vec()],
            "UNLINK contains only the scanned key"
        );
        assert!(
            !any_arg_contains(&commands, "camel:cache:default:b"),
            "clear must not touch other repositories' keys"
        );
        assert!(
            !any_arg_contains(&commands, "FLUSHDB") && !any_arg_contains(&commands, "FLUSHALL"),
            "clear must never issue FLUSHDB/FLUSHALL"
        );
    }

    #[test]
    fn idempotent_constructor_rejects_glob_name() {
        let err = RedisIdempotentRepository::with_executor(
            "my*idem",
            "camel:idem",
            Arc::new(FakeRepoExecutor::new()),
        )
        .expect_err("glob metacharacters in the repository name must fail construction");
        match &err {
            CamelError::Config(message) => assert!(
                message.contains("repository name"),
                "error must name the invalid token kind, got: {message}"
            ),
            other => panic!("expected CamelError::Config, got: {other}"),
        }
    }

    #[tokio::test]
    async fn transient_failure_returns_err_not_ok() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(fake.clone());
        fake.push_result(Err(CamelError::Io(
            "connection timed out during failover".into(),
        )));

        match repo.add("k").await {
            Err(CamelError::Io(_)) => {}
            other => panic!("unknown SET NX outcome must surface as Io, never Ok, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn add_never_reissues_set_nx_after_lost_response() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(fake.clone());
        fake.push_result(Err(CamelError::Io("connection reset by peer".into())));

        match repo.add("k").await {
            Err(CamelError::Io(_)) => {}
            other => panic!("lost response must surface as Io, got: {other:?}"),
        }
        assert_eq!(
            fake.execute_count(),
            1,
            "exactly one SET NX issued — never re-issued"
        );
        assert_eq!(
            fake.refresh_count(),
            1,
            "connection refreshed once so the NEXT call starts healthy"
        );

        // The refreshed executor serves the next call: a fresh scripted
        // success comes back through the same repository.
        fake.push_result(Ok(redis::Value::SimpleString("OK".into())));
        assert!(
            repo.add("k2")
                .await
                .expect("next add must use the refreshed executor"),
            "the subsequent add reports newly added"
        );
    }

    #[tokio::test]
    async fn contains_retries_once_retry_safe() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(fake.clone());
        // EXISTS is safe to re-issue, so unlike add it retries once after a
        // transient failure — the asymmetry vs add is intentional (C1).
        fake.push_result(Err(CamelError::Io("connection reset by peer".into())));
        fake.push_result(Ok(redis::Value::Int(1)));

        assert!(
            repo.contains("k")
                .await
                .expect("contains must succeed after one retry"),
            "the retried EXISTS reports the key"
        );
        assert_eq!(fake.execute_count(), 2, "command executed twice");
        assert_eq!(fake.refresh_count(), 1, "connection refreshed once");
    }
}
