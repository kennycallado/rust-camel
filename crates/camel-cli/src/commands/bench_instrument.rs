//! Benchmark timing injection for YAML-loaded routes.
//!
//! When `BENCH_LATENCY_FILE` env var is set, every top-level `BuilderStep::To(_)`
//! in each route definition is wrapped with two processors:
//!
//! 1. **Pre**: stores `Arc<Instant::now()>` in `exchange.extensions["BenchStart"]`
//! 2. **Post**: reads it back, computes `elapsed().as_nanos()`, writes
//!    `BENCH_LATENCY <id> <duration_ns> <route_id> <encoded_to_uri>` to the
//!    shared file. The `route_id` and percent-encoded `to_uri` are appended so
//!    raw logs are attributable; the first two fields are unchanged so the
//!    benchmark harness (`run.sh` regex) and loadgen parser keep working.
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
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

/// Encode only whitespace and control chars in the `to_uri` field so the
/// `BENCH_LATENCY` line stays whitespace-delimited while remaining readable
/// (`sql:noop?ds=c` passes through unchanged; `http:host?q=a b` → `...a%20b`).
const BENCH_URI_SAFE: &AsciiSet = &CONTROLS.add(b' ');

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
            let route_id = def.route_id().to_string();
            def.map_steps(|steps| inject_timing(steps, route_id, sf))
        })
        .collect()
}

/// Walk the flat step list and insert pre/post processors around each `To`.
fn inject_timing(
    steps: Vec<BuilderStep>,
    route_id: String,
    file: Arc<Mutex<File>>,
) -> Vec<BuilderStep> {
    let mut result = Vec::with_capacity(steps.len() * 3);
    for step in steps {
        if let BuilderStep::To(uri) = step {
            let counter = Arc::new(AtomicU64::new(0)); // per-pair: ids unique within one To step
            result.push(make_start_processor());
            result.push(BuilderStep::To(uri.clone()));
            result.push(make_end_processor(
                route_id.clone(),
                uri,
                counter,
                Arc::clone(&file),
            ));
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
/// `BENCH_LATENCY <id> <ns> <route_id> <encoded_uri>` to the shared file.
fn make_end_processor(
    route_id: String,
    uri: String,
    counter: Arc<AtomicU64>,
    file: Arc<Mutex<File>>,
) -> BuilderStep {
    BuilderStep::Processor(OpaqueProcessor(BoxProcessor::from_fn(
        move |exchange: Exchange| {
            let counter = Arc::clone(&counter);
            let file = Arc::clone(&file);
            let route_id = route_id.clone();
            let uri = uri.clone();
            Box::pin(async move {
                let id = counter.fetch_add(1, Ordering::Relaxed) + 1;
                let duration_ns = exchange
                    .get_extension::<Instant>(BENCH_START)
                    .map(|t| t.elapsed().as_nanos() as u64)
                    .unwrap_or(0);
                let line = format_bench_line(&route_id, &uri, id, duration_ns);
                if let Ok(mut f) = file.lock() {
                    let _ = f.write_all(line.as_bytes());
                }
                Ok(exchange)
            })
        },
    )))
}

/// Compose one `BENCH_LATENCY` record line.
///
/// Format: `BENCH_LATENCY <tick_id> <duration_ns> <route_id> <encoded_uri>`.
/// The first two fields are unchanged from the original contract so the
/// benchmark harness (`run.sh` regex) and the loadgen parser
/// (`protocol_b::parse_line`, which ignores trailing tokens) keep working.
/// The `route_id` and percent-encoded `to_uri` are appended so raw logs are
/// attributable to a specific route and endpoint. An empty `route_id`
/// collapses to `-` to keep the field count stable for whitespace-delimited
/// consumers.
fn format_bench_line(route_id: &str, uri: &str, id: u64, duration_ns: u64) -> String {
    let route = if route_id.trim().is_empty() {
        "-"
    } else {
        route_id
    };
    let encoded = utf8_percent_encode(uri, BENCH_URI_SAFE);
    format!("BENCH_LATENCY {id} {duration_ns} {route} {encoded}\n")
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
        let out = inject_timing(steps, "test-route".to_string(), file);

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
        let out = inject_timing(steps, "test-route".to_string(), file);
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

    #[test]
    fn format_bench_line_emits_route_id_and_percent_encoded_uri() {
        // URI without spaces passes through readable; route_id preserved verbatim.
        assert_eq!(
            format_bench_line("nacional-chain", "sql:noop?ds=cartodb", 1, 821_852_517),
            "BENCH_LATENCY 1 821852517 nacional-chain sql:noop?ds=cartodb\n"
        );
        // Spaces inside the URI are percent-encoded so the line stays
        // whitespace-delimited (the loadgen parser splits on whitespace).
        assert_eq!(
            format_bench_line("r2", "http:host?q=a b", 3, 1000),
            "BENCH_LATENCY 3 1000 r2 http:host?q=a%20b\n"
        );
        // Empty route_id collapses to a placeholder so the field count is
        // stable for whitespace-delimited consumers.
        assert_eq!(
            format_bench_line("", "direct:foo", 2, 500),
            "BENCH_LATENCY 2 500 - direct:foo\n"
        );
    }
}
