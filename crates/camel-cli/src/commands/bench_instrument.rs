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
//! **Route mode (`BENCH_LATENCY_MODE=route`, sentinel-anchored).**
//! Per the e_opus ruling D3 (2026-09-16, bd rc-h42s6), the normative
//! Protocol-B window is the Java/lib anchor set: body supply EXCLUDED,
//! core pipeline INCLUDED, trailing log EXCLUDED. For each TIMER-SOURCED
//! route (`from` URI starts with `timer:`), the window runs from a
//! `Log` step whose message is exactly `BENCH_WINDOW_START` to a `Log`
//! step whose message is exactly `BENCH_WINDOW_END`; each sentinel step
//! is REPLACED in place by its timing processor (start-stamp / end-write,
//! route-local slot below — one record per pass, i.e. per tick). Fixture
//! YAMLs place the sentinels per the anchor set: body supply before
//! `START`, trailing log after `END`. Route defs from any other source
//! pass through UNCHANGED: their work is already inside the main
//! record's window (the timer route reaches them by synchronous
//! `direct:` dispatch), and instrumenting a consumer route (e.g.
//! split-aggregate's per-fragment `direct:agg-in`) would emit ~100
//! records per tick and break cross-runtime parity with the lib crate
//! and the JVM latency-writer bean. This mode exists because the T2
//! scenario yamls have no top-level `To` (their `to:` steps are nested
//! inside `split`), so pair mode would write zero records for them.
//!
//! Fail-closed validation: a timer route must carry EXACTLY one `START`
//! before exactly one `END`. Any violation (missing, duplicated, or
//! misordered sentinels), a sentinel in a non-timer route, or any
//! sentinel while route mode is NOT selected (pair mode, or
//! `BENCH_LATENCY_FILE` unset) logs an error and leaves that route
//! UNINSTRUMENTED — zero `BENCH_LATENCY` records downstream make the
//! bench harness fail the cell. Sentinels must never print as literal
//! log lines in a measured run. Non-sentinel `Log` steps pass through
//! untouched.
//!
//! History: era-2 route mode EDGE-BRACKETED the whole route (a start
//! processor before the first step, an end processor after the last).
//! That behavior is REMOVED — D3 re-anchors the window to the anchor
//! set and invalidates the prior cli Protocol-B numbers: those arms are
//! re-run for era-3, not carried forward.
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

/// `BENCH_LATENCY_MODE` value that selects sentinel-anchored route mode.
const BENCH_MODE_ROUTE: &str = "route";

/// Sentinel-anchored route mode is opt-in via `BENCH_LATENCY_MODE=route`
/// (case-insensitive, surrounding whitespace tolerated). Any other
/// value — including unset — selects the default pair mode, which
/// stays bit-identical for its existing consumers (bridge cells wire
/// pair mode — `BENCH_LATENCY_FILE` only).
/// A set, non-empty value that is not `route` additionally logs a
/// warning (fail-open: pair mode is still selected).
fn is_route_mode(mode: &str) -> bool {
    mode.trim().eq_ignore_ascii_case(BENCH_MODE_ROUTE)
}

/// Sentinel `Log` message marking the window start in route mode:
/// matched by EXACT string equality (any log level). Fixture YAMLs
/// place it after body supply, per the e_opus ruling D3 anchor set.
const BENCH_WINDOW_START: &str = "BENCH_WINDOW_START";

/// Sentinel `Log` message marking the window end in route mode:
/// matched by EXACT string equality (any log level). Fixture YAMLs
/// place it before any trailing log step.
const BENCH_WINDOW_END: &str = "BENCH_WINDOW_END";

/// Structural sentinel detection: a `Log` step's message, if any.
fn log_message(step: &BuilderStep) -> Option<&str> {
    match step {
        BuilderStep::Log { message, .. } => Some(message.as_str()),
        _ => None,
    }
}

/// True for the window-START sentinel `Log` step (exact message match).
fn is_window_start_sentinel(step: &BuilderStep) -> bool {
    log_message(step) == Some(BENCH_WINDOW_START)
}

/// True for the window-END sentinel `Log` step (exact message match).
fn is_window_end_sentinel(step: &BuilderStep) -> bool {
    log_message(step) == Some(BENCH_WINDOW_END)
}

