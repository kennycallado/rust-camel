//! Eager-predecessor-reclaim tests for the disk-payload offload
//! decorator (OpenSpec `cachereclaim`). Shared fixtures live in
//! [`super::disk_offload_tests`]; this module owns the reclaim behavior
//! tests — happy path here, failure paths and guards too.

use super::*;
use crate::cache::MemoryCacheRepository;
use async_trait::async_trait;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use tempfile::tempdir;

use super::disk_offload_tests::{
    MAX_TTL, RETENTION, SWEEP, capture_warns, dir_names, entry, fixed_clock, inner_repo, new_repo,
    new_repo_dyn,
};

// ── eager predecessor reclaim on overwrite ───────────────────────────────

/// Overwriting a key bounds on-disk blob versions at ONE (the fresh blob):
/// the predecessor is eagerly reclaimed inside the overwrite `set()`.
/// Expected NOW (no eager reclaim yet): FAIL — both blobs coexist.
/// After the reclaim implementation: PASS.
#[tokio::test]
async fn overwrite_reclaims_predecessor_bounds_versions_at_one() {
    let dir = tempdir().expect("tempdir");
    let inner: Arc<dyn CacheRepository> = inner_repo();
    // Fixed clock for deterministic death epochs; a huge sweep interval
    // keeps the real-clock background sweeper out of the test's way.
    let clock = fixed_clock(UNIX_EPOCH + Duration::from_secs(1_800_000_000));
    let repo = DiskOffloadRepository::with_clock(
        inner,
        dir.path().to_path_buf(),
        RETENTION,
        Duration::from_secs(30 * 24 * 3600),
        MAX_TTL,
        CancellationToken::new(),
        clock,
    );

    let payload_a = vec![b'A'; 128];
    let payload_b = vec![b'B'; 256];
    repo.set("k", entry(payload_a, ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set A");
    repo.set(
        "k",
        entry(payload_b.clone(), ContentType::Bytes),
        Some(SWEEP),
    )
    .await
    .expect("set B");

    let names = dir_names(dir.path());
    assert_eq!(
        names.len(),
        1,
        "exactly one blob expected after overwrite, got {names:?}"
    );
    let got = repo.get("k").await.expect("get").expect("present");
    assert_eq!(got.bytes, payload_b, "second payload must be served");
}

/// A reader holding the pre-swap index row while an overwrite reclaims the
/// predecessor must hydrate to `Ok(None)` MISS with WARN — never `Err`.
/// Expected NOW: FAIL — the old blob is still on disk, so `hydrate`
/// returns `Ok(Some(A))`. After the reclaim implementation: PASS.
#[tokio::test]
async fn overwrite_reclaim_reader_race_miss_not_error() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let (warns, _guard) = capture_warns();
    let repo = new_repo(
        Arc::clone(&inner),
        dir.path().to_path_buf(),
        default_offload_clock(),
    );

    let payload_a = vec![b'A'; 128];
    let payload_b = vec![b'B'; 256];
    repo.set("k", entry(payload_a, ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set A");

    // Reader captures the pre-swap row: payload_path names the old blob.
    let captured = inner
        .get("k")
        .await
        .expect("inner get")
        .expect("row present");
    assert!(
        captured.payload_path.is_some(),
        "pre-swap row must carry the offloaded payload path"
    );

    repo.set(
        "k",
        entry(payload_b.clone(), ContentType::Bytes),
        Some(SWEEP),
    )
    .await
    .expect("set B");

    // Hydrating the stale row must be a clean MISS, never an error.
    let hydrated = repo
        .hydrate("k", captured)
        .await
        .expect("hydrate must never Err");
    assert_eq!(
        hydrated, None,
        "reclaimed predecessor must turn the stale row into a MISS"
    );
    assert!(
        warns.lock().iter().any(|m| m.contains("blob")),
        "expected a WARN about the reclaimed blob, got {:?}",
        warns.lock()
    );

    let names = dir_names(dir.path());
    assert_eq!(
        names.len(),
        1,
        "exactly one blob expected after overwrite, got {names:?}"
    );
    let got = repo.get("k").await.expect("get").expect("present");
    assert_eq!(got.bytes, payload_b, "second payload must be served");
}

/// The reclaim's targeted unlink must never touch foreign files or another
/// key's blob named by a corrupt row (key-ownership guard).
/// Expected NOW: PASS — nothing is unlinked today. Must stay PASS after
/// the reclaim implementation.
#[tokio::test]
async fn reclaim_never_unlinks_foreign_or_other_key_blobs() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let repo = new_repo(
        Arc::clone(&inner),
        dir.path().to_path_buf(),
        default_offload_clock(),
    );

    repo.set("k1", entry(vec![b'1'; 32], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set k1");
    let b1 = dir_names(dir.path()).into_iter().next().expect("b1");
    repo.set("k2", entry(vec![b'2'; 32], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set k2");
    let b2 = dir_names(dir.path())
        .into_iter()
        .find(|n| n != &b1)
        .expect("b2");

    // Foreign files: an operator artifact and a k1-prefixed name whose
    // "epoch" component does not parse.
    std::fs::write(dir.path().join("readme.txt"), b"operator artifact").expect("write readme");
    let garbage = format!("{}.garbage.blob", blake3_128hex(b"k1"));
    std::fs::write(dir.path().join(&garbage), b"garbage").expect("write garbage");

    // Corruption 1: k1's row names k2's blob — sanitize-passing,
    // epoch-bearing, but the WRONG key prefix.
    inner
        .set(
            "k1",
            CacheEntry {
                bytes: Vec::new(),
                payload_path: Some(b2),
                content_type: ContentType::Bytes,
                expires_at: None,
            },
            None,
        )
        .await
        .expect("corrupt k1 row toward b2");
    repo.set("k1", entry(vec![b'3'; 32], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("overwrite k1 (fresh b3)");
    let names = dir_names(dir.path());
    assert_eq!(
        names.len(),
        5,
        "b1 orphan, b2 live, b3 fresh, readme, garbage expected, got {names:?}"
    );

    // Corruption 2: k1's row names the garbage file.
    inner
        .set(
            "k1",
            CacheEntry {
                bytes: Vec::new(),
                payload_path: Some(garbage.clone()),
                content_type: ContentType::Bytes,
                expires_at: None,
            },
            None,
        )
        .await
        .expect("corrupt k1 row toward garbage");
    repo.set("k1", entry(vec![b'4'; 32], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("overwrite k1 (fresh b4)");
    let names = dir_names(dir.path());
    assert_eq!(
        names.len(),
        6,
        "b1..b4, readme, garbage expected, got {names:?}"
    );
    assert!(
        names.contains(&garbage),
        "garbage file must survive the reclaim, got {names:?}"
    );
    let got_k1 = repo.get("k1").await.expect("get k1").expect("k1 present");
    assert_eq!(
        got_k1.bytes,
        vec![b'4'; 32],
        "k1 must serve the last payload"
    );
    let got_k2 = repo.get("k2").await.expect("get k2").expect("k2 present");
    assert_eq!(
        got_k2.bytes,
        vec![b'2'; 32],
        "k2's blob must be untouched by k1's overwrites"
    );
}

/// A same-second identical rewrite produces the SAME destination filename
/// (same death epoch + fingerprint); the fresh blob must survive the
/// overwrite via the equal-name guard.
/// Expected: PASS now (no reclaim yet) and after the implementation.
#[tokio::test]
async fn same_second_identical_rewrite_keeps_blob() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    // Frozen clock: both writes land in the same death-epoch second.
    let clock = fixed_clock(UNIX_EPOCH + Duration::from_secs(1_800_000_000));
    let repo = new_repo(Arc::clone(&inner), dir.path().to_path_buf(), clock);

    let payload = vec![b'P'; 128];
    repo.set("k", entry(payload.clone(), ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set 1");
    repo.set("k", entry(payload.clone(), ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set 2");

    let names = dir_names(dir.path());
    assert_eq!(
        names.len(),
        1,
        "identical same-second rewrite must keep one blob, got {names:?}"
    );
    let got = repo.get("k").await.expect("get").expect("present");
    assert_eq!(got.bytes, payload, "payload must round-trip");
}

/// Two identical writes one second apart differ in the death-epoch name
/// component (filename changes), so the overwrite must reclaim the old
/// blob. Expected NOW: FAIL — two blobs coexist. After the reclaim
/// implementation: PASS.
#[tokio::test]
async fn same_content_next_second_reclaims_old_blob() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    // Injectable clock seam, dated far beyond the real-clock sweeper's
    // horizon so nothing is reclaimed behind the test's back.
    let t0 = UNIX_EPOCH + Duration::from_secs(1_900_000_000);
    let now = Arc::new(Mutex::new(t0));
    let clock: OffloadClock = {
        let now = Arc::clone(&now);
        Arc::new(move || *now.lock())
    };
    let repo = new_repo(Arc::clone(&inner), dir.path().to_path_buf(), clock);

    let payload = vec![b'P'; 128];
    repo.set("k", entry(payload.clone(), ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set 1");
    // Advance exactly one second: the death epoch changes, so does the
    // destination filename.
    *now.lock() = t0 + Duration::from_secs(1);
    repo.set("k", entry(payload.clone(), ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set 2");

    let names = dir_names(dir.path());
    assert_eq!(
        names.len(),
        1,
        "one blob expected after the next-second rewrite, got {names:?}"
    );
    let got = repo.get("k").await.expect("get").expect("present");
    assert_eq!(got.bytes, payload, "payload must round-trip");
}

/// An expired-but-retained row is invisible to the backend's `get`, so an
/// overwrite must NOT eagerly reclaim its blob — the sweeper owns it at
/// its death epoch.
/// Expected: PASS now (no reclaim yet) and after the implementation.
#[tokio::test]
async fn expired_retained_row_not_eagerly_reclaimed() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    // Fixed decorator clock: deterministic far-future death epochs. The
    // inner memory backend computes expiry on its own real clock, so a
    // tiny REAL ttl expires the row for `get` while `peek_stale` still
    // serves it within stale retention.
    let clock = fixed_clock(UNIX_EPOCH + Duration::from_secs(1_800_000_000));
    let repo = new_repo(Arc::clone(&inner), dir.path().to_path_buf(), clock);

    let payload_a = vec![b'A'; 32];
    let payload_b = vec![b'B'; 64];
    repo.set(
        "k",
        entry(payload_a, ContentType::Bytes),
        Some(Duration::from_millis(20)),
    )
    .await
    .expect("set A");
    let b1 = dir_names(dir.path())
        .into_iter()
        .next()
        .expect("first blob");

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        repo.get("k").await.expect("get"),
        None,
        "row must be expired and invisible to get"
    );
    assert!(
        repo.peek_stale("k").await.expect("peek_stale").is_some(),
        "row must still be retained (peekable past expiry)"
    );

    repo.set(
        "k",
        entry(payload_b.clone(), ContentType::Bytes),
        Some(SWEEP),
    )
    .await
    .expect("set B");

    let names = dir_names(dir.path());
    assert!(
        names.contains(&b1),
        "expired-but-retained row's blob must not be eagerly reclaimed, got {names:?}"
    );
    let got = repo.get("k").await.expect("get").expect("present");
    assert_eq!(got.bytes, payload_b, "second payload must be served");
}

/// An inline predecessor row (payload_path = None — legacy or a prior
/// inline fallback) has no blob to reclaim: the overwrite behaves exactly
/// as a first write — no unlink attempted, no WARN.
/// Expected: PASS now and after the implementation.
#[tokio::test]
async fn inline_row_predecessor_no_unlink() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let (warns, _guard) = capture_warns();
    let repo = new_repo(
        Arc::clone(&inner),
        dir.path().to_path_buf(),
        default_offload_clock(),
    );

    // Legacy/inline row written directly into the index: full bytes,
    // payload_path = None, no blob file.
    inner
        .set("k", entry(vec![b'L'; 32], ContentType::Bytes), None)
        .await
        .expect("inject inline row");

    repo.set("k", entry(vec![b'N'; 64], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("overwrite k");

    let names = dir_names(dir.path());
    assert_eq!(
        names.len(),
        1,
        "exactly the fresh blob expected, got {names:?}"
    );
    let got = repo.get("k").await.expect("get").expect("present");
    assert_eq!(got.bytes, vec![b'N'; 64], "new payload must be served");
    assert!(
        warns.lock().is_empty(),
        "no reclaim WARN expected for an inline predecessor, got {:?}",
        warns.lock()
    );
}

// ── reclaim failure paths & guards ───────────────────────────────────────

/// Inner backend whose `get` fails once [`Self::fail_get`] is set: models
/// a backend failure at the reclaim's pre-swap row read. `set` always
/// delegates unchanged.
#[derive(Debug)]
struct FailGetRepo {
    inner: Arc<MemoryCacheRepository>,
    fail_get: AtomicBool,
}

#[async_trait]
impl CacheRepository for FailGetRepo {
    fn name(&self) -> &str {
        "fail-get-memory"
    }

    async fn get(&self, key: &str) -> Result<Option<CacheEntry>, CamelError> {
        if self.fail_get.load(Ordering::SeqCst) {
            return Err(CamelError::Io(
                "fail-get-memory: injected get failure".into(),
            ));
        }
        self.inner.get(key).await
    }

    async fn set(
        &self,
        key: &str,
        value: CacheEntry,
        ttl: Option<Duration>,
    ) -> Result<(), CamelError> {
        self.inner.set(key, value, ttl).await
    }

    async fn peek_stale(&self, key: &str) -> Result<Option<CacheEntry>, CamelError> {
        self.inner.peek_stale(key).await
    }

    async fn invalidate(&self, key: &str) -> Result<(), CamelError> {
        self.inner.invalidate(key).await
    }

    async fn clear(&self) -> Result<(), CamelError> {
        self.inner.clear().await
    }

    async fn invalidate_prefix(&self, prefix: &str) -> Result<u64, CamelError> {
        self.inner.invalidate_prefix(prefix).await
    }

    async fn stats(&self) -> CacheStats {
        self.inner.stats().await
    }
}

/// Inner backend whose `set` always fails: models the decorated backend
/// rejecting the index write after the blob was written. The reclaim must
/// be skipped and the inner error surfaced unchanged.
#[derive(Debug)]
struct FailSetRepo {
    inner: Arc<MemoryCacheRepository>,
}

#[async_trait]
impl CacheRepository for FailSetRepo {
    fn name(&self) -> &str {
        "fail-set-memory"
    }

    async fn get(&self, key: &str) -> Result<Option<CacheEntry>, CamelError> {
        self.inner.get(key).await
    }

    async fn set(
        &self,
        _key: &str,
        _value: CacheEntry,
        _ttl: Option<Duration>,
    ) -> Result<(), CamelError> {
        Err(CamelError::Io(
            "fail-set-memory: injected set failure".into(),
        ))
    }

    async fn peek_stale(&self, key: &str) -> Result<Option<CacheEntry>, CamelError> {
        self.inner.peek_stale(key).await
    }

    async fn invalidate(&self, key: &str) -> Result<(), CamelError> {
        self.inner.invalidate(key).await
    }

    async fn clear(&self) -> Result<(), CamelError> {
        self.inner.clear().await
    }

    async fn invalidate_prefix(&self, prefix: &str) -> Result<u64, CamelError> {
        self.inner.invalidate_prefix(prefix).await
    }

    async fn stats(&self) -> CacheStats {
        self.inner.stats().await
    }
}

/// Restores `path` to 0o755 on drop so a failing assert never leaks a
/// read-only temp dir (declared after the [`tempdir::TempDir`], it drops
/// before it).
#[cfg(unix)]
struct RestorePerms<'a>(&'a Path);

#[cfg(unix)]
impl Drop for RestorePerms<'_> {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;

        let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755));
    }
}

/// A pre-swap row read failure (`get` → `Err`) must skip the reclaim with
/// a WARN and never fail the write — the reclaim adds no failure mode.
/// Expected NOW (no pre-swap read exists): FAIL — no such WARN. After the
/// reclaim implementation: PASS.
#[tokio::test]
async fn preswap_row_read_failure_skips_reclaim() {
    let dir = tempdir().expect("tempdir");
    let (warns, _guard) = capture_warns();
    let backend = Arc::new(FailGetRepo {
        inner: inner_repo(),
        fail_get: AtomicBool::new(false),
    });
    let repo = new_repo_dyn(
        backend.clone(),
        dir.path().to_path_buf(),
        default_offload_clock(),
    );

    repo.set("k", entry(vec![b'A'; 128], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set A with the get toggle off");
    let predecessor = dir_names(dir.path())
        .into_iter()
        .next()
        .expect("predecessor blob");

    backend.fail_get.store(true, Ordering::SeqCst);
    repo.set("k", entry(vec![b'B'; 64], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("write must proceed unchanged despite the read failure");

    assert!(
        warns.lock().iter().any(|m| m.contains("pre-swap row read")),
        "expected a WARN about the failed pre-swap row read, got {:?}",
        warns.lock()
    );
    assert!(
        dir_names(dir.path()).contains(&predecessor),
        "reclaim must be skipped: predecessor survives until its death epoch, got {:?}",
        dir_names(dir.path())
    );
}

/// If the decorated backend rejects the index write, the reclaim must be
/// skipped (the surviving row may still reference the predecessor blob)
/// and the inner error surfaced unchanged.
/// Expected NOW: PASS — no reclaim exists yet. Must stay PASS after.
#[tokio::test]
async fn inner_set_failure_skips_reclaim() {
    let dir = tempdir().expect("tempdir");
    let mem = inner_repo();
    let backend = Arc::new(FailSetRepo {
        inner: Arc::clone(&mem),
    });
    // Predecessor blob on disk: a plain decorator over the same memory
    // backend performs the first (successful) offloaded write.
    let plain = new_repo_dyn(mem, dir.path().to_path_buf(), default_offload_clock());
    plain
        .set("k", entry(vec![b'A'; 128], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set A offloaded");
    let predecessor = dir_names(dir.path())
        .into_iter()
        .next()
        .expect("predecessor blob");

    // Second decorator over the failing inner, SAME payload dir: the
    // fresh blob is written, then the index swap fails.
    let repo = new_repo_dyn(backend, dir.path().to_path_buf(), default_offload_clock());
    let result = repo
        .set("k", entry(vec![b'B'; 64], ContentType::Bytes), Some(SWEEP))
        .await;

    let err = result.expect_err("inner set failure must surface");
    assert!(
        err.to_string().contains("fail-set-memory"),
        "the inner error must propagate unchanged, got {err}"
    );
    assert!(
        dir_names(dir.path()).contains(&predecessor),
        "predecessor blob must NOT be unlinked when the inner set fails, got {:?}",
        dir_names(dir.path())
    );
}

/// An unlink failure (EACCES via a read-only dir) must never turn `set`
/// into `Err`: at least one WARN is captured, the predecessor survives
/// until its death epoch, and `get` serves the FULL inline bytes.
/// Expected NOW: PASS — the blob-write failure already degrades inline.
/// Must stay PASS after (the failing reclaim WARNs, never errs).
#[cfg(unix)]
#[tokio::test]
async fn reclaim_unlink_failure_never_fails_set() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let (warns, _guard) = capture_warns();
    let repo = new_repo_dyn(inner, dir.path().to_path_buf(), default_offload_clock());

    repo.set("k", entry(vec![b'A'; 128], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("first set offloads");
    let predecessor = dir_names(dir.path())
        .into_iter()
        .next()
        .expect("predecessor blob");

    // r-x dir: the fresh blob write fails (create denied) AND the
    // predecessor unlink fails (remove denied) — both degraded paths in
    // one overwrite.
    std::fs::set_permissions(dir.path(), PermissionsExt::from_mode(0o500)).expect("chmod 500");
    let _perms = RestorePerms(dir.path());

    let payload_b = vec![b'B'; 64];
    repo.set(
        "k",
        entry(payload_b.clone(), ContentType::Bytes),
        Some(SWEEP),
    )
    .await
    .expect("set must degrade inline, never Err");

    assert!(
        !warns.lock().is_empty(),
        "expected at least one WARN (blob write, then reclaim), got {:?}",
        warns.lock()
    );
    assert!(
        dir_names(dir.path()).contains(&predecessor),
        "unlink-denied predecessor must survive until its death epoch, got {:?}",
        dir_names(dir.path())
    );
    let got = repo.get("k").await.expect("get").expect("present");
    assert_eq!(got.bytes, payload_b, "get must serve the FULL inline bytes");
    assert!(got.payload_path.is_none(), "degraded row stays inline");
}

/// When the blob write degrades to inline storage the predecessor must
/// still be reclaimed: either unlinked (the inline row no longer
/// references it) or — unlink denied — WARNed, leaving it for the sweeper.
/// Expected NOW: FAIL — the predecessor remains AND no "eager reclaim"
/// WARN exists (the fallback path runs no reclaim yet).
#[cfg(unix)]
#[tokio::test]
async fn inline_fallback_reclaims_predecessor() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let (warns, _guard) = capture_warns();
    let repo = new_repo_dyn(inner, dir.path().to_path_buf(), default_offload_clock());

    repo.set("k", entry(vec![b'A'; 128], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("first set offloads");
    let predecessor = dir_names(dir.path())
        .into_iter()
        .next()
        .expect("predecessor blob");

    // Read-only dir: the second write's blob fails → inline fallback, and
    // its reclaim attempt (if any) is denied EACCES.
    std::fs::set_permissions(dir.path(), PermissionsExt::from_mode(0o500)).expect("chmod 500");
    let _perms = RestorePerms(dir.path());

    repo.set("k", entry(vec![b'B'; 64], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("degraded set must return Ok");

    let predecessor_gone = !dir_names(dir.path()).contains(&predecessor);
    let reclaim_warned = warns.lock().iter().any(|m| m.contains("eager reclaim"));
    assert!(
        predecessor_gone || reclaim_warned,
        "predecessor must be unlinked or its failed reclaim WARNed; \
         warns={:?} dir={:?}",
        warns.lock(),
        dir_names(dir.path())
    );
}

/// The equal-name guard must NOT apply on the inline-fallback path: with
/// a frozen clock the retry's destination filename equals the
/// predecessor's, yet the reclaim must still be ATTEMPTED — the read-only
/// dir makes the attempt audible as a WARN, where a wrongly-applied guard
/// would stay silent.
/// Expected NOW: FAIL — no reclaim attempt, hence no WARN.
#[cfg(unix)]
#[tokio::test]
async fn inline_fallback_equal_name_attempts_reclaim() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let (warns, _guard) = capture_warns();
    // Frozen clock: both writes land in the same death-epoch second, so
    // the retry yields the SAME destination filename as the predecessor.
    let clock = fixed_clock(UNIX_EPOCH + Duration::from_secs(1_800_000_000));
    let repo = new_repo_dyn(inner, dir.path().to_path_buf(), clock);

    let payload = vec![b'P'; 128];
    repo.set("k", entry(payload.clone(), ContentType::Bytes), Some(SWEEP))
        .await
        .expect("first set offloads");
    let predecessor = dir_names(dir.path())
        .into_iter()
        .next()
        .expect("predecessor blob");

    std::fs::set_permissions(dir.path(), PermissionsExt::from_mode(0o500)).expect("chmod 500");
    let _perms = RestorePerms(dir.path());

    // Identical payload + ttl: same death epoch + fingerprint → the
    // destination name equals the predecessor's, but the failed write
    // leaves NO fresh blob owning that name.
    repo.set("k", entry(payload.clone(), ContentType::Bytes), Some(SWEEP))
        .await
        .expect("degraded set must return Ok");

    assert!(
        warns.lock().iter().any(|m| m.contains("eager reclaim")),
        "the equal-name reclaim attempt must be made (and WARN on denial), got {:?}",
        warns.lock()
    );
    assert!(
        dir_names(dir.path()).contains(&predecessor),
        "unlink-denied predecessor must remain for the sweeper, got {:?}",
        dir_names(dir.path())
    );
    let got = repo.get("k").await.expect("get").expect("present");
    assert_eq!(got.bytes, payload, "get must serve the inline row's bytes");
}

/// A traversal `payload_path` in the pre-swap row must be rejected by the
/// name guards — no file outside `payload_dir` is ever unlinked, and the
/// fresh blob stays intact.
/// Expected: PASS now (nothing is unlinked) and after the implementation.
#[tokio::test]
async fn reclaim_rejects_traversal_predecessor_name() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let repo = new_repo_dyn(
        inner.clone(),
        dir.path().to_path_buf(),
        default_offload_clock(),
    );

    // Canary in the payload dir's PARENT: `../canary.blob` escapes the dir.
    let canary = dir
        .path()
        .parent()
        .expect("tempdir has a parent")
        .join("canary.blob");
    std::fs::write(&canary, b"do not touch").expect("write canary");

    repo.set("k", entry(vec![b'A'; 128], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("first set offloads");

    // Corrupt the row: payload_path escapes the payload dir.
    inner
        .set(
            "k",
            CacheEntry {
                bytes: Vec::new(),
                payload_path: Some("../canary.blob".into()),
                content_type: ContentType::Bytes,
                expires_at: None,
            },
            None,
        )
        .await
        .expect("corrupt row k toward the canary");

    repo.set("k", entry(vec![b'B'; 64], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("overwrite k");

    assert!(canary.exists(), "the canary must never be unlinked");
    let names = dir_names(dir.path());
    assert_eq!(
        names.len(),
        2,
        "old + fresh blob expected (traversal name never unlinked), got {names:?}"
    );
    let got = repo.get("k").await.expect("get").expect("present");
    assert_eq!(got.bytes, vec![b'B'; 64], "fresh blob must be intact");
    let _ = std::fs::remove_file(&canary);
}
