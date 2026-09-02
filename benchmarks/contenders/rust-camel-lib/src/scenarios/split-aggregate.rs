//! split-aggregate scenario fixture — rust-camel-lib (Pair A partner,
//! OpenSpec change `bench-missing-cells` task 2.3).
//!
//! Two routes: an outer split route and an aggregation route joined by
//! `direct:agg-in`.
//!
//! ```text
//! timer:bench?period=10&repeatCount=10000
//!   -> set_body(canonical array)                    Body::Text, 591 bytes
//!   -> split(json array items, SEQUENTIAL)          100 fragments "b0".."b99"
//!      -> to("direct:agg-in")                       per-fragment dispatch
//!
//! direct:agg-in
//!   -> set_header("bench.correlation", "bench")     constant correlation key
//!   -> aggregate(completion_size=100,               CollectAll = list-append
//!                force_completion_on_stop=false)
//!   -> completion assert (len == 100, set property) guarded on pending
//!   -> marker               BENCH_ROUTE_READY items=100
//! ```
//!
//! # Canonical array body
//!
//! Inline in this fixture (documented in the scenario README): a JSON
//! array whose 100 items are the strings `b0` through `b99` (item `i`
//! is `"b" + i`), serialized compactly — `["b0","b1",...,"b99"]`,
//! exactly 591 bytes, SHA-256
//! `123444b475c48473309ed966eb69896c6725429021a5a5d2e0eaa0a77a159316`
//! (pasted golden, pinned by `split_aggregate_array_golden`). Before
//! the context starts the fixture logs `BENCH_INPUT_SHA256=<digest>`
//! — same input-provenance pattern as the t2-json fixtures, identical
//! across every contender in the scenario.
//!
//! # Completion binding (pinned mechanism, task 2.3)
//!
//! The aggregate step's completion path is the `tower::Service<Exchange>`
//! response of [`camel_processor::AggregatorService`] (see
//! `impl Service<Exchange> for AggregatorService`, `fn call`, in
//! `crates/camel-processor/src/aggregator.rs`):
//!
//! - A COMPLETED bucket returns the aggregated `Exchange` whose body is
//!   `Body::Json(serde_json::Value::Array)` — the CollectAll strategy
//!   (the "list-append" strategy) appends every fragment body into one
//!   JSON array — and carries the property `CamelAggregatedSize`
//!   ([`camel_processor::aggregator::CAMEL_AGGREGATED_SIZE`], a JSON
//!   u64 set to the bucket length by `call`).
//! - A NOT-yet-complete bucket returns a sentinel `Exchange` with
//!   property `CamelAggregatorPending=true`
//!   ([`camel_processor::aggregator::CAMEL_AGGREGATOR_PENDING`]) and an
//!   empty body. This aggregate has no timeout and
//!   `force_completion_on_stop=false`, so it compiles as a plain
//!   mid-route `Process` step: the pending sentinel flows through the
//!   SAME subsequent steps as the completion. Every step below therefore
//!   guards on the properties — an incomplete bucket can never produce
//!   the marker.
//!
//! # Marker contract
//!
//! The completion-assert step verifies the aggregated collection
//! (length exactly 100 AND consistent with `CamelAggregatedSize`) and
//! sets the exchange property `bench.aggregated.size = 100`. The marker
//! step fires ONLY when that property is present with value 100,
//! emitting the single stdout line `BENCH_ROUTE_READY items=100`. An
//! assert failure returns an error — the route fails before the marker
//! is printed, so the cell fails.
//!
//! # Tick mode (OpenSpec change `bench-consol-tick` task 2.2)
//!
//! The timer is the repeating warm-tick form
//! `timer:bench?period=10&repeatCount=10000` (verbatim, matching the
//! `xsd-validation-bridge` reference): every tick runs the full
//! split→aggregate pipeline, so each tick completes one 100-item
//! bucket. The marker keeps its exact code-path position (the
//! completion path of the agg route) but is gated to the FIRST
//! completed bucket — exactly one marker line per process lifetime.
//! The tick start timestamp is carried route-locally (AtomicU64 epoch
//! nanos — the post-split exchange is the aggregator's rebuilt
//! completion exchange, so exchange state does not survive the
//! boundary); the trailing split-route step appends
//! `BENCH_LATENCY <id> <duration_ns>` to `$BENCH_LATENCY_FILE` per
//! exchange (env read ONCE at branch start; when unset the canonical
//! harness path is used, exactly like the reference — the lib cell
//! argv is bare, so the default is what makes the M2 protocol B
//! reader find the log). Pattern mirrors
//! `xsd-validation-bridge.rs:58,111`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use camel_api::aggregator::{AggregationStrategy, AggregatorConfig};
use camel_api::splitter::SplitterConfig;
use camel_api::{Body, CamelError, Exchange, Value, split_body_json_array};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_direct::DirectComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_processor::aggregator::{CAMEL_AGGREGATED_SIZE, CAMEL_AGGREGATOR_PENDING};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

