//! Inventory metrics — route-state gauge (dashboard-observability T3.1)
//! and build/uptime info (T3.2).
//!
//! Starts an app with one route + prometheus, scrapes `/metrics`, and
//! asserts the route-state family reports the startup-complete projection
//! state. The projection's state vocabulary comes from
//! `RouteRuntimeState::state_label()` and is a closed set: `Registered`,
//! `Starting`, `Started`, `Suspended`, `Stopping`, `Stopped`, `Failed` —
//! so the spec's "route starts" scenario asserts `state="Started"`.
//! Harness idioms mirror `metrics_wiring_test.rs` (pre-allocated port,
//! `configure_context` + direct component, poll-GET against `/metrics`).

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use camel_api::{
    AggregatorConfig, BatchCompletion, Exchange, Message, ResequenceMode, ResequencePolicyConfig,
};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::{NoOpComponentContext, RuntimeObservability};
use camel_component_direct::DirectComponent;
use camel_component_seda::SedaComponent;
use camel_config::config::CamelConfig;
use camel_core::CamelContext;
use camel_core::route::{BuilderStep, RouteDefinition};
use tower::ServiceExt;

/// Pre-allocate an ephemeral port: bind, read, drop. The prometheus service
/// (constructed inside `configure_context`) re-binds it on `ctx.start()`.
fn prealloc_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    listener
        .local_addr()
        .expect("read pre-allocated local addr")
        .port()
}

async fn route_started(ctx: &CamelContext, route_id: &str) -> bool {
    matches!(
        ctx.runtime_route_status(route_id).await,
        Ok(Some(status)) if status == "Started"
    )
}

/// Poll `GET /metrics` until the body contains `needle` — e.g. the
/// route-state family (populated only once the route publishes its
/// lifecycle transitions) or the build/uptime families (T3.2).
async fn poll_metrics_body(port: u16, needle: &str) -> String {
    let url = format!("http://127.0.0.1:{port}/metrics");
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(resp) = client.get(&url).send().await
            && resp.status().is_success()
            && let Ok(body) = resp.text().await
            && body.contains(needle)
        {
            return body;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "prometheus /metrics never exposed {needle} at {url}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn route_starts_end_to_end() {
    let port = prealloc_port();
    let toml_cfg = format!(
        r#"[observability.prometheus]
enabled = true
host = "127.0.0.1"
port = {port}
"#
    );

    let config: CamelConfig = toml::from_str(&toml_cfg).expect("test TOML parses into CamelConfig");
    let mut ctx = CamelConfig::configure_context(&config)
        .await
        .expect("configure_context succeeds");
    // configure_context registers no components by default; the direct
    // component backs the route entry.
    ctx.register_component(DirectComponent::new());

    // direct:entry → direct:missing (failIfNoConsumers=false) mirrors the
    // proven startup path from metrics_wiring_test: the route starts and
    // reaches Started without needing the missing consumer (no exchange is
    // driven here).
    let route = RouteBuilder::from("direct:entry")
        .route_id("entry")
        .to("direct:missing?failIfNoConsumers=false")
        .build()
        .expect("route builds");
    ctx.add_route_definition(route)
        .await
        .expect("route registers");

    ctx.start().await.expect("context starts");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !route_started(&ctx, "entry").await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "route 'entry' did not reach Started within 5s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let body = poll_metrics_body(port, "camel_route_state").await;
    assert!(
        body.contains("camel_route_state{route=\"entry\",state=\"Started\"} 1"),
        "route-state gauge must report Started=1 for 'entry':\n{body}"
    );

    ctx.stop().await.expect("context stops");
}

/// Spec scenario "fresh scrape after restart" (T3.2): a just-built context
/// must expose `camel_build_info` and a `camel_uptime_seconds` that parses
/// to a near-zero value (< 120s determinism pin) — restart resets uptime.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_scrape_shows_restart() {
    let port = prealloc_port();
    let toml_cfg = format!(
        r#"[observability.prometheus]
enabled = true
host = "127.0.0.1"
port = {port}
"#
    );

    let config: CamelConfig = toml::from_str(&toml_cfg).expect("test TOML parses into CamelConfig");
    let mut ctx = CamelConfig::configure_context(&config)
        .await
        .expect("configure_context succeeds");
    ctx.start().await.expect("context starts");

    // Needle is build_info: the uptime gauge renders a 0 default before
    // any record_uptime, so polling on uptime would pass with the
    // emission path deleted. build_info (IntGaugeVec) renders only
    // after with_label_values — a real wiring proof.
    let body = poll_metrics_body(port, "camel_build_info{").await;
    assert!(
        body.contains("camel_build_info"),
        "build info family must be present on a fresh scrape:\n{body}"
    );
    let uptime: f64 = body
        .lines()
        .filter(|l| l.starts_with("camel_uptime_seconds"))
        .filter_map(|l| l.rsplit(' ').next().and_then(|v| v.parse::<f64>().ok()))
        .next()
        .expect("camel_uptime_seconds series must parse");
    assert!(
        uptime > 0.0,
        "uptime must be a real recorded value, not the gauge default:\n{body}"
    );
    assert!(
        uptime < 120.0,
        "fresh context uptime must read < 120s, got {uptime}"
    );

    ctx.stop().await.expect("context stops");
}

