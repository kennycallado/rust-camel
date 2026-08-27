//! DNS-pinned reqwest client cache.
//!
//! Producers pin each validated DNS resolution onto the reqwest client via
//! `resolve_to_addrs`, so reusing one client per `(host, pinned address
//! set)` pair preserves SSRF safety while avoiding a per-request client
//! build. Entries expire after [`PINNED_CLIENT_TTL`].

use moka::future::Cache;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// How long a cached pinned client stays live after creation.
///
/// Together with [`PINNED_CLIENT_MAX_ENTRIES`] this bounds the retained
/// client pool size, the staleness window of TLS material held by pooled
/// connections, and how long an address set pinned at DNS-validation time
/// keeps being trusted. Expiry is logical (non-retrievability); physical reclamation happens once cache maintenance drops the entry and no in-flight request still holds a client clone.
pub(crate) const PINNED_CLIENT_TTL: Duration = Duration::from_secs(60);

/// Maximum number of pinned clients retained at once.
///
/// Together with [`PINNED_CLIENT_TTL`] this bounds the retained client pool
/// size, the staleness window of TLS material held by pooled connections,
/// and how long an address set pinned at DNS-validation time keeps being
/// trusted. Expiry is logical (non-retrievability); physical reclamation
/// happens once cache maintenance drops the entry and no in-flight request
/// still holds a client clone.
pub(crate) const PINNED_CLIENT_MAX_ENTRIES: u64 = 64;

/// Cache key: host plus its canonicalized validated address set.
///
/// The same host reached through a changed validated address set must map
/// to a different client, so the set is part of the key.
#[derive(Clone, Eq, PartialEq, Hash)]
struct PinnedKey(String, Vec<SocketAddr>);

fn pinned_key(host: &str, addrs: &[SocketAddr]) -> PinnedKey {
    let mut canonical = addrs.to_vec();
    canonical.sort_unstable();
    canonical.dedup();
    PinnedKey(host.to_owned(), canonical)
}

/// Reusable cache of DNS-pinned reqwest clients.
///
/// One client per `(host, canonicalized pinned address set)` pair, built on
/// first use and reused until TTL expiry or capacity eviction. Concurrent
/// callers for one key share a single build (moka `get_with` single-flight).
pub(crate) struct PinnedClientCache {
    cache: Cache<PinnedKey, reqwest::Client>,
    build_counter: AtomicU64,
}

impl PinnedClientCache {
    pub(crate) fn new(ttl: Duration, max_entries: u64) -> Self {
        Self {
            cache: Cache::builder()
                .time_to_live(ttl)
                .max_capacity(max_entries)
                .build(),
            build_counter: AtomicU64::new(0),
        }
    }

    /// Returns the cached client for `(host, addrs)` or builds and inserts
    /// one via `build`. Only actual builds increment the build counter; the
    /// increment sits inside the init future.
    pub(crate) async fn get_or_build(
        &self,
        host: &str,
        addrs: &[SocketAddr],
        build: impl FnOnce() -> reqwest::Client,
    ) -> reqwest::Client {
        let key = pinned_key(host, addrs);
        self.cache
            .get_with(key, async move {
                self.build_counter.fetch_add(1, Ordering::Relaxed);
                build()
            })
            .await
    }

    /// Number of clients actually built (cache misses), test-only.
    #[cfg(test)]
    pub(crate) fn build_count(&self) -> u64 {
        self.build_counter.load(Ordering::Relaxed)
    }

    /// Drains moka's pending maintenance queue so `entry_count` is settled,
    /// test-only.
    #[cfg(test)]
    pub(crate) async fn run_pending_tasks(&self) {
        self.cache.run_pending_tasks().await;
    }