/// Canonical array cardinality — and the aggregator completion size.
const BENCH_ITEMS: usize = 100;

/// Constant correlation header value; every fragment aggregates into
/// the same bucket.
const BENCH_CORRELATION: &str = "bench";

/// Correlation header name. The constant value is stamped by the
/// `set_header` step at the head of the agg route.
const CORRELATION_HEADER: &str = "bench.correlation";

/// Exchange property stamped by the completion-assert step AFTER the
/// aggregated collection passed the length assert. The marker reads
/// THIS property — never the raw aggregator state — so an incomplete
/// bucket can never produce the marker.
const AGGREGATED_SIZE_PROPERTY: &str = "bench.aggregated.size";

/// Pasted golden: lowercase hex SHA-256 of the exact serialized
/// canonical array `["b0","b1",...,"b99"]` (591 bytes), computed once
/// with python3 hashlib and pinned here + in the scenario README.
#[cfg(test)]
const CANONICAL_ARRAY_SHA256: &str =
    "123444b475c48473309ed966eb69896c6725429021a5a5d2e0eaa0a77a159316";

/// Lowercase hex SHA-256 of `data`.
fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// The canonical array body: 100 items, item `i` is the string
/// `"b<i>"`, serialized compactly (no whitespace) — exactly what
/// `serde_json::to_string` produces for that value. 591 bytes.
fn canonical_split_array() -> String {
    let items: Vec<String> = (0..BENCH_ITEMS).map(|i| format!("\"b{i}\"")).collect();
    format!("[{}]", items.join(","))
}

/// Aggregator configuration shared by the lib route and (mirrored in
/// YAML) by the CLI route: constant-header correlation, completion at
/// exactly [`BENCH_ITEMS`] fragments, CollectAll (list-append) output,
/// no force-completion on stop. `max_buckets` keeps the mandatory
/// memory-release bound at its default (10_000).
fn bench_agg_config() -> AggregatorConfig {
    AggregatorConfig::correlate_by(CORRELATION_HEADER)
        .complete_when_size(BENCH_ITEMS)
        .strategy(AggregationStrategy::CollectAll)
        .force_completion_on_stop(false)
        .build()
        .unwrap_or_else(|e| panic!("bench agg config must build: {e}"))
}

/// The aggregation route: `direct:agg-in` -> constant correlation
/// header -> aggregate -> completion assert -> marker. Registered
/// BEFORE the split route so the `direct:agg-in` consumer exists when
/// the first fragment is dispatched. `marker_fired` gates the marker
/// to the FIRST completed bucket (tick mode completes one bucket per
/// tick; the marker contract is exactly one line).
fn agg_route_definition(
    marker_fired: Arc<AtomicBool>,
) -> Result<camel_core::route::RouteDefinition, CamelError> {
    RouteBuilder::from("direct:agg-in")
        .route_id("bench-agg-route")
        .set_header(
            CORRELATION_HEADER,
            Value::String(BENCH_CORRELATION.to_string()),
        )
        .aggregate(bench_agg_config())
        .process(|ex| async move { completion_assert(ex) })
        .process(move |ex| {
            let mf = Arc::clone(&marker_fired);
            async move { emit_ready_marker(ex, &mf) }
        })
        .build()
}