// ---------------------------------------------------------------------------
// Queue-depth visibility for buffered stages (T3.3)
// ---------------------------------------------------------------------------

/// No-op observability stand-in for producer creation (mirrors
/// `metrics_wiring_test`'s `test_rt`).
fn test_rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(NoOpComponentContext)
}

/// One `GET /metrics`; empty body on any transport hiccup (callers poll).
async fn scrape(port: u16) -> String {
    let url = format!("http://127.0.0.1:{port}/metrics");
    match reqwest::Client::new().get(&url).send().await {
        Ok(resp) if resp.status().is_success() => resp.text().await.unwrap_or_default(),
        _ => String::new(),
    }
}

/// Parse the `camel_queue_depth{queue="<queue>"} <value>` series.
fn queue_depth_value(body: &str, queue: &str) -> Option<f64> {
    let prefix = format!("camel_queue_depth{{queue=\"{queue}\"}}");
    body.lines()
        .find(|l| l.starts_with(&prefix))
        .and_then(|l| l.rsplit(' ').next())
        .and_then(|v| v.parse::<f64>().ok())
}

/// Poll the scrape until the queue-depth series satisfies the expectation
/// (`want_positive = true` → `> 0`, else `== 0`). The poll window absorbs
/// one extra sampling tick of tolerance after a drain trigger.
async fn await_queue_depth(port: u16, queue: &str, want_positive: bool) -> f64 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let body = scrape(port).await;
        if let Some(v) = queue_depth_value(&body, queue) {
            let satisfied = if want_positive { v > 0.0 } else { v == 0.0 };
            if satisfied {
                return v;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "camel_queue_depth for queue {queue:?} never reached {} within 10s:\n{body}",
            if want_positive { "> 0" } else { "0" }
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Drive one InOnly (fire-and-forget) exchange through `uri` — used where
/// the receiving stage buffers the exchange (aggregator, resequencer), so
/// awaiting a reply would block the test.
async fn drive_inonly(ctx: &CamelContext, uri: &str, headers: &[(&str, &str)]) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let producer = {
            let producer_ctx = ctx.producer_context();
            let registry = ctx.registry();
            let component = registry.get("direct").expect("direct component registered");
            let endpoint = component
                .create_endpoint(uri, ctx)
                .expect("direct endpoint creates");
            endpoint
                .create_producer(test_rt(), &producer_ctx)
                .expect("direct producer creates")
        };
        let mut exchange = Exchange::new(Message::new("queue-depth-probe"));
        for (key, value) in headers {
            exchange.input.set_header(*key, *value);
        }
        match producer.oneshot(exchange).await {
            Ok(_) => return,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => panic!("drive through {uri} failed: {e}"),
        }
    }
}

