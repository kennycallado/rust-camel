//! Benchmark timing injection for YAML-loaded routes.
//!
//! Two opt-in modes, both gated on `BENCH_LATENCY_FILE` (unset ⇒ the
//! module is a no-op):
//!
//! **Pair mode (default).** Every top-level `BuilderStep::To(_)` in
//! each route definition is wrapped with two processors:
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
//!
//! **Route mode (`BENCH_LATENCY_MODE=route`, bench-consol-tick task
//! 2.3).** For each TIMER-SOURCED route (`from` URI starts with
//! `timer:`), the WHOLE route body is bracketed: a start processor
//! before the first step stamps the clock, and an end processor after
//! the last step appends one record per pass — one record per tick.
//! Route defs from any other source pass through UNCHANGED: their work
//! is already inside the main record's window (the timer route reaches
//! them by synchronous `direct:` dispatch), and bracketing a consumer
//! route (e.g. split-aggregate's per-fragment `direct:agg-in`) would
//! emit ~100 records per tick and break cross-runtime parity with the
//! lib crate and the JVM latency-writer bean. The window is the full
//! per-tick pipeline (route entry → last step) — the same span the
//! rust-camel-lib fixture and the JVM latency-writer bean measure.
//! This mode exists because the T2 scenario yamls have no top-level
//! `To` (their `to:` steps are nested inside `split`), so pair mode
//! would write zero records for them.
//!
//! The start timestamp is carried ROUTE-LOCALLY, not on the exchange:
//! a split + aggregate boundary rebuilds the exchange, so an extension
//! stamped at route entry would not survive to the last step (verified
//! empirically by the lib fixture, task 2.2). The end processor only
//! READS the slot — taking it would let an overlapped tick's end read
//! `None` and emit `0 ns`, which the protocol-B parser treats as
//! round-invalidating. Residual exposure (mirrors the 2.2 lib crate's
//! documented same): the slot is None-safe but not overlap-safe — if
//! tick N+1's start stamps before tick N's end reads, tick N
//! under-measures by attributing the latest start.

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

/// `BENCH_LATENCY_MODE` value that selects route-bracket mode.
const BENCH_MODE_ROUTE: &str = "route";

/// Route-bracket mode is opt-in via `BENCH_LATENCY_MODE=route`
/// (case-insensitive, surrounding whitespace tolerated). Any other
/// value — including unset — selects the default pair mode, which
/// stays bit-identical for its existing consumer (the
/// xsd-validation-bridge cli cell wires `BENCH_LATENCY_FILE` only).
/// A set, non-empty value that is not `route` additionally logs a
/// warning (fail-open: pair mode is still selected).
fn is_route_mode(mode: &str) -> bool {
    mode.trim().eq_ignore_ascii_case(BENCH_MODE_ROUTE)
}

/// If `BENCH_LATENCY_FILE` is set, instrument each route — either the
/// default pair mode (wrap top-level `To` steps) or, when
/// `BENCH_LATENCY_MODE=route`, the route-bracket mode (wrap the whole
/// route body of every timer-sourced route). Returns defs unchanged
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

    let mode_raw = std::env::var("BENCH_LATENCY_MODE").ok();
    let route_mode = mode_raw.as_deref().map(is_route_mode).unwrap_or(false);
    // Fail-open: an unrecognized value keeps the pair-mode default, but is
    // called out — a silent fallback would hide a typo'd mode selection.
    if let Some(mode) = &mode_raw
        && !mode.is_empty()
        && !is_route_mode(mode)
    {
        tracing::warn!(
            "bench_instrument: unrecognized BENCH_LATENCY_MODE '{mode}' \
             (expected 'route'); falling back to pair mode"
        );
    }

    if route_mode {
        tracing::info!(
            "bench_instrument: route-bracket mode, timer-sourced routes only (file={path})"
        );
        route_bracket_defs(defs, &shared_file)
    } else {
        tracing::info!("bench_instrument: wrapping top-level To steps (file={path})");
        defs.into_iter()
            .map(|def| {
                let sf = Arc::clone(&shared_file);
                let route_id = def.route_id().to_string();
                def.map_steps(|steps| inject_timing(steps, route_id, sf))
            })
            .collect()
    }
}