/// COMPLETION ASSERT — the pinned task-2.3 mechanism.
///
/// Binding (see the module doc for the full contract): the completion
/// payload is the `tower::Service<Exchange>` response `Exchange` of
/// `camel_processor::AggregatorService::call`
/// (`crates/camel-processor/src/aggregator.rs`) — body
/// `Body::Json(Value::Array)` from `AggregationStrategy::CollectAll`,
/// property `CAMEL_AGGREGATED_SIZE` = bucket length (JSON u64). The
/// pending sentinel (`CAMEL_AGGREGATOR_PENDING = true`, empty body)
/// passes through untouched: it never reaches the assert.
///
/// Asserts the aggregated collection length is exactly
/// [`BENCH_ITEMS`] AND consistent with `CamelAggregatedSize`, then
/// sets `bench.aggregated.size = 100` for the marker step.
fn completion_assert(ex: Exchange) -> Result<Exchange, CamelError> {
    let pending = ex
        .property(CAMEL_AGGREGATOR_PENDING)
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if pending {
        return Ok(ex);
    }
    let arr_len = {
        let Body::Json(v) = &ex.input.body else {
            return Err(CamelError::ProcessorError(
                "split-aggregate completion body is not Body::Json".to_string(),
            ));
        };
        let arr = v.as_array().ok_or_else(|| {
            CamelError::ProcessorError(
                "split-aggregate completion body is not a JSON array".to_string(),
            )
        })?;
        if arr.len() != BENCH_ITEMS {
            return Err(CamelError::ProcessorError(format!(
                "split-aggregate aggregated collection length {} != {BENCH_ITEMS}",
                arr.len()
            )));
        }
        arr.len()
    };
    let reported = ex
        .property(CAMEL_AGGREGATED_SIZE)
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            CamelError::ProcessorError(
                "split-aggregate completion exchange missing CamelAggregatedSize".to_string(),
            )
        })?;
    if reported as usize != arr_len {
        return Err(CamelError::ProcessorError(format!(
            "split-aggregate CamelAggregatedSize {reported} != collection length {arr_len}"
        )));
    }
    let mut ex = ex;
    ex.set_property(AGGREGATED_SIZE_PROPERTY, serde_json::json!(arr_len as u64));
    Ok(ex)
}

/// Marker step — fires ONLY from the completion path: it reads the
/// `bench.aggregated.size` property set by [`completion_assert`], so a
/// pending sentinel (incomplete bucket) can never produce the marker.
/// Gated by `marker_fired` to the FIRST completed bucket — tick mode
/// completes one bucket per tick, the marker contract is exactly one
/// line.
fn emit_ready_marker(ex: Exchange, marker_fired: &AtomicBool) -> Result<Exchange, CamelError> {
    if let Some(size) = ex
        .property(AGGREGATED_SIZE_PROPERTY)
        .and_then(Value::as_u64)
        && size == BENCH_ITEMS as u64
        && !marker_fired.swap(true, Ordering::Relaxed)
    {
        tracing::info!("BENCH_ROUTE_READY items={size}");
    }
    Ok(ex)
}

/// Verbatim port of the per-scenario fixture's `main()`.
pub fn run() -> i32 {
    match main_async() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("split-aggregate: {e:?}");
            1
        }
    }
}