/// True if the step list carries any BENCH_WINDOW sentinel — at the
/// top level OR at any depth inside container steps (Split/Filter/
/// Choice/Multicast). Depth-aware by design: a sentinel nested inside
/// a container is a fixture bug in EVERY mode (it would print as a
/// literal log line per inner pass — per-fragment under a split), so
/// every sentinel guard rejects it, not just the route-mode layout
/// check.
fn has_sentinels(steps: &[BuilderStep]) -> bool {
    count_sentinels_anywhere(steps) > 0
}

/// Count BENCH_WINDOW sentinels at every depth of the step tree,
/// including the given level (callers pass a route's top-level step
/// list; container-nested sentinels are reached by recursion).
///
/// r_glm finding on the e_opus D3 review (2026-09-16): a nested
/// sentinel paired with a VALID top-level layout would not trip the
/// zero-record fail-closed — the stray `Log` would print per inner
/// pass INSIDE the measured window (~100x/tick under split-aggregate),
/// silently corrupting Protocol-B. Any sentinel below the top level is
/// therefore a hard fixture bug; `inject_sentinel_anchored` rejects
/// the route when `count_sentinels_anywhere(top) > top-level
/// starts + ends`.
fn count_sentinels_anywhere(steps: &[BuilderStep]) -> usize {
    let mut count = 0usize;
    for step in steps {
        if is_window_start_sentinel(step) || is_window_end_sentinel(step) {
            count += 1;
            continue;
        }
        match step {
            BuilderStep::DeclarativeFilter { steps, .. }
            | BuilderStep::DeclarativeSplit { steps, .. }
            | BuilderStep::DeclarativeStreamSplit { steps, .. }
            | BuilderStep::Split { steps, .. }
            | BuilderStep::Filter { steps, .. }
            | BuilderStep::Multicast { steps, .. } => {
                count += count_sentinels_anywhere(steps);
            }
            BuilderStep::DeclarativeChoice { whens, otherwise } => {
                for when in whens {
                    count += count_sentinels_anywhere(&when.steps);
                }
                if let Some(steps) = otherwise {
                    count += count_sentinels_anywhere(steps);
                }
            }
            BuilderStep::Choice { whens, otherwise } => {
                for when in whens {
                    count += count_sentinels_anywhere(&when.steps);
                }
                if let Some(steps) = otherwise {
                    count += count_sentinels_anywhere(steps);
                }
            }
            _ => {}
        }
    }
    count
}

/// If `BENCH_LATENCY_FILE` is set, instrument each route — either the
/// default pair mode (wrap top-level `To` steps) or, when
/// `BENCH_LATENCY_MODE=route`, the sentinel-anchored route mode
/// (replace the `BENCH_WINDOW_START`/`BENCH_WINDOW_END` sentinel `Log`
/// steps of every timer-sourced route with timing processors). Returns
/// defs unchanged when the env var is absent (zero-cost no-op, except
/// an error log if any route still carries sentinels — they are
/// route-mode-only markers and must not run unmeasured).
pub fn maybe_instrument_routes(defs: Vec<RouteDefinition>) -> Vec<RouteDefinition> {
    let Ok(path) = std::env::var("BENCH_LATENCY_FILE") else {
        for def in &defs {
            if has_sentinels(def.steps()) {
                // log-policy: system-broken
                tracing::error!(
                    "bench_instrument: route '{}' carries BENCH_WINDOW sentinel Log steps \
                     but BENCH_LATENCY_FILE is unset; sentinels are route-mode-only markers \
                     and must not run unmeasured — leaving the route unchanged",
                    def.route_id()
                );
            }
        }
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
            "bench_instrument: route mode (sentinel-anchored), timer-sourced routes \
             only (file={path})"
        );
        route_anchor_defs(defs, &shared_file)
    } else {
        tracing::info!("bench_instrument: wrapping top-level To steps (file={path})");
        pair_wrap_defs(defs, &shared_file)
    }
}

/// Pair mode: wrap every top-level `To` of each route (bit-identical to
/// the pre-sentinel behavior for sentinel-free routes). A route that
/// carries BENCH_WINDOW sentinels fails closed: error + full pass-through
/// with NO `To` wrapping — sentinels are route-mode-only markers, and
/// leaving them half-instrumented would print them as literal log lines
/// in a measured run.
fn pair_wrap_defs(defs: Vec<RouteDefinition>, file: &Arc<Mutex<File>>) -> Vec<RouteDefinition> {
    defs.into_iter()
        .map(|def| {
            if has_sentinels(def.steps()) {
                // log-policy: system-broken
                tracing::error!(
                    "bench_instrument: route '{}' carries BENCH_WINDOW sentinel Log steps \
                     but BENCH_LATENCY_MODE=route is not selected; sentinels are \
                     route-mode-only markers — leaving the route uninstrumented",
                    def.route_id()
                );
                return def;
            }
            let sf = Arc::clone(file);
            let route_id = def.route_id().to_string();
            def.map_steps(|steps| inject_timing(steps, route_id, sf))
        })
        .collect()
}

