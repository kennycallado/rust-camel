//! Criterion bench for `HttpRouteRegistry` read paths.
//!
//! Measures the lock-read shape used by `dispatch_handler` (exact legacy
//! lookup, then method-aware REST scan) and by `dispatch_static` (longest
//! mount-prefix selection). The probes called here are bench-only
//! `#[doc(hidden)]` helpers on the registry; production dispatch is not
//! changed by this harness.

use std::collections::HashMap;
use std::hint;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use camel_component_http::RequestEnvelope;
use camel_component_http::registry::{
    HttpRouteRegistry, MountMode, PathSegment, ProbeLookup, StaticMount,
};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tower_http::services::ServeDir;

/// Registry with `n` legacy exact-path routes (`/legacy0` .. `/legacy{n-1}`).
///
/// Each route gets its own channel; the receiver is dropped because the bench
/// never sends, only clones the sender out of the registry. Sender clone cost
/// is identical regardless of whether the receiver is alive.
fn registry_with_legacy(rt: &Runtime, n: usize) -> HttpRouteRegistry {
    let registry = HttpRouteRegistry::new();
    rt.block_on(async {
        for i in 0..n {
            let (tx, _rx) = mpsc::channel(1);
            registry
                .register_api_route(format!("/legacy{i}"), tx)
                .await
                .expect("legacy bench route registration succeeds");
        }
    });
    registry
}

/// Registry with `n` literal REST endpoints (`GET /api/r0` .. `/api/r{n-1}`).
fn registry_with_rest(rt: &Runtime, n: usize) -> HttpRouteRegistry {
    let registry = HttpRouteRegistry::new();
    rt.block_on(async {
        for i in 0..n {
            let (tx, _rx) = mpsc::channel(1);
            registry
                .register_rest_endpoint(
                    "GET".to_string(),
                    vec![
                        PathSegment::Literal("api".to_string()),
                        PathSegment::Literal(format!("r{i}")),
                    ],
                    tx,
                )
                .await
                .expect("REST bench endpoint registration succeeds");
        }
    });
    registry
}

/// One static mount at `mount_path`; the directory never exists because the
/// probe only performs prefix selection, never filesystem IO.
fn static_mount(mount_path: &str) -> StaticMount {
    let dir = PathBuf::from("/nonexistent/rust-camel-bench-static");
    StaticMount {
        mount_path: mount_path.to_string(),
        mode: MountMode::Static,
        dir: dir.clone(),
        cache_control: String::new(),
        error_pages: HashMap::new(),
        serve_dir: ServeDir::new(dir),
    }
}

/// Registry with one static mount per path in `mount_paths`.
fn registry_with_mounts(rt: &Runtime, mount_paths: &[&str]) -> HttpRouteRegistry {
    let registry = HttpRouteRegistry::new();
    rt.block_on(async {
        for mount_path in mount_paths {
            registry
                .register_static_mount(static_mount(mount_path))
                .await
                .expect("bench static mount registration succeeds");
        }
    });
    registry
}

fn bench_lookup_exact(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime builds");
    let mut group = c.benchmark_group("registry_lookup/exact");
    for n in [1usize, 50, 500] {
        let registry = registry_with_legacy(&rt, n);
        let setup = rt.block_on(registry.probe_lookup("GET", "/legacy0"));
        assert!(
            matches!(setup, ProbeLookup::Exact(_)),
            "setup probe on /legacy0 must hit the exact route (n={n})"
        );
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let outcome = rt.block_on(registry.probe_lookup("GET", "/legacy0"));
                hint::black_box(outcome);
            });
        });
    }
    group.finish();
}

fn bench_lookup_rest(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime builds");
    let mut group = c.benchmark_group("registry_lookup/rest");
    for n in [1usize, 50, 500] {
        let registry = registry_with_rest(&rt, n);
        let setup = rt.block_on(registry.probe_lookup("GET", "/api/r0"));
        assert!(
            matches!(setup, ProbeLookup::Rest(_)),
            "setup probe on /api/r0 must hit the REST endpoint (n={n})"
        );
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let outcome = rt.block_on(registry.probe_lookup("GET", "/api/r0"));
                hint::black_box(outcome);
            });
        });
    }
    group.finish();
}