    /// Approximate number of retained entries, test-only.
    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> u64 {
        self.cache.entry_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HttpConfig;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    fn addr(spec: &str) -> SocketAddr {
        spec.parse().expect("valid socket address")
    }

    /// Accept-loop that records the first received connection and replies
    /// with a fixed 200 response.
    fn spawn_recorder(listener: TcpListener, hit: Arc<AtomicBool>) {
        tokio::spawn(async move {
            loop {
                let Ok((mut conn, _)) = listener.accept().await else {
                    break;
                };
                hit.store(true, Ordering::Relaxed);
                let _ = conn
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await;
                let _ = conn.shutdown().await;
            }
        });
    }

    #[tokio::test]
    async fn hit_within_ttl_builds_once() {
        let cache = PinnedClientCache::new(Duration::from_millis(50), 64);
        let builder = || reqwest::Client::new();
        let addr_a = addr("127.0.0.1:8081");
        cache
            .get_or_build("api.example.com", &[addr_a], builder)
            .await;
        cache
            .get_or_build("api.example.com", &[addr_a], builder)
            .await;
        assert_eq!(cache.build_count(), 1);
    }

    #[tokio::test]
    async fn ttl_expiry_rebuilds() {
        let cache = PinnedClientCache::new(Duration::from_millis(50), 64);
        let addr_a = addr("127.0.0.1:8081");
        let builder = || reqwest::Client::new();
        cache
            .get_or_build("api.example.com", &[addr_a], builder)
            .await;
        tokio::time::sleep(Duration::from_millis(80)).await;
        cache
            .get_or_build("api.example.com", &[addr_a], builder)
            .await;
        assert_eq!(
            cache.build_count(),
            2,
            "expired entry is logically dead; a fresh client must be built"
        );
    }

    #[tokio::test]
    async fn changed_addr_set_builds_new_entry() {
        // Bind two listeners on 127.0.0.1 with DISTINCT ports. Distinct
        // loopback IPs (127.0.0.2/.3) only exist on Linux — macOS exposes
        // 127.0.0.1 alone and fails with AddrNotAvailable (rc-dwmd). The
        // cache key is the address SET, so different ports distinguish
        // entries exactly like different IPs did.
        let listener_a = TcpListener::bind("127.0.0.1:0").await.expect("bind a");
        let listener_b = TcpListener::bind("127.0.0.1:0").await.expect("bind b");
        let addr_a = listener_a.local_addr().expect("local addr a");
        let addr_b = listener_b.local_addr().expect("local addr b");
        let port = addr_b.port();

        let hits_a = Arc::new(AtomicBool::new(false));
        let hits_b = Arc::new(AtomicBool::new(false));
        spawn_recorder(listener_a, Arc::clone(&hits_a));
        spawn_recorder(listener_b, Arc::clone(&hits_b));

        let cache = PinnedClientCache::new(PINNED_CLIENT_TTL, PINNED_CLIENT_MAX_ENTRIES);
        let _client_a = cache
            .get_or_build("localhost", &[addr_a], || {
                crate::build_client(&HttpConfig::default(), Some(("localhost", &[addr_a])))
            })
            .await;
        let client_b = cache
            .get_or_build("localhost", &[addr_b], || {
                crate::build_client(&HttpConfig::default(), Some(("localhost", &[addr_b])))
            })
            .await;

        assert_eq!(cache.build_count(), 2);

        let resp = client_b
            .get(format!("http://localhost:{port}/x"))
            .send()
            .await
            .expect("request over pinned address set");
        assert_eq!(resp.status(), 200);

        cache.run_pending_tasks().await;
        assert_eq!(cache.entry_count(), 2);
        assert!(
            !hits_a.load(Ordering::Relaxed),
            "connection must not reach listener a ({addr_a})"
        );
        assert!(
            hits_b.load(Ordering::Relaxed),
            "connection must reach listener b ({addr_b})"
        );
    }

    #[tokio::test]
    async fn addr_order_normalized() {
        let cache = PinnedClientCache::new(Duration::from_millis(50), 64);
        let addr_a = addr("127.0.0.1:8081");
        let addr_b = addr("127.0.0.1:8082");
        let builder = || reqwest::Client::new();
        cache
            .get_or_build("api.example.com", &[addr_a, addr_b], builder)
            .await;
        // Duplicated and reordered addresses canonicalize to the same key.
        cache
            .get_or_build("api.example.com", &[addr_b, addr_a, addr_a], builder)
            .await;
        assert_eq!(cache.build_count(), 1);
    }

    #[tokio::test]
    async fn concurrent_same_key_builds_once() {
        let cache = Arc::new(PinnedClientCache::new(Duration::from_millis(50), 64));
        let addr_a = addr("127.0.0.1:8081");
        let mut joins = Vec::with_capacity(8);
        for _ in 0..8 {
            let cache = Arc::clone(&cache);
            joins.push(tokio::spawn(async move {
                cache
                    .get_or_build("api.example.com", &[addr_a], reqwest::Client::new)
                    .await
            }));
        }
        for join in joins {
            join.await.expect("task join");
        }
        assert_eq!(cache.build_count(), 1, "moka get_with single-flight");
    }

    #[tokio::test]
    async fn capacity_bounds_entries() {
        let cache = PinnedClientCache::new(PINNED_CLIENT_TTL, 2);
        for (host, ip) in [
            ("one.example.com", [127, 0, 0, 1]),
            ("two.example.com", [127, 0, 0, 1]),
            ("three.example.com", [127, 0, 0, 1]),
        ] {
            cache
                .get_or_build(host, &[SocketAddr::from((ip, 8081))], || {
                    reqwest::Client::new()
                })
                .await;
        }
        assert_eq!(cache.build_count(), 3);
        cache.run_pending_tasks().await;
        assert!(
            cache.entry_count() <= 2,
            "max_capacity must bound retained entries"
        );
    }
}
