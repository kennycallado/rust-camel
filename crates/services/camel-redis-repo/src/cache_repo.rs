//! Redis-backed `CacheRepository` storage core.
//!
//! [`RedisCacheRepository`] owns the namespaced key layout
//! (`{key_prefix}:{name}:{key}`), an injectable [`ClockFn`] for deterministic
//! expiry tests, and the [`RepoCommandExecutor`] transport seam. The storage
//! primitives are inherent methods; the `CacheRepository` trait impl
//! forwards to them.

use crate::connection;
use crate::executor::RepoCommandExecutor;
use crate::executor::execute_retry_safe;
use crate::executor::scan_unlink_pattern;
use crate::namespaced;
use crate::validate_namespace_token;
use camel_api::CamelError;
use camel_api::cache::CacheEntry;
use camel_api::cache::CacheRepository;
use camel_api::cache::CacheStats;
use camel_component_redis::RedisEndpointConfig;
use redis::SetExpiry;
use redis::SetOptions;
use std::sync::Arc;
use std::time::Duration;

/// Injectable wall clock for expiry math and tests.
pub type ClockFn = Arc<dyn Fn() -> std::time::SystemTime + Send + Sync>;

/// The default production clock: [`std::time::SystemTime::now`].
pub fn default_clock() -> ClockFn {
    Arc::new(std::time::SystemTime::now)
}

/// Redis-backed cache repository (see module docs).
pub struct RedisCacheRepository {
    name: String,
    key_prefix: String,
    stale_retention: Duration,
    clock: ClockFn,
    executor: Arc<dyn RepoCommandExecutor>,
    hits: std::sync::atomic::AtomicU64,
    misses: std::sync::atomic::AtomicU64,
}

impl RedisCacheRepository {
    /// Connect to `endpoint` and build the repository.
    ///
    /// Validates `name` and `key_prefix` before any network I/O, then
    /// eagerly connects (one topology resolution — see [`connection`]).
    /// Uses the production clock.
    pub async fn connect(
        name: &str,
        endpoint: &RedisEndpointConfig,
        key_prefix: &str,
        stale_retention: Duration,
    ) -> Result<Self, CamelError> {
        validate_namespace_token("repository name", name)?;
        validate_namespace_token("key_prefix", key_prefix)?;
        let executor = connection::connect_executor(endpoint).await?;
        Self::with_executor(
            name,
            key_prefix,
            stale_retention,
            default_clock(),
            Arc::new(executor),
        )
    }