async fn wait_for_started(ctx: &CamelContext, route_ids: &[&str]) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut all_started = true;
        for id in route_ids {
            if !route_started(ctx, id).await {
                all_started = false;
                break;
            }
        }
        if all_started {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "routes {route_ids:?} did not reach Started within 5s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Spec scenario "SEDA backlog under load": a blocked consumer parks the
/// forwarder inside the pipeline, so queued envelopes accumulate behind it.
/// The queue-depth series must be positive during the backlog and reach
/// zero after the consumer is released and the queue drains.
///
/// The test drives InOut exchanges through a SEDA producer configured with
/// `waitForTaskToComplete=Always`: each envelope carries a reply channel,
/// so the forwarder hands it to the pipeline with `send_and_wait` and
/// parks while the processor holds the exchange. With
/// `concurrentConsumers=1` (default) the backlog therefore accumulates in
/// the SEDA channel itself — a blocked-consumer backlog, not a speed race
/// (an InOnly fire-and-forget producer would drain into the route intake
/// instead and never back up the channel).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seda_backlog_visible() {
    let port = prealloc_port();
    let toml_cfg = format!(
        r#"[observability.prometheus]
enabled = true
host = "127.0.0.1"
port = {port}
"#
    );

    let config: CamelConfig = toml::from_str(&toml_cfg).expect("test TOML parses into CamelConfig");
    let mut ctx = CamelConfig::configure_context(&config)
        .await
        .expect("configure_context succeeds");
    ctx.register_component(SedaComponent::new());

    // Park latch: the worker's processor blocks every exchange until the
    // test fires the release.
    let (release_tx, release_rx) = tokio::sync::watch::channel(false);
    let worker = RouteBuilder::from("seda:work")
        .route_id("seda-worker")
        .process(move |ex| {
            let mut gate = release_rx.clone();
            async move {
                while !*gate.borrow() {
                    gate.changed().await.expect("park latch sender stays alive");
                }
                Ok(ex)
            }
        })
        .build()
        .expect("seda worker route builds");
    ctx.add_route_definition(worker)
        .await
        .expect("worker route registers");

    ctx.start().await.expect("context starts");
    wait_for_started(&ctx, &["seda-worker"]).await;

    // Enqueue 3 reply-awaiting exchanges while the consumer is parked.
    // Producers are created up front (BoxProcessor is 'static), then each
    // drive task parks until its reply arrives — all three run
    // concurrently behind the blocked forwarder.
    let drives = {
        let mut handles = Vec::new();
        for _ in 0..3 {
            let producer = {
                let producer_ctx = ctx.producer_context();
                let registry = ctx.registry();
                let component = registry.get("seda").expect("seda component registered");
                let endpoint = component
                    .create_endpoint("seda:work?waitForTaskToComplete=Always", &ctx)
                    .expect("seda endpoint creates");
                endpoint
                    .create_producer(test_rt(), &producer_ctx)
                    .expect("seda producer creates")
            };
            handles.push(tokio::spawn(async move {
                producer
                    .oneshot(Exchange::new_in_out(Message::new("backlog-probe")))
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }));
        }
        handles
    };

    // Backlog must be visible after one poll tick + scrape.
    let backlog = await_queue_depth(port, "seda:work", true).await;
    assert!(
        backlog > 0.0,
        "SEDA backlog must be positive, got {backlog}"
    );

    // Release the consumer; the queue must drain to zero (one extra
    // sampling tick of tolerance is absorbed by the poll window).
    release_tx.send(true).expect("park latch fires");
    for handle in drives {
        let outcome = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("drive completes within 5s of the release")
            .expect("drive task is not cancelled");
        assert!(
            outcome.is_ok(),
            "reply-awaiting drive must succeed after release: {:?}",
            outcome.err()
        );
    }
    await_queue_depth(port, "seda:work", false).await;

    ctx.stop().await.expect("context stops");
}