fn bench_lookup_miss(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime builds");
    let mut group = c.benchmark_group("registry_lookup/miss");
    for n in [1usize, 50, 500] {
        let registry = registry_with_rest(&rt, n);
        let setup = rt.block_on(registry.probe_lookup("GET", "/absent"));
        assert!(
            matches!(setup, ProbeLookup::Miss),
            "setup probe on /absent must miss both registries (n={n})"
        );
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let outcome = rt.block_on(registry.probe_lookup("GET", "/absent"));
                hint::black_box(outcome);
            });
        });
    }
    group.finish();
}

/// Eight logical readers under a continuous register/unregister writer.
///
/// Semantics (kept stable so `http-reg-pre` stays comparable): the eight
/// probe futures are polled by `join_all` on ONE task — the metric is
/// reader-BATCH wall-time (8 lookups per iteration) while the writer
/// contends on a separate thread/runtime, NOT eight OS-parallel readers.
/// The writer runs on its own thread with its own runtime, busy-looping
/// without sleeps until the bench group finishes, so reader latency is
/// measured under real lock/snapshot contention. Per-lookup lock
/// elimination is independently covered by the exact/miss benches.
fn bench_concurrent_readers_one_writer(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime builds");
    let registry = registry_with_legacy(&rt, 50);

    let stop = Arc::new(AtomicBool::new(false));
    let writer_stop = Arc::clone(&stop);
    let writer_registry = registry.clone();
    let writer = std::thread::spawn(move || {
        let writer_rt = Runtime::new().expect("writer tokio runtime builds");
        let (tx, _rx) = mpsc::channel::<RequestEnvelope>(1);
        writer_rt.block_on(async {
            while !writer_stop.load(Ordering::Relaxed) {
                if writer_registry
                    .register_api_route("/writer-key".to_string(), tx.clone())
                    .await
                    .is_ok()
                {
                    writer_registry.unregister_api_route("/writer-key").await;
                }
            }
        });
    });

    let mut group = c.benchmark_group("registry_lookup/concurrent_readers_1writer");
    group.bench_function("50_routes_8_readers", |b| {
        b.iter(|| {
            let probes = (0..8).map(|_| registry.probe_lookup("GET", "/legacy0"));
            let outcomes = rt.block_on(futures::future::join_all(probes));
            hint::black_box(outcomes);
        });
    });
    group.finish();

    stop.store(true, Ordering::Relaxed);
    writer.join().expect("writer thread joins");
}

fn bench_static_select(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime builds");
    let mut group = c.benchmark_group("registry_static/select");
    let single = ["/"];
    let nested = ["/", "/assets", "/assets/sub"];
    for (id, mounts, expected) in [
        ("1", single.as_slice(), "/"),
        ("3", nested.as_slice(), "/assets/sub"),
    ] {
        let registry = registry_with_mounts(&rt, mounts);
        let setup = rt.block_on(registry.probe_static_select("/assets/sub/file.txt"));
        assert_eq!(
            setup.as_ref().map(|(_, mount_path)| mount_path.as_str()),
            Some(expected),
            "setup static probe must select the longest matching mount ({id} mount(s))"
        );
        group.bench_with_input(BenchmarkId::from_parameter(id), &id, |b, _| {
            b.iter(|| {
                let selected = rt.block_on(registry.probe_static_select("/assets/sub/file.txt"));
                hint::black_box(selected);
            });
        });
    }
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(30)
        .measurement_time(Duration::from_secs(3));
    targets = bench_lookup_exact,
        bench_lookup_rest,
        bench_lookup_miss,
        bench_concurrent_readers_one_writer,
        bench_static_select
}
criterion_main!(benches);