    /// Test seam: build the repository around an injected executor and clock.
    ///
    /// Sync and network-free; validates both namespace tokens first.
    pub(crate) fn with_executor(
        name: &str,
        key_prefix: &str,
        stale_retention: Duration,
        clock: ClockFn,
        executor: Arc<dyn RepoCommandExecutor>,
    ) -> Result<Self, CamelError> {
        validate_namespace_token("repository name", name)?;
        validate_namespace_token("key_prefix", key_prefix)?;
        Ok(Self {
            name: name.to_string(),
            key_prefix: key_prefix.to_string(),
            stale_retention,
            clock,
            executor,
            hits: std::sync::atomic::AtomicU64::new(0),
            misses: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Store one entry under `key`, applying Redis-side expiry of
    /// `expires_at + stale_retention` so expired-but-retained entries stay
    /// readable for [`Self::peek_stale_entry`].
    ///
    /// The in-band `expires_at` is computed from `ttl` BEFORE serialization
    /// (mirroring the redb implementation). Any overflow — in the clock add
    /// or the EXAT computation — degrades to a plain SET, never a failed
    /// write. One command total.
    pub(crate) async fn set_entry(
        &self,
        key: &str,
        value: CacheEntry,
        ttl: Option<Duration>,
    ) -> Result<(), CamelError> {
        let mut entry = value;
        entry.expires_at = ttl.and_then(|d| (self.clock)().checked_add(d));
        let blob = serde_json::to_vec(&entry)
            .map_err(|e| CamelError::Io(format!("cache serialization: {e}")))?;

        let mut cmd = redis::Cmd::new();
        cmd.arg("SET")
            .arg(namespaced(&self.key_prefix, &self.name, key))
            .arg(blob);
        // EXAT seconds = expires_at + stale_retention since the Unix epoch;
        // every step is checked — overflow means keep the entry forever
        // rather than fail the write.
        let exat_secs = entry
            .expires_at
            .and_then(|t| t.checked_add(self.stale_retention))
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        if let Some(secs) = exat_secs {
            cmd.arg(SetOptions::default().with_expiration(SetExpiry::EXAT(secs)));
        }
        execute_retry_safe(&self.executor, cmd).await.map(|_| ())
    }

    /// Fetch `key`, enforcing the in-band expiry: an entry whose
    /// `expires_at` is in the past counts as a miss even though Redis still
    /// retains it. Transport failures surface as `Err` (Contract C1) —
    /// never a silent miss.
    pub(crate) async fn get_entry(&self, key: &str) -> Result<Option<CacheEntry>, CamelError> {
        let value = self.fetch_entry(key).await?;
        match value {
            None => {
                self.count_miss();
                Ok(None)
            }
            Some(entry) => {
                let expired = entry
                    .expires_at
                    // `<=` boundary matches the redb backend (redb.rs):
                    // at exact equality the entry counts as expired.
                    .map(|e| e <= (self.clock)())
                    .unwrap_or(false);
                if expired {
                    self.count_miss();
                    Ok(None)
                } else {
                    self.count_hit();
                    Ok(Some(entry))
                }
            }
        }
    }

    /// Fetch `key` ignoring the in-band expiry — serves stale-but-retained
    /// entries. Hit/miss counting matches [`Self::get_entry`].
    pub(crate) async fn peek_stale_entry(
        &self,
        key: &str,
    ) -> Result<Option<CacheEntry>, CamelError> {
        let value = self.fetch_entry(key).await?;
        if value.is_some() {
            self.count_hit();
        } else {
            self.count_miss();
        }
        Ok(value)
    }

    /// Unlink (async delete) the namespaced key. One command; a nil or
    /// integer reply both mean success.
    pub(crate) async fn invalidate_key(&self, key: &str) -> Result<(), CamelError> {
        let mut cmd = redis::Cmd::new();
        cmd.arg("UNLINK")
            .arg(namespaced(&self.key_prefix, &self.name, key));
        match execute_retry_safe(&self.executor, cmd).await? {
            redis::Value::Nil | redis::Value::Int(_) => Ok(()),
            other => Err(CamelError::Io(format!(
                "unexpected UNLINK reply: {other:?}"
            ))),
        }
    }

    /// Shared GET path for [`Self::get_entry`] and [`Self::peek_stale_entry`]:
    /// one command, deserialize the payload, no expiry filtering.
    async fn fetch_entry(&self, key: &str) -> Result<Option<CacheEntry>, CamelError> {
        let mut cmd = redis::Cmd::new();
        cmd.arg("GET")
            .arg(namespaced(&self.key_prefix, &self.name, key));
        match execute_retry_safe(&self.executor, cmd).await? {
            redis::Value::Nil => Ok(None),
            redis::Value::BulkString(bytes) => {
                let entry: CacheEntry = serde_json::from_slice(&bytes)
                    .map_err(|e| CamelError::Io(format!("cache deserialization: {e}")))?;
                Ok(Some(entry))
            }
            other => Err(CamelError::Io(format!("unexpected GET reply: {other:?}"))),
        }
    }

    fn count_hit(&self) {
        self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn count_miss(&self) {
        self.misses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Atomic snapshot of the hit/miss counters.
    ///
    /// `entries` and `evictions` report 0: Redis owns entry lifetime
    /// (server-side TTL + lazy UNLINK), so the client cannot count either
    /// without a full SCAN; per-key byte totals are equally unreportable.
    pub(crate) async fn stats_snapshot(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(std::sync::atomic::Ordering::Relaxed),
            misses: self.misses.load(std::sync::atomic::Ordering::Relaxed),
            entries: 0,
            evictions: 0,
            ..CacheStats::default()
        }
    }
}

#[async_trait::async_trait]
impl CacheRepository for RedisCacheRepository {
    fn name(&self) -> &str {
        &self.name
    }

    async fn get(&self, key: &str) -> Result<Option<CacheEntry>, CamelError> {
        self.get_entry(key).await
    }

    async fn set(
        &self,
        key: &str,
        value: CacheEntry,
        ttl: Option<Duration>,
    ) -> Result<(), CamelError> {
        self.set_entry(key, value, ttl).await
    }

    async fn peek_stale(&self, key: &str) -> Result<Option<CacheEntry>, CamelError> {
        self.peek_stale_entry(key).await
    }

    async fn invalidate(&self, key: &str) -> Result<(), CamelError> {
        self.invalidate_key(key).await
    }

    async fn clear(&self) -> Result<(), CamelError> {
        scan_unlink_pattern(
            &self.executor,
            &format!("{}:{}:*", self.key_prefix, self.name),
        )
        .await?;
        Ok(())
    }

    async fn invalidate_prefix(&self, prefix: &str) -> Result<u64, CamelError> {
        validate_namespace_token("invalidate_prefix", prefix)?;
        scan_unlink_pattern(
            &self.executor,
            &format!("{}:{}:{}*", self.key_prefix, self.name, prefix),
        )
        .await
    }

    async fn stats(&self) -> CacheStats {
        self.stats_snapshot().await
    }
}

// The CacheRepository trait requires Debug, but the executor and clock are
// not Debug (and must never leak transport details into logs).
impl std::fmt::Debug for RedisCacheRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisCacheRepository")
            .field("name", &self.name)
            .field("key_prefix", &self.key_prefix)
            .field("stale_retention", &self.stale_retention)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::ClockFn;
    use super::RedisCacheRepository;
    use super::default_clock;
    use crate::executor::FakeRepoExecutor;
    use crate::executor::test_support::any_arg_contains;
    use crate::executor::test_support::arg_after;
    use crate::executor::test_support::cmd_args;
    use crate::executor::test_support::scan_reply;
    use camel_api::CamelError;
    use camel_api::cache::CacheEntry;
    use camel_api::cache::ContentType;
    use std::sync::Arc;
    use std::time::Duration;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    /// Deterministic test epoch: 2023-11-14T22:13:20Z.
    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    fn fixed_clock(t: SystemTime) -> ClockFn {
        Arc::new(move || t)
    }

    fn plain_entry() -> CacheEntry {
        CacheEntry {
            bytes: vec![1, 2, 3],
            payload_path: None,
            content_type: ContentType::Bytes,
            expires_at: None,
        }
    }

    fn repo(clock: ClockFn, executor: Arc<FakeRepoExecutor>) -> RedisCacheRepository {
        RedisCacheRepository::with_executor(
            "default",
            "camel:cache",
            Duration::from_secs(10),
            clock,
            executor,
        )
        .expect("valid constructor arguments")
    }

    #[test]
    fn cache_constructor_rejects_glob_name() {
        let err = RedisCacheRepository::with_executor(
            "my*cache",
            "camel:cache",
            Duration::from_secs(1),
            default_clock(),
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

    #[test]
    fn cache_constructor_rejects_empty_prefix() {
        let result = RedisCacheRepository::with_executor(
            "default",
            "",
            Duration::from_secs(1),
            default_clock(),
            Arc::new(FakeRepoExecutor::new()),
        );
        assert!(result.is_err(), "empty key_prefix must fail construction");
    }

    #[tokio::test]
    async fn set_get_roundtrip() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(default_clock(), fake.clone());
        let entry = plain_entry();

        repo.set_entry("k", entry.clone(), None)
            .await
            .expect("set without ttl must succeed");
        fake.push_result(Ok(redis::Value::BulkString(
            serde_json::to_vec(&entry).expect("entry serializes"),
        )));
        let got = repo.get_entry("k").await.expect("get must succeed");

        assert_eq!(got, Some(entry));
        assert_eq!(fake.commands().len(), 2, "one SET then one GET command");
        let set_args = cmd_args(&fake.commands()[0]);
        assert_eq!(set_args[0], b"SET".to_vec());
        assert_eq!(set_args[1], b"camel:cache:default:k".to_vec());
    }

    #[tokio::test]
    async fn exat_applied_only_with_expiry() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(fixed_clock(now()), fake.clone());
        let entry = plain_entry();

        repo.set_entry("k", entry.clone(), Some(Duration::from_secs(30)))
            .await
            .expect("set with ttl must succeed");
        repo.set_entry("k", entry, None)
            .await
            .expect("set without ttl must succeed");

        let commands = fake.commands();
        assert_eq!(commands.len(), 2, "each set is ONE command");
        let with_ttl = cmd_args(&commands[0]);
        let without_ttl = cmd_args(&commands[1]);

        // EXAT = expires_at (now+30) + stale_retention (10) = now+40.
        let expected_exat = (now() + Duration::from_secs(40))
            .duration_since(UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs();
        let exat_at = with_ttl
            .iter()
            .position(|arg| arg == b"EXAT")
            .expect("first SET carries EXAT");
        assert_eq!(with_ttl[exat_at + 1], expected_exat.to_string().as_bytes());
        let stored: CacheEntry =
            serde_json::from_slice(&with_ttl[2]).expect("stored blob is a CacheEntry");
        assert_eq!(stored.expires_at, Some(now() + Duration::from_secs(30)));

        assert!(
            !without_ttl.iter().any(|arg| arg == b"EXAT"),
            "SET without ttl must not carry EXAT: {without_ttl:?}"
        );
    }

    #[tokio::test]
    async fn in_band_expiry_enforced_on_get_peek_stale_still_reads() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(fixed_clock(now()), fake.clone());
        let expired = CacheEntry {
            bytes: vec![9],
            payload_path: None,
            content_type: ContentType::Text,
            expires_at: Some(now() - Duration::from_secs(1)),
        };
        let blob = serde_json::to_vec(&expired).expect("entry serializes");

        fake.push_result(Ok(redis::Value::BulkString(blob.clone())));
        let got = repo.get_entry("k").await.expect("get must succeed");
        assert_eq!(got, None, "expired entry must read as a miss on get");

        fake.push_result(Ok(redis::Value::BulkString(blob)));
        let peeked = repo
            .peek_stale_entry("k")
            .await
            .expect("peek_stale must succeed");
        assert_eq!(
            peeked,
            Some(expired),
            "peek_stale ignores in-band expiry while the key is retained"
        );
    }

    #[tokio::test]
    async fn get_err_on_transient_never_silent_miss() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(default_clock(), fake.clone());
        // Persistent failure: transient on both the first attempt and the
        // post-refresh retry, so the error still surfaces (never a miss).
        fake.push_result(Err(CamelError::Io("connection refused".into())));
        fake.push_result(Err(CamelError::Io("connection refused".into())));

        match repo.get_entry("k").await {
            Err(CamelError::Io(_)) => {}
            other => panic!("backend failure must surface as Io, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn invalidate_unlinks_namespaced_key() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(default_clock(), fake.clone());

        repo.invalidate_key("k")
            .await
            .expect("invalidate must succeed");

        let commands = fake.commands();
        assert_eq!(commands.len(), 1, "invalidate is ONE command");
        assert_eq!(
            cmd_args(&commands[0]),
            vec![b"UNLINK".to_vec(), b"camel:cache:default:k".to_vec()],
            "must UNLINK the namespaced key"
        );
    }

    fn transient(message: &str) -> CamelError {
        CamelError::Io(message.into())
    }

    #[tokio::test]
    async fn set_retries_once_after_transient() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(default_clock(), fake.clone());
        fake.push_result(Err(transient("connection reset by peer")));
        fake.push_result(Ok(redis::Value::SimpleString("OK".into())));

        repo.set_entry("k", plain_entry(), None)
            .await
            .expect("set must succeed after one transient failure");

        assert_eq!(fake.execute_count(), 2, "command executed twice");
        assert_eq!(fake.refresh_count(), 1, "connection refreshed once");
        let commands = fake.commands();
        assert_eq!(commands.len(), 2);
        assert_eq!(
            cmd_args(&commands[0]),
            cmd_args(&commands[1]),
            "the retry re-issues the SAME command"
        );
    }

    #[tokio::test]
    async fn get_retries_once_then_succeeds() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(default_clock(), fake.clone());
        let entry = plain_entry();
        fake.push_result(Err(transient("connection reset by peer")));
        fake.push_result(Ok(redis::Value::BulkString(
            serde_json::to_vec(&entry).expect("entry serializes"),
        )));

        let got = repo
            .get_entry("k")
            .await
            .expect("get must succeed after one transient failure");

        assert_eq!(got, Some(entry));
        assert_eq!(fake.execute_count(), 2);
        assert_eq!(fake.refresh_count(), 1);
    }

    #[tokio::test]
    async fn no_retry_on_second_failure() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(default_clock(), fake.clone());
        fake.push_result(Err(transient("connection reset by peer")));
        fake.push_result(Err(transient("connection refused")));

        match repo.get_entry("k").await {
            Err(CamelError::Io(_)) => {}
            other => panic!("second failure must surface as Io, got: {other:?}"),
        }
        assert_eq!(fake.execute_count(), 2, "exactly one retry, no more");
        assert_eq!(fake.refresh_count(), 1);
    }

    #[tokio::test]
    async fn non_transient_no_retry() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(default_clock(), fake.clone());
        fake.push_result(Err(CamelError::Config("non-transient".into())));

        assert!(
            repo.get_entry("k").await.is_err(),
            "non-transient error must surface immediately"
        );
        assert_eq!(fake.execute_count(), 1, "no retry on non-transient error");
        assert_eq!(fake.refresh_count(), 0, "no refresh on non-transient error");
    }