/// Spec requirement "Queue depth visible for buffered stages" (aggregator):
/// a partial group in flight (missing closing message) holds a bucket, so
/// the queue-depth series for the aggregator label must be positive;
/// completing the group drains it to zero.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aggregator_buffer_visible() {
    let port = prealloc_port();
    let toml_cfg = format!(
        r#"[observability.prometheus]
enabled = true
host = "127.0.0.1"
port = {port}
"#
    );

    let config: CamelConfig = toml::from_str(&toml_cfg).expect("test TOML parses into CamelConfig");
    let mut ctx = CamelConfig::configure_context(&config)
        .await
        .expect("configure_context succeeds");
    ctx.register_component(DirectComponent::new());

    let agg_config = AggregatorConfig::correlate_by("corr")
        .complete_when_size(3)
        .bucket_ttl(Duration::from_secs(2))
        .build()
        .expect("aggregator config builds");
    let route = RouteBuilder::from("direct:agg")
        .route_id("agg-route")
        .aggregate(agg_config)
        .build()
        .expect("aggregator route builds");
    ctx.add_route_definition(route)
        .await
        .expect("aggregator route registers");

    ctx.start().await.expect("context starts");
    wait_for_started(&ctx, &["agg-route"]).await;

    // Partial group in flight: 2 of the 3 closing messages.
    drive_inonly(&ctx, "direct:agg", &[("corr", "g1")]).await;
    drive_inonly(&ctx, "direct:agg", &[("corr", "g1")]).await;

    let buffered = await_queue_depth(port, "aggregator:agg-route", true).await;
    assert!(
        buffered > 0.0,
        "aggregator buffer must be positive, got {buffered}"
    );

    // Complete the group; the bucket drains to zero.
    drive_inonly(&ctx, "direct:agg", &[("corr", "g1")]).await;
    await_queue_depth(port, "aggregator:agg-route", false).await;

    ctx.stop().await.expect("context stops");
}

/// Spec requirement "Queue depth visible for buffered stages" (resequencer):
/// out-of-order deliveries with the gap sequence missing hold exchanges in
/// the batch buffer, so the queue-depth series for the resequencer label
/// must be positive; delivering the gap drains it to zero.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resequencer_buffer_visible() {
    let port = prealloc_port();
    let toml_cfg = format!(
        r#"[observability.prometheus]
enabled = true
host = "127.0.0.1"
port = {port}
"#
    );

    let config: CamelConfig = toml::from_str(&toml_cfg).expect("test TOML parses into CamelConfig");
    let mut ctx = CamelConfig::configure_context(&config)
        .await
        .expect("configure_context succeeds");
    ctx.register_component(DirectComponent::new());

    // Batch resequencer keyed by header `id`, sorted by header `seq`,
    // window size 3 — the resequencer is the terminal step (continuation
    // boundary, ADR-0029), so no post-steps are needed.
    let route = RouteDefinition::new(
        "direct:reseq",
        vec![BuilderStep::Resequence {
            policy_config: ResequencePolicyConfig {
                mode: ResequenceMode::Batch {
                    correlation: "${header.id}".into(),
                    sort: "${header.seq}".into(),
                    completion: BatchCompletion::Size(3),
                },
            },
        }],
    )
    .with_route_id("reseq-route");
    ctx.add_route_definition(route)
        .await
        .expect("resequencer route registers");

    ctx.start().await.expect("context starts");
    wait_for_started(&ctx, &["reseq-route"]).await;

    // Out-of-order deliveries with the gap (seq 1) missing.
    drive_inonly(&ctx, "direct:reseq", &[("id", "a"), ("seq", "3")]).await;
    drive_inonly(&ctx, "direct:reseq", &[("id", "a"), ("seq", "2")]).await;

    let buffered = await_queue_depth(port, "resequencer:reseq-route", true).await;
    assert!(
        buffered > 0.0,
        "resequencer buffer must be positive, got {buffered}"
    );

    // Deliver the gap; the batch completes and drains to zero.
    drive_inonly(&ctx, "direct:reseq", &[("id", "a"), ("seq", "1")]).await;
    await_queue_depth(port, "resequencer:reseq-route", false).await;

    ctx.stop().await.expect("context stops");
}