#[tokio::main]
async fn main_async() -> Result<(), CamelError> {
    // Initialize the tracing subscriber first so the SHA line and the
    // route's marker emission land on stdout.
    tracing_subscriber::fmt().with_target(false).init();

    let array = canonical_split_array();

    // Input provenance — identical across every contender in the
    // scenario (README golden table), logged before the split.
    tracing::info!("BENCH_INPUT_SHA256={}", sha256_hex(array.as_bytes()));

    // Tick-mode latency sink — read ONCE at branch start, mirroring
    // the reference exactly (xsd-validation-bridge.rs:58-59): when the
    // env is unset, fall back to the CANONICAL harness path so the M2
    // protocol B reader finds the log without scenario-specific wiring
    // (the lib cell argv is bare — run.sh resolve_all_cells does not
    // pass BENCH_LATENCY_FILE for the T2 scenarios).
    let latency_file = std::env::var("BENCH_LATENCY_FILE")
        .unwrap_or_else(|_| "/tmp/v3-protocol-b-split-aggregate_rust-camel-lib.log".to_string());
    if let Some(parent) = std::path::Path::new(&latency_file).parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    _ = tokio::fs::File::create(&latency_file).await;

    let mut ctx = CamelContext::builder().build().await?;
    ctx.register_component(TimerComponent::new());
    ctx.register_component(DirectComponent::new());

    let tick_counter = Arc::new(AtomicU64::new(0));
    let marker_fired = Arc::new(AtomicBool::new(false));
    let latency_file_arc = Arc::new(latency_file);
    let tick_start_ns = Arc::new(AtomicU64::new(0));

    // The agg route first: its direct consumer must exist before the
    // timer route dispatches the first fragment. The marker is gated
    // to the FIRST completed bucket (tick mode completes one bucket
    // per tick).
    ctx.add_route_definition(agg_route_definition(Arc::clone(&marker_fired))?)
        .await?;

    let rc_for_route = Arc::clone(&tick_counter);
    let lf_for_route = Arc::clone(&latency_file_arc);
    let tick_start = Arc::clone(&tick_start_ns);

    let split_route = RouteBuilder::from("timer:bench?period=10&repeatCount=10000")
        .route_id("bench-split-route")
        .set_body(array)
        // Bracket the per-exchange pipeline (unmarshal → split →
        // aggregate) with the tick start timestamp. The bracket step
        // sits AFTER set_body (same as t2-json + the
        // xsd-validation-bridge reference) — with a moved/closure body,
        // a process step ahead of set_body broke the body swap in
        // testing. NOT a general rule: t2-realistic-eip places the same
        // bracket BEFORE its `&'static str` set_body and works.
        //
        // The start is carried ROUTE-LOCALLY (AtomicU64 epoch nanos),
        // NOT on the exchange: the post-split exchange is the
        // aggregator's rebuilt completion exchange — neither
        // extensions nor properties survive the split+aggregate
        // boundary (verified empirically, task 2.2). Ticks are
        // sequential (10 ms period vs ~ms pipeline), so the shared
        // slot cannot straddle ticks in practice.
        .process(move |exchange| {
            let ts = Arc::clone(&tick_start_ns);
            async move {
                let now_ns = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0);
                ts.store(now_ns, Ordering::Relaxed);
                Ok(exchange)
            }
        })
        // Body::Text -> Body::Json (the parsed array): the split
        // expression selects the array items from the STRUCTURED body
        // (a text body would split to zero fragments, silently).
        .unmarshal("json")?
        // Sequential (parallel: false) — one fragment after another,
        // each dispatched to direct:agg-in inside the split scope.
        .split(SplitterConfig::new(split_body_json_array()).parallel(false))
        .to("direct:agg-in")
        .end_split()
        .process(move |exchange| {
            let rc = Arc::clone(&rc_for_route);
            let lf = Arc::clone(&lf_for_route);
            let ts = Arc::clone(&tick_start);
            async move {
                let id = rc.fetch_add(1, Ordering::Relaxed) + 1;
                let duration_ns = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0)
                    .saturating_sub(ts.load(Ordering::Relaxed));
                let line = format!("BENCH_LATENCY {id} {duration_ns}\n");
                if let Ok(mut f) = OpenOptions::new().append(true).open(lf.as_str()).await {
                    let _ = f.write_all(line.as_bytes()).await;
                }
                Ok(exchange)
            }
        })
        .build()?;

    ctx.add_route_definition(split_route).await?;
    ctx.start().await?;

    // Keep the context alive until killed by the smoke/harness (tick
    // mode: the route keeps ticking; the marker fired on the first
    // completed bucket).
    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::Message;
    use camel_component_api::{NoOpComponentContext, RuntimeObservability};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tower::ServiceExt;

    /// Golden: exact serialized length (591) AND SHA-256 against the
    /// pasted literal. Also pins the single-serialization invariant:
    /// re-serializing the parsed array with serde_json reproduces the
    /// exact same bytes.
    #[test]
    fn split_aggregate_array_golden() {
        let array = canonical_split_array();
        assert_eq!(array.len(), 591, "canonical array must be 591 bytes");
        assert_eq!(
            sha256_hex(array.as_bytes()),
            CANONICAL_ARRAY_SHA256,
            "canonical array digest drift"
        );
        let parsed: Value = serde_json::from_str(&array)
            .unwrap_or_else(|e| panic!("canonical array must be valid JSON: {e}"));
        let items = parsed
            .as_array()
            .unwrap_or_else(|| panic!("canonical array must parse as a JSON array"));
        assert_eq!(items.len(), BENCH_ITEMS);
        for (i, item) in items.iter().enumerate() {
            assert_eq!(item, &Value::String(format!("b{i}")), "item {i} drifted");
        }
        let reserialized = serde_json::to_string(&parsed)
            .unwrap_or_else(|e| panic!("re-serialization must succeed: {e}"));
        assert_eq!(reserialized, array, "serde_json round-trip must be exact");
    }

    /// Build a fragment exchange with an empty body `b<i>` (the
    /// correlation header is stamped by the route's set_header step,
    /// not by the sender).
    fn fragment(i: usize) -> Exchange {
        Exchange::new(Message {
            headers: Default::default(),
            body: Body::Json(Value::String(format!("b{i}"))),
        })
    }

    /// Send one exchange into the live `direct:agg-in` consumer —
    /// same producer path as examples/xj-example (`registry` ->
    /// `create_endpoint` -> `create_producer` -> `oneshot`).
    async fn send_to_agg_in(ctx: &CamelContext, ex: Exchange) -> Result<Exchange, CamelError> {
        let component = {
            let registry = ctx.registry();
            registry.get_or_err("direct")?
        };
        let endpoint = component.create_endpoint("direct:agg-in", ctx)?;
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoOpComponentContext);
        let producer = endpoint.create_producer(rt, &ctx.producer_context())?;
        producer.oneshot(ex).await
    }

    /// Incomplete-bucket simulation: the agg route wiring ONLY (no
    /// timer route), 99 of 100 fragments driven through the real
    /// `direct:agg-in` consumer. Every response must be the pending
    /// sentinel; after a 500 ms await window NO completion may have
    /// fired and NO marker line may be in the captured log. The
    /// aggregator config has no timeout and no force-completion, so
    /// the 99-fragment bucket has NO completion path at all — the
    /// marker property (`bench.aggregated.size`) is unreachable.
    #[tokio::test]
    async fn incomplete_bucket_no_completion() {
        /// In-memory log sink: the marker assertion reads what the
        /// route actually logged.
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buffer = Arc::new(Mutex::new(Vec::<u8>::new()));
        let sink = buffer.clone();
        tracing_subscriber::fmt()
            .with_target(false)
            .with_writer(move || Sink(sink.clone()))
            .init();

        let mut ctx = CamelContext::builder()
            .build()
            .await
            .unwrap_or_else(|e| panic!("context build: {e}"));
        ctx.register_component(DirectComponent::new());
        ctx.add_route_definition(
            agg_route_definition(Arc::new(AtomicBool::new(false)))
                .unwrap_or_else(|e| panic!("agg route: {e}")),
        )
        .await
        .unwrap_or_else(|e| panic!("register agg route: {e}"));
        ctx.start()
            .await
            .unwrap_or_else(|e| panic!("context start: {e}"));

        for i in 0..(BENCH_ITEMS - 1) {
            let out = send_to_agg_in(&ctx, fragment(i))
                .await
                .unwrap_or_else(|e| panic!("fragment {i} dispatch failed: {e}"));
            assert_eq!(
                out.property(CAMEL_AGGREGATOR_PENDING),
                Some(&serde_json::json!(true)),
                "fragment {i}: expected the pending sentinel"
            );
            assert!(
                out.property(CAMEL_AGGREGATED_SIZE).is_none(),
                "fragment {i}: completion fired early"
            );
            assert!(
                out.property(AGGREGATED_SIZE_PROPERTY).is_none(),
                "fragment {i}: completion-assert property set without completion"
            );
            assert!(
                matches!(out.input.body, Body::Empty),
                "fragment {i}: pending sentinel body must stay empty"
            );
        }

        // Await window: give any (wrongly configured) timeout or
        // background completion path time to fire.
        tokio::time::sleep(Duration::from_millis(500)).await;

        let captured =
            String::from_utf8_lossy(&buffer.lock().unwrap_or_else(|e| e.into_inner())).into_owned();
        assert!(
            !captured.contains("BENCH_ROUTE_READY"),
            "marker logged on an incomplete bucket:\n{captured}"
        );

        ctx.stop()
            .await
            .unwrap_or_else(|e| panic!("context stop: {e}"));
    }
}
