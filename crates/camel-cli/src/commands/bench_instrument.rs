//! Benchmark timing injection for YAML-loaded routes.
//!
//! When `BENCH_LATENCY_FILE` env var is set, every top-level `BuilderStep::To(_)`
//! in each route definition is wrapped with two processors:
//!
//! 1. **Pre**: stores `Arc<Instant::now()>` in `exchange.extensions["BenchStart"]`
//! 2. **Post**: reads it back, computes `elapsed().as_nanos()`, writes
//!    `BENCH_LATENCY <id> <duration_ns>` to the shared file.
//!
//! Per-pair `Arc<AtomicU64>` counter guarantees coherent ids. The latency
//! file is opened once at injection time and shared via `Arc<Mutex<File>>`
//! (no per-tick reopen). When the env var is unset, this module is a no-op.
//!
//! Only top-level `To` steps are wrapped — nested steps inside `Choice`,
//! `Split`, `Filter` etc. are left untouched (wrapping those would measure
//! per-sub-message latency, not per-tick bridge tax).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use camel_api::{BoxProcessor, BoxProcessorExt, Exchange, OpaqueProcessor};
use camel_core::{BuilderStep, RouteDefinition};

/// Extension key under which the pre-`.to()` `Instant` is stored.
const BENCH_START: &str = "BenchStart";

/// If `BENCH_LATENCY_FILE` is set, instrument each route by wrapping
/// top-level `To` steps with timing processors. Returns defs unchanged
/// when the env var is absent (zero-cost no-op).
pub fn maybe_instrument_routes(defs: Vec<RouteDefinition>) -> Vec<RouteDefinition> {
    let Ok(path) = std::env::var("BENCH_LATENCY_FILE") else {
        return defs;
    };

    let file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            // log-policy: system-broken
            tracing::error!("bench_instrument: cannot open BENCH_LATENCY_FILE '{path}': {e}");
            return defs;
        }
    };
    let shared_file = Arc::new(Mutex::new(file));

    tracing::info!("bench_instrument: wrapping top-level To steps (file={path})");

    defs.into_iter()
        .map(|def| {
            let sf = Arc::clone(&shared_file);
            def.map_steps(|steps| inject_timing(steps, sf))
        })
        .collect()
}

/// Walk the flat step list and insert pre/post processors around each `To`.
fn inject_timing(steps: Vec<BuilderStep>, file: Arc<Mutex<File>>) -> Vec<BuilderStep> {
    let mut result = Vec::with_capacity(steps.len() * 3);
    for step in steps {
        if matches!(step, BuilderStep::To(_)) {
            let counter = Arc::new(AtomicU64::new(0)); // per-pair: ids unique within one To step
            result.push(make_start_processor());
            result.push(step);
            result.push(make_end_processor(counter, Arc::clone(&file)));
        } else {
            result.push(step);
        }
    }
    result
}

/// Create a processor that stamps `Instant::now()` into the exchange extension.
fn make_start_processor() -> BuilderStep {
    BuilderStep::Processor(OpaqueProcessor(BoxProcessor::from_fn(
        |mut exchange: Exchange| {
            Box::pin(async move {
                exchange.set_extension(BENCH_START, Arc::new(Instant::now()));
                Ok(exchange)
            })
        },
    )))
}

/// Create a processor that reads the stored `Instant` (via `Arc<dyn Any>`
/// downcast to `Instant`), computes the delta, and appends
/// `BENCH_LATENCY <id> <ns>` to the shared file.
fn make_end_processor(counter: Arc<AtomicU64>, file: Arc<Mutex<File>>) -> BuilderStep {
    BuilderStep::Processor(OpaqueProcessor(BoxProcessor::from_fn(
        move |exchange: Exchange| {
            let counter = Arc::clone(&counter);
            let file = Arc::clone(&file);
            Box::pin(async move {
                let id = counter.fetch_add(1, Ordering::Relaxed) + 1;
                let duration_ns = exchange
                    .get_extension::<Instant>(BENCH_START)
                    .map(|t| t.elapsed().as_nanos() as u64)
                    .unwrap_or(0);
                let line = format!("BENCH_LATENCY {id} {duration_ns}\n");
                if let Ok(mut f) = file.lock() {
                    let _ = f.write_all(line.as_bytes());
                }
                Ok(exchange)
            })
        },
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn inject_timing_wraps_each_top_level_to() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = Arc::new(Mutex::new(File::create(tmp.path()).unwrap()));

        let steps = vec![
            BuilderStep::To("xslt:a".into()),
            BuilderStep::Stop,
            BuilderStep::To("xslt:b".into()),
        ];
        let out = inject_timing(steps, file);

        // 2 To steps × 3 (pre + To + post) + 1 Stop = 7
        assert_eq!(out.len(), 7);
        assert!(matches!(out[0], BuilderStep::Processor(_)));
        assert!(matches!(out[1], BuilderStep::To(_)));
        assert!(matches!(out[2], BuilderStep::Processor(_)));
        assert!(matches!(out[3], BuilderStep::Stop));
        assert!(matches!(out[4], BuilderStep::Processor(_)));
        assert!(matches!(out[5], BuilderStep::To(_)));
        assert!(matches!(out[6], BuilderStep::Processor(_)));
    }

    #[test]
    fn inject_timing_skips_non_to_steps() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = Arc::new(Mutex::new(File::create(tmp.path()).unwrap()));

        let steps = vec![BuilderStep::Stop, BuilderStep::Stop];
        let out = inject_timing(steps, file);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn maybe_instrument_routes_noop_when_env_unset() {
        // SAFETY: test is single-threaded, no other code reads this env var
        // during the test.
        unsafe {
            std::env::remove_var("BENCH_LATENCY_FILE");
        }
        let def = camel_core::RouteDefinition::new(
            "direct:test".to_string(),
            vec![BuilderStep::To("mock:a".into())],
        );
        let defs = vec![def];
        let out = maybe_instrument_routes(defs);
        assert_eq!(out[0].steps().len(), 1);
    }
}