    use camel_api::cache::CacheRepository;

    #[tokio::test]
    async fn clear_scoped_to_repository_prefix() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(default_clock(), fake.clone());
        fake.push_result(Ok(scan_reply(0, &["camel:cache:default:a"])));
        fake.push_result(Ok(redis::Value::Int(1)));

        repo.clear().await.expect("clear must succeed");

        let commands = fake.commands();
        assert_eq!(commands.len(), 2, "one SCAN then one UNLINK");
        let scan_args = cmd_args(&commands[0]);
        assert_eq!(scan_args[0], b"SCAN".to_vec());
        assert_eq!(
            arg_after(&scan_args, b"MATCH"),
            b"camel:cache:default:*".to_vec(),
            "clear must SCAN only this repository's keyspace"
        );
        let unlink_args = cmd_args(&commands[1]);
        assert_eq!(unlink_args[0], b"UNLINK".to_vec());
        assert_eq!(
            unlink_args,
            vec![b"UNLINK".to_vec(), b"camel:cache:default:a".to_vec()],
            "UNLINK contains only the scanned key"
        );
        assert!(
            !any_arg_contains(&commands, "FLUSHDB") && !any_arg_contains(&commands, "FLUSHALL"),
            "clear must never issue FLUSHDB/FLUSHALL"
        );
        assert!(
            !any_arg_contains(&commands, "camel:idem:default:b"),
            "clear must not touch other repositories' keys"
        );
    }

    #[tokio::test]
    async fn invalidate_prefix_purges_one_namespace_and_guards_step_prefix() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(default_clock(), fake.clone());
        fake.push_result(Ok(scan_reply(0, &["camel:cache:default:ns:a"])));
        fake.push_result(Ok(redis::Value::Int(1)));

        let removed = repo
            .invalidate_prefix("ns:")
            .await
            .expect("invalidate_prefix must succeed");

        assert_eq!(removed, 1, "removed count comes from the UNLINK reply");
        let commands = fake.commands();
        assert_eq!(commands.len(), 2, "one SCAN then one UNLINK");
        let scan_args = cmd_args(&commands[0]);
        assert_eq!(
            arg_after(&scan_args, b"MATCH"),
            b"camel:cache:default:ns:*".to_vec(),
            "SCAN must target exactly the requested namespace"
        );
        assert_eq!(
            cmd_args(&commands[1]),
            vec![b"UNLINK".to_vec(), b"camel:cache:default:ns:a".to_vec()],
            "UNLINK contains only the namespace key"
        );
        assert!(
            !any_arg_contains(&commands, "camel:cache:default:other:b"),
            "sibling namespaces must not be touched"
        );

        // Glob metacharacters are rejected before ANY command is issued.
        let err = repo
            .invalidate_prefix("ns*")
            .await
            .expect_err("glob metacharacter in prefix must be rejected");
        assert!(
            matches!(err, CamelError::Config(_)),
            "expected Config error, got: {err}"
        );
        assert_eq!(
            fake.execute_count(),
            2,
            "the rejected prefix must not trigger a SCAN"
        );
    }

    #[tokio::test]
    async fn stats_one_hit_one_miss() {
        let fake = Arc::new(FakeRepoExecutor::new());
        let repo = repo(default_clock(), fake.clone());
        let entry = plain_entry();
        fake.push_result(Ok(redis::Value::BulkString(
            serde_json::to_vec(&entry).expect("entry serializes"),
        )));
        fake.push_result(Ok(redis::Value::Nil));

        assert_eq!(
            repo.get("k").await.expect("hit get"),
            Some(entry),
            "warm key is a hit"
        );
        assert_eq!(repo.get("x").await.expect("miss get"), None);

        let stats = repo.stats().await;
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.entries, 0);
        assert_eq!(stats.evictions, 0);
    }
}