/// Route-bracket mode applies to TIMER-SOURCED routes only (`from` URI
/// starts with `timer:`). Every other route def passes through
/// unchanged: a consumer route's work is already inside the main
/// record's window (the timer route reaches it by synchronous `direct:`
/// dispatch), so bracketing it would emit per-fragment records and
/// break the one-record-per-tick cross-runtime contract.
fn route_bracket_defs(defs: Vec<RouteDefinition>, file: &Arc<Mutex<File>>) -> Vec<RouteDefinition> {
    defs.into_iter()
        .map(|def| {
            if !def.from_uri().starts_with("timer:") {
                return def;
            }
            let sf = Arc::clone(file);
            let route_id = def.route_id().to_string();
            let from_uri = def.from_uri().to_string();
            def.map_steps(|steps| inject_route_bracket(steps, route_id, from_uri, sf))
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

/// Route-bracket mode: wrap the WHOLE step list in a timing pair — one
/// start processor before the first step, one end processor after the
/// last. The emitted record covers the full per-tick pipeline (route
/// entry → last step); the `from` URI takes the attribution field the
/// pair mode fills with the `to` URI. Unlike pair mode this also times
/// routes with no top-level `To` (filter/choice/split pipelines).
///
/// The start timestamp lives in a route-local
/// `Arc<Mutex<Option<Instant>>>` (see the module doc: exchange state
/// does not survive a split + aggregate rebuild). The slot is shared
/// between exactly this pair of processors — a nested `to direct:`
/// into another instrumented route cannot clobber it.
fn inject_route_bracket(
    steps: Vec<BuilderStep>,
    route_id: String,
    from_uri: String,
    file: Arc<Mutex<File>>,
) -> Vec<BuilderStep> {
    let start_slot = Arc::new(Mutex::new(None::<Instant>));
    let mut result = Vec::with_capacity(steps.len() + 2);
    result.push(make_route_start_processor(Arc::clone(&start_slot)));
    result.extend(steps);
    result.push(make_route_end_processor(
        route_id, from_uri, start_slot, file,
    ));
    result
}

/// Create a processor that stamps `Instant::now()` into the route-local
/// start slot (route entry).
fn make_route_start_processor(start_slot: Arc<Mutex<Option<Instant>>>) -> BuilderStep {
    BuilderStep::Processor(OpaqueProcessor(BoxProcessor::from_fn(
        move |exchange: Exchange| {
            let start_slot = Arc::clone(&start_slot);
            Box::pin(async move {
                if let Ok(mut slot) = start_slot.lock() {
                    *slot = Some(Instant::now());
                }
                Ok(exchange)
            })
        },
    )))
}

/// Create a processor that reads the route-local start stamp (READ,
/// never take — see the module doc), computes the delta, and appends
/// `BENCH_LATENCY <id> <ns> <route_id> <encoded_from_uri>` to the
/// shared file. One record per route pass; ids are per-route.
fn make_route_end_processor(
    route_id: String,
    from_uri: String,
    start_slot: Arc<Mutex<Option<Instant>>>,
    file: Arc<Mutex<File>>,
) -> BuilderStep {
    let counter = Arc::new(AtomicU64::new(0)); // per-route: ids unique within one route
    BuilderStep::Processor(OpaqueProcessor(BoxProcessor::from_fn(
        move |exchange: Exchange| {
            let counter = Arc::clone(&counter);
            let file = Arc::clone(&file);
            let route_id = route_id.clone();
            let from_uri = from_uri.clone();
            let start_slot = Arc::clone(&start_slot);
            Box::pin(async move {
                let id = counter.fetch_add(1, Ordering::Relaxed) + 1;
                let duration_ns = start_slot
                    .lock()
                    .ok()
                    .and_then(|slot| *slot)
                    .map(|t| t.elapsed().as_nanos() as u64)
                    .unwrap_or(0);
                let line = format_bench_line(&route_id, &from_uri, id, duration_ns);
                if let Ok(mut f) = file.lock() {
                    let _ = f.write_all(line.as_bytes());
                }
                Ok(exchange)
            })
        },
    )))
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

    #[test]
    fn is_route_mode_matches_only_route() {
        assert!(is_route_mode("route"));
        assert!(is_route_mode(" Route "));
        assert!(is_route_mode("ROUTE"));
        // Anything else — including unset (empty) — selects the default
        // pair mode (xsd-validation-bridge cli cell relies on this).
        assert!(!is_route_mode(""));
        assert!(!is_route_mode("to"));
        assert!(!is_route_mode("pair"));
        assert!(!is_route_mode("routes"));
    }

    #[test]
    fn route_bracket_defs_brackets_only_timer_sourced_routes() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = Arc::new(Mutex::new(File::create(tmp.path()).unwrap()));

        let timer = camel_core::RouteDefinition::new(
            "timer:bench?period=10&repeatCount=10000".to_string(),
            vec![BuilderStep::Stop],
        );
        // split-aggregate's consumer route: reached by synchronous direct
        // dispatch from the timer route; must NOT get its own bracket.
        let consumer = camel_core::RouteDefinition::new(
            "direct:agg-in".to_string(),
            vec![BuilderStep::To("mock:a".into())],
        );

        let out = route_bracket_defs(vec![timer, consumer], &file);
        assert_eq!(out.len(), 2);

        // Timer-sourced route: bracketed (1 prepended processor + steps
        // + 1 appended processor).
        assert_eq!(out[0].steps().len(), 3);
        assert!(matches!(
            out[0].steps().first(),
            Some(BuilderStep::Processor(_))
        ));
        assert!(matches!(
            out[0].steps().last(),
            Some(BuilderStep::Processor(_))
        ));

        // Non-timer-sourced route def: steps pass through UNCHANGED.
        assert_eq!(out[1].steps().len(), 1);
        assert!(matches!(out[1].steps().first(), Some(BuilderStep::To(_))));
    }

    #[test]
    fn inject_route_bracket_wraps_whole_step_list() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = Arc::new(Mutex::new(File::create(tmp.path()).unwrap()));

        // The T2 yaml shapes have NO top-level To — the bracket must
        // still land: 1 prepended processor + steps + 1 appended.
        let steps = vec![BuilderStep::Stop, BuilderStep::Stop];
        let out = inject_route_bracket(
            steps,
            "bench-route".to_string(),
            "timer:bench".to_string(),
            file,
        );
        assert_eq!(out.len(), 4);
        assert!(matches!(out[0], BuilderStep::Processor(_)));
        assert!(matches!(out[1], BuilderStep::Stop));
        assert!(matches!(out[2], BuilderStep::Stop));
        assert!(matches!(out[3], BuilderStep::Processor(_)));
    }

    #[tokio::test]
    async fn route_bracket_emits_one_record_per_pass() {
        use tower::Service as _;
        use tower::ServiceExt as _;

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = Arc::new(Mutex::new(File::create(tmp.path()).unwrap()));

        let steps = vec![BuilderStep::Stop];
        let out = inject_route_bracket(
            steps,
            "bench-route".to_string(),
            "timer:bench?period=10&repeatCount=10000".to_string(),
            file,
        );
        let (start, end) = match (out.first(), out.last()) {
            (Some(BuilderStep::Processor(s)), Some(BuilderStep::Processor(e))) => {
                (s.0.clone(), e.0.clone())
            }
            _ => panic!("expected processor bracket, got {out:?}"),
        };

        // n passes through the bracket pair → n records, ids 1..=n,
        // positive nanosecond durations, exact 5-field format.
        for _pass in 1..=3u64 {
            let ex = Exchange::new(camel_api::Message::default());
            let ex = start.clone().ready().await.unwrap().call(ex).await.unwrap();
            let _ = end.clone().ready().await.unwrap().call(ex).await.unwrap();
        }

        let content = std::fs::read_to_string(tmp.path()).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3, "one record per pass, got: {content}");
        for (i, line) in lines.iter().enumerate() {
            let id = i + 1;
            let rest = line
                .strip_prefix(&format!("BENCH_LATENCY {id} "))
                .unwrap_or_else(|| panic!("record {id} has bad id: {line}"));
            let ns = rest.split_whitespace().next().unwrap();
            assert!(
                ns.parse::<u64>().unwrap() > 0,
                "record {id} duration must be positive ns: {line}"
            );
            assert_eq!(
                rest,
                format!("{ns} bench-route timer:bench?period=10&repeatCount=10000"),
                "record {id} exact format: {line}"
            );
        }
    }
}