/// Route mode applies to TIMER-SOURCED routes only (`from` URI starts
/// with `timer:`), anchored at the BENCH_WINDOW sentinel steps. Every
/// other route def passes through unchanged: a consumer route's work is
/// already inside the main record's window (the timer route reaches it
/// by synchronous `direct:` dispatch), so instrumenting it would emit
/// per-fragment records and break the one-record-per-tick cross-runtime
/// contract. A non-timer route that carries sentinels is a fixture bug:
/// error + unchanged pass-through (fail-closed).
fn route_anchor_defs(defs: Vec<RouteDefinition>, file: &Arc<Mutex<File>>) -> Vec<RouteDefinition> {
    defs.into_iter()
        .map(|def| {
            if !def.from_uri().starts_with("timer:") {
                if has_sentinels(def.steps()) {
                    // log-policy: system-broken
                    tracing::error!(
                        "bench_instrument: non-timer route '{}' (from '{}') carries \
                         BENCH_WINDOW sentinel Log steps; sentinels are only valid in \
                         timer-sourced routes under BENCH_LATENCY_MODE=route — leaving \
                         the route unchanged",
                        def.route_id(),
                        def.from_uri()
                    );
                }
                return def;
            }
            let sf = Arc::clone(file);
            let route_id = def.route_id().to_string();
            let from_uri = def.from_uri().to_string();
            def.map_steps(|steps| inject_sentinel_anchored(steps, route_id, from_uri, sf))
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

/// Route mode: validate the sentinel layout, then REPLACE each sentinel
/// `Log` step in place with its timing processor — the window is
/// `BENCH_WINDOW_START` step → `BENCH_WINDOW_END` step (e_opus ruling
/// D3 anchor set: body supply and trailing log excluded). Unlike the
/// era-2 edge bracket there is NO insertion before the first step or
/// after the last. The `from` URI takes the attribution field the pair
/// mode fills with the `to` URI; routes with no top-level `To`
/// (filter/choice/split pipelines) are timed fine.
///
/// Fail-closed validation: EXACTLY one START before exactly one END. On
/// any violation the ORIGINAL step list is returned untouched (no
/// processors injected at all) after an error log — zero `BENCH_LATENCY`
/// records downstream make the bench harness fail the cell rather than
/// silently measure a wrong window.
fn inject_sentinel_anchored(
    steps: Vec<BuilderStep>,
    route_id: String,
    from_uri: String,
    file: Arc<Mutex<File>>,
) -> Vec<BuilderStep> {
    let mut starts = 0usize;
    let mut ends = 0usize;
    let mut last_start = None;
    let mut first_end = None;
    for (i, step) in steps.iter().enumerate() {
        if is_window_start_sentinel(step) {
            starts += 1;
            last_start = Some(i);
        } else if is_window_end_sentinel(step) {
            ends += 1;
            first_end = first_end.or(Some(i));
        }
    }
    // Nested sentinels are rejected even when the top-level layout is
    // valid: they print as literal log lines per inner pass inside the
    // measured window (see count_sentinels_anywhere).
    let nested = count_sentinels_anywhere(&steps) - starts - ends;
    if nested > 0 {
        // log-policy: system-broken
        tracing::error!(
            "bench_instrument: timer route '{route_id}' (from '{from_uri}') carries {nested} \
             BENCH_WINDOW sentinel Log step(s) NESTED inside container steps; sentinels are \
             valid only at the top level of a timer route — leaving the route uninstrumented \
             (zero BENCH_LATENCY records — the bench cell will fail)"
        );
        return steps;
    }
    let ordered = matches!((last_start, first_end), (Some(s), Some(e)) if s < e);
    if starts != 1 || ends != 1 || !ordered {
        let order_note = if starts == 1 && ends == 1 {
            " with END before START"
        } else {
            ""
        };
        // log-policy: system-broken
        tracing::error!(
            "bench_instrument: timer route '{route_id}' (from '{from_uri}') has an invalid \
             BENCH_WINDOW sentinel layout: expected exactly one Log('{BENCH_WINDOW_START}') \
             before exactly one Log('{BENCH_WINDOW_END}'), found {starts} start(s) and {ends} \
             end(s){order_note}; leaving the route uninstrumented (zero BENCH_LATENCY \
             records — the bench cell will fail)"
        );
        return steps;
    }

    // The start timestamp lives in a route-local
    // `Arc<Mutex<Option<Instant>>>` (see the module doc: exchange state
    // does not survive a split + aggregate rebuild). The slot is shared
    // between exactly this pair of processors — a nested `to direct:`
    // into another instrumented route cannot clobber it.
    let start_slot = Arc::new(Mutex::new(None::<Instant>));
    steps
        .into_iter()
        .map(|step| {
            if is_window_start_sentinel(&step) {
                make_route_start_processor(Arc::clone(&start_slot))
            } else if is_window_end_sentinel(&step) {
                make_route_end_processor(
                    route_id.clone(),
                    from_uri.clone(),
                    Arc::clone(&start_slot),
                    Arc::clone(&file),
                )
            } else {
                step
            }
        })
        .collect()
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

    /// Test fixture: a `Log` step with the given message (any level —
    /// sentinel detection matches on message only).
    fn log_step(message: &str) -> BuilderStep {
        BuilderStep::Log {
            level: camel_processor::LogLevel::Info,
            message: message.to_string(),
        }
    }

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
        // pair mode (bridge cells rely on this: they wire
        // BENCH_LATENCY_FILE only).
        assert!(!is_route_mode(""));
        assert!(!is_route_mode("to"));
        assert!(!is_route_mode("pair"));
        assert!(!is_route_mode("routes"));
    }

    #[test]
    fn route_anchor_defs_passes_timer_route_without_sentinels_through() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = Arc::new(Mutex::new(File::create(tmp.path()).unwrap()));

        // Timer route with NO sentinels: fail-closed pass-through — no
        // timing processors anywhere (missing_sentinels case, at the
        // route-definition level).
        let timer = camel_core::RouteDefinition::new(
            "timer:bench?period=10&repeatCount=10000".to_string(),
            vec![BuilderStep::Stop, BuilderStep::To("mock:a".into())],
        );
        let out = route_anchor_defs(vec![timer], &file);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].steps().len(), 2);
        assert!(
            !out[0]
                .steps()
                .iter()
                .any(|s| matches!(s, BuilderStep::Processor(_)))
        );
    }

    #[test]
    fn sentinel_replacement() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = Arc::new(Mutex::new(File::create(tmp.path()).unwrap()));

        let steps = vec![
            BuilderStep::Stop,
            log_step(BENCH_WINDOW_START),
            BuilderStep::Stop,
            log_step(BENCH_WINDOW_END),
            log_step("real"),
        ];
        let out = inject_sentinel_anchored(steps, "r".to_string(), "timer:bench".to_string(), file);

        // In-place replacement: same length, processors exactly at the
        // sentinel positions, NO edge processors (era-2 bracket removed).
        assert_eq!(out.len(), 5);
        assert!(matches!(out[0], BuilderStep::Stop));
        assert!(matches!(out[1], BuilderStep::Processor(_)));
        assert!(matches!(out[2], BuilderStep::Stop));
        assert!(matches!(out[3], BuilderStep::Processor(_)));
        match &out[4] {
            BuilderStep::Log { message, .. } => assert_eq!(message, "real"),
            other => panic!("expected untouched Log step, got {other:?}"),
        }
    }

    #[test]
    fn nested_sentinel_with_valid_top_level_fails_closed() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = Arc::new(Mutex::new(File::create(tmp.path()).unwrap()));

        // Valid top-level layout PLUS a sentinel nested inside a
        // Multicast container: must reject (the nested Log would print
        // per inner pass inside the measured window — r_glm finding on
        // the e_opus D3 review).
        let steps = vec![
            log_step(BENCH_WINDOW_START),
            BuilderStep::Multicast {
                steps: vec![log_step(BENCH_WINDOW_END)],
                config: camel_api::MulticastConfig::new(),
            },
            log_step(BENCH_WINDOW_END),
        ];
        let out = inject_sentinel_anchored(steps, "r".to_string(), "timer:bench".to_string(), file);

        // Pass-through: no processor injected anywhere, sentinels kept
        // as-is (zero BENCH_LATENCY records — the harness trips).
        assert_eq!(out.len(), 3);
        assert!(!out.iter().any(|s| matches!(s, BuilderStep::Processor(_))));
        match &out[1] {
            BuilderStep::Multicast { steps, .. } => {
                assert!(matches!(steps[0], BuilderStep::Log { .. }));
            }
            other => panic!("expected untouched Multicast, got {other:?}"),
        }
    }

    #[test]
    fn pair_mode_nested_sentinel_fails_closed() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = Arc::new(Mutex::new(File::create(tmp.path()).unwrap()));

        // The route's ONLY sentinel is nested: pair mode must treat the
        // route as sentinel-carrying and leave it entirely unwrapped.
        let def = camel_core::RouteDefinition::new(
            "timer:bench".to_string(),
            vec![
                BuilderStep::Multicast {
                    steps: vec![log_step(BENCH_WINDOW_START)],
                    config: camel_api::MulticastConfig::new(),
                },
                BuilderStep::To("mock:a".into()),
            ],
        );
        let out = pair_wrap_defs(vec![def], &file);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].steps().len(),
            2,
            "no To-wrapping on a sentinel-carrying route"
        );
        assert!(
            !out[0]
                .steps()
                .iter()
                .any(|s| matches!(s, BuilderStep::Processor(_)))
        );
    }

    #[test]
    fn route_mode_sentinel_free_non_timer_passthrough() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = Arc::new(Mutex::new(File::create(tmp.path()).unwrap()));

        // Non-timer route without sentinels: the fast path — route mode
        // never pair-wraps and never injects into consumer routes.
        let consumer = camel_core::RouteDefinition::new(
            "direct:agg-in".to_string(),
            vec![BuilderStep::To("mock:a".into()), BuilderStep::Stop],
        );
        let out = route_anchor_defs(vec![consumer], &file);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].steps().len(), 2);
        assert!(matches!(out[0].steps()[0], BuilderStep::To(_)));
        assert!(matches!(out[0].steps()[1], BuilderStep::Stop));
    }

    #[test]
    fn non_sentinel_log_preserved() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = Arc::new(Mutex::new(File::create(tmp.path()).unwrap()));

        let steps = vec![
            log_step("hello"),
            log_step(BENCH_WINDOW_START),
            log_step("world"),
            log_step(BENCH_WINDOW_END),
        ];
        let out = inject_sentinel_anchored(steps, "r".to_string(), "timer:bench".to_string(), file);
        assert_eq!(out.len(), 4);
        for (idx, expected) in [(0usize, "hello"), (2, "world")] {
            match &out[idx] {
                BuilderStep::Log { message, .. } => assert_eq!(message, expected),
                other => panic!("step {idx}: expected Log('{expected}'), got {other:?}"),
            }
        }
        assert!(matches!(out[1], BuilderStep::Processor(_)));
        assert!(matches!(out[3], BuilderStep::Processor(_)));
    }

    #[test]
    fn missing_sentinels_fail_closed() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = Arc::new(Mutex::new(File::create(tmp.path()).unwrap()));

        // Timer route with NO sentinels at all: uninstrumented
        // pass-through, all original steps intact.
        let steps = vec![
            BuilderStep::Stop,
            BuilderStep::To("mock:a".into()),
            log_step("ordinary"),
        ];
        let out = inject_sentinel_anchored(steps, "r".to_string(), "timer:bench".to_string(), file);
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0], BuilderStep::Stop));
        assert!(matches!(out[1], BuilderStep::To(_)));
        assert!(matches!(out[2], BuilderStep::Log { .. }));
        assert!(!out.iter().any(|s| matches!(s, BuilderStep::Processor(_))));
    }

    #[test]
    fn duplicate_or_misordered_sentinels_fail_closed() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = Arc::new(Mutex::new(File::create(tmp.path()).unwrap()));

        // Duplicate START: pass-through, all steps stay Log.
        let out = inject_sentinel_anchored(
            vec![
                log_step(BENCH_WINDOW_START),
                log_step(BENCH_WINDOW_START),
                log_step(BENCH_WINDOW_END),
            ],
            "r".to_string(),
            "timer:bench".to_string(),
            Arc::clone(&file),
        );
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|s| matches!(s, BuilderStep::Log { .. })));

        // END before START: same fail-closed pass-through.
        let out = inject_sentinel_anchored(
            vec![log_step(BENCH_WINDOW_END), log_step(BENCH_WINDOW_START)],
            "r".to_string(),
            "timer:bench".to_string(),
            file,
        );
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|s| matches!(s, BuilderStep::Log { .. })));
    }

    #[test]
    fn end_without_start_fail_closed() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = Arc::new(Mutex::new(File::create(tmp.path()).unwrap()));

        let out = inject_sentinel_anchored(
            vec![log_step(BENCH_WINDOW_END)],
            "r".to_string(),
            "timer:bench".to_string(),
            file,
        );
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], BuilderStep::Log { .. }));
    }

    #[test]
    fn non_timer_route_with_sentinels_fail_closed() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = Arc::new(Mutex::new(File::create(tmp.path()).unwrap()));

        // Sentinels in a non-timer route are a fixture bug: error +
        // unchanged steps (assertable via the unchanged output).
        let direct = camel_core::RouteDefinition::new(
            "direct:in".to_string(),
            vec![
                log_step(BENCH_WINDOW_START),
                BuilderStep::Stop,
                log_step(BENCH_WINDOW_END),
            ],
        );
        let out = route_anchor_defs(vec![direct], &file);
        assert_eq!(out.len(), 1);
        let steps = out[0].steps();
        assert_eq!(steps.len(), 3);
        assert!(matches!(steps[0], BuilderStep::Log { .. }));
        assert!(matches!(steps[1], BuilderStep::Stop));
        assert!(matches!(steps[2], BuilderStep::Log { .. }));
    }

    #[test]
    fn pair_mode_untouched_by_sentinels() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = Arc::new(Mutex::new(File::create(tmp.path()).unwrap()));

        // Route WITH sentinels: fail-closed passthrough — no To
        // wrapping, sentinel Log left as-is.
        let marked = camel_core::RouteDefinition::new(
            "direct:marked".to_string(),
            vec![
                BuilderStep::To("mock:a".into()),
                log_step(BENCH_WINDOW_START),
            ],
        );
        // Route WITHOUT sentinels: wrapped exactly as before
        // (2 To × 3 + 1 Stop = 7, same shape as the pair-mode test).
        let plain = camel_core::RouteDefinition::new(
            "direct:plain".to_string(),
            vec![
                BuilderStep::To("xslt:a".into()),
                BuilderStep::Stop,
                BuilderStep::To("xslt:b".into()),
            ],
        );

        let out = pair_wrap_defs(vec![marked, plain], &file);
        assert_eq!(out.len(), 2);

        assert_eq!(out[0].steps().len(), 2);
        assert!(matches!(out[0].steps()[0], BuilderStep::To(_)));
        assert!(matches!(out[0].steps()[1], BuilderStep::Log { .. }));

        assert_eq!(out[1].steps().len(), 7);
        assert!(matches!(out[1].steps()[0], BuilderStep::Processor(_)));
        assert!(matches!(out[1].steps()[1], BuilderStep::To(_)));
        assert!(matches!(out[1].steps()[2], BuilderStep::Processor(_)));
        assert!(matches!(out[1].steps()[3], BuilderStep::Stop));
        assert!(matches!(out[1].steps()[4], BuilderStep::Processor(_)));
        assert!(matches!(out[1].steps()[5], BuilderStep::To(_)));
        assert!(matches!(out[1].steps()[6], BuilderStep::Processor(_)));
    }

    #[tokio::test]
    async fn one_record_per_pass_through_sentinels() {
        use tower::Service as _;
        use tower::ServiceExt as _;

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = Arc::new(Mutex::new(File::create(tmp.path()).unwrap()));

        // Bracket via sentinels — the D3 window. The sentinel Log steps
        // are replaced by the processor pair; the Stop between them is
        // the (tiny) measured pipeline.
        let steps = vec![
            log_step(BENCH_WINDOW_START),
            BuilderStep::Stop,
            log_step(BENCH_WINDOW_END),
        ];
        let out = inject_sentinel_anchored(
            steps,
            "bench-route".to_string(),
            "timer:bench?period=10&repeatCount=10000".to_string(),
            file,
        );
        let (start, end) = match (out.first(), out.last()) {
            (Some(BuilderStep::Processor(s)), Some(BuilderStep::Processor(e))) => {
                (s.0.clone(), e.0.clone())
            }
            _ => panic!("expected sentinel-anchored processor pair, got {out:?}"),
        };
        assert!(matches!(out[1], BuilderStep::Stop));

        // n passes through the sentinel pair → n records, ids 1..=n,
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
