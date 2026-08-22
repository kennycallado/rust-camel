//! In-process mock test runner: boots a lean `CamelContext`, loads routes,
//! delivers `direct:` inputs, settles traffic, and evaluates expectations.
//!
//! Spec: openspec/changes/mock-declarative-testkit (design D3-D7).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use camel_api::{Body, Exchange, Message};
use camel_component_api::NoOpComponentContext;
use camel_component_direct::DirectComponent;
use camel_component_log::LogComponent;
use camel_component_mock::MockComponent;
use camel_component_seda::SedaComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;
use noyalib::compat::serde_yaml;
use tokio::sync::Mutex;
use tower::ServiceExt;

use super::document::{ExpectSet, InputBody, TestDocError, TestDocument, TestInput};

/// Instability budget: traffic must quiesce within this window after the
/// quiet window elapses, anchored at route-execution begin.
pub(crate) const SETTLE_DEADLINE: Duration = Duration::from_secs(5);
/// Sampling cadence for `received_count` across all expected endpoints.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(50);
/// Default quiet window when the document declares no `settle`.
const DEFAULT_QUIET: Duration = Duration::from_millis(250);
/// Startup-race retry sleep for `direct:` producer delivery.
const STARTUP_RETRY_SLEEP: Duration = Duration::from_millis(20);
/// Startup-race retry deadline for `direct:` producer delivery.
const STARTUP_RETRY_DEADLINE: Duration = Duration::from_secs(1);

/// Outcome of evaluating one mock endpoint.
pub struct EndpointResult {
    /// Bare mock endpoint name (URI suffix after `mock:`).
    pub endpoint: String,
    /// `Ok(())` when all expectations hold; `Err` carries the failure text.
    pub outcome: Result<(), String>,
}

/// Outcome of running one test document.
pub struct TestDocResult {
    /// Per-endpoint evaluation results, in `expects` iteration order.
    pub endpoint_results: Vec<EndpointResult>,
    /// Document-level error (boot, route load, input delivery, settle
    /// timeout). `None` when the document ran to evaluation.
    pub doc_error: Option<String>,
}

/// Boot a lean `CamelContext` with the mock component plus the direct, timer,
/// log, and seda defaults (mirrors camel-test's `build_context`). Returns the
/// context and the shared mock handle used for sampling and assertions.
async fn boot_context() -> Result<(CamelContext, MockComponent), String> {
    let mut ctx = CamelContext::builder()
        .build()
        .await
        .map_err(|e| format!("failed to boot CamelContext: {e}"))?;
    let mock = MockComponent::new();
    ctx.register_component(mock.clone());
    ctx.register_component(DirectComponent::new());
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(SedaComponent::new());
    Ok((ctx, mock))
}

/// Find the nearest ancestor directory of `start` (including `start`
/// itself) that contains a `Camel.toml`.
///
/// The walk is STRICT: only `Camel.toml` marks a root here. This differs
/// from [`crate::commands::plugin::find_camel_root`], which at each
/// ancestor level accepts the nearest marker of either kind (`Camel.toml`
/// OR a workspace `Cargo.toml`). Test documents must resolve
/// `routeFilesFromRoot` against a real Camel project root, so the two
/// walks must not be merged.
pub(crate) fn find_camel_toml_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join("Camel.toml").exists())
        .map(Path::to_path_buf)
}

/// Load route definitions from the document's route source.
///
/// `routeFilesFromRoot` paths resolve against the nearest ancestor
/// `Camel.toml` directory found by [`find_camel_toml_root`]; no such root
/// is a document error ([`TestDocError::NoProjectRoot`]). `routeFiles`
/// paths resolve relative to `doc_dir`, and both file forms load through
/// `camel_dsl::load_from_file` (the same per-file parser `camel run` uses,
/// including the 16 MiB size cap and path-annotated errors). Inline `routes`
/// are re-serialized to YAML and parsed through `camel_dsl::parse_yaml`.
async fn load_routes(
    doc: &TestDocument,
    doc_dir: &Path,
) -> Result<Vec<camel_core::RouteDefinition>, String> {
    if let Some(files) = &doc.route_files_from_root {
        let root = find_camel_toml_root(doc_dir).ok_or_else(|| {
            TestDocError::NoProjectRoot {
                doc_dir: doc_dir.display().to_string(),
            }
            .to_string()
        })?;
        let mut defs = Vec::new();
        for path in files {
            let full = root.join(path);
            let loaded =
                camel_dsl::load_from_file(&full).map_err(|e| format!("{}: {e}", full.display()))?;
            defs.extend(loaded);
        }
        Ok(defs)
    } else if let Some(files) = &doc.route_files {
        let mut defs = Vec::new();
        for path in files {
            let full = doc_dir.join(path);
            let loaded =
                camel_dsl::load_from_file(&full).map_err(|e| format!("{}: {e}", full.display()))?;
            defs.extend(loaded);
        }
        Ok(defs)
    } else if let Some(value) = &doc.routes {
        // `parse_yaml` expects a top-level `routes:` key; wrap the inline
        // value (the array under `routes:`) back into that shape.
        let mut mapping = serde_yaml::Mapping::new();
        mapping.insert("routes", value.clone());
        let text = serde_yaml::to_string(&serde_yaml::Value::Mapping(mapping))
            .map_err(|e| format!("failed to serialize inline routes: {e}"))?;
        camel_dsl::parse_yaml(&text).map_err(|e| format!("inline routes: {e}"))
    } else {
        Err("document declares none of routeFiles, routeFilesFromRoot, or routes".to_string())
    }
}

/// Deliver one input to a `direct:` endpoint, retrying the consumer-startup
/// race. A non-race `Err` is a document error.
async fn deliver_input(ctx: &Arc<Mutex<CamelContext>>, input: &TestInput) -> Result<(), String> {
    let body = match &input.body {
        Some(InputBody::Text(s)) => Body::Text(s.clone()),
        Some(InputBody::Json(v)) => Body::Json(v.clone()),
        None => Body::Empty,
    };
    let mut message = Message::new(body);
    if let Some(headers) = &input.headers {
        for (k, v) in headers {
            message.set_header(k.clone(), v.clone());
        }
    }
    let exchange = Exchange::new(message);

    let deadline = tokio::time::Instant::now() + STARTUP_RETRY_DEADLINE;
    loop {
        let producer = {
            let ctx = ctx.lock().await;
            let producer_ctx = ctx.producer_context();
            let registry = ctx.registry();
            let component = registry
                .get("direct")
                .ok_or_else(|| "direct component not registered".to_string())?;
            let endpoint = component
                .create_endpoint(&input.to, &*ctx)
                .map_err(|e| format!("failed to create endpoint {}: {e}", input.to))?;
            endpoint
                .create_producer(Arc::new(NoOpComponentContext), &producer_ctx)
                .map_err(|e| format!("failed to create producer for {}: {e}", input.to))?
        };
        match producer.oneshot(exchange.clone()).await {
            Ok(_) => return Ok(()),
            Err(e) => {
                let is_startup_race = matches!(e, camel_api::CamelError::EndpointCreationFailed(_))
                    || e.to_string().contains("not registered");
                if is_startup_race && tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(STARTUP_RETRY_SLEEP).await;
                    continue;
                }
                return Err(format!("input to {} failed: {e}", input.to));
            }
        }
    }
}

/// Sample every expected endpoint's `received_count` simultaneously. Endpoints
/// absent from the registry at sample time count as 0 (routes may create them
/// late).
async fn sample_counts(mock: &MockComponent, names: &[String]) -> Vec<usize> {
    let mut counts = Vec::with_capacity(names.len());
    for name in names {
        let count = match mock.get_endpoint(name) {
            Some(inner) => inner.received_count().await,
            None => 0,
        };
        counts.push(count);
    }
    counts
}

/// Settle traffic: wait until all expected endpoints' counts are stable for
/// the quiet window, or fail on the deadline (quiet + instability budget,
/// anchored at route-execution begin).
async fn settle(
    mock: &MockComponent,
    names: &[String],
    quiet: Duration,
    route_started_at: Instant,
) -> Result<(), String> {
    let deadline = route_started_at + quiet + SETTLE_DEADLINE;
    let mut last_change = Instant::now();
    let mut last_counts = sample_counts(mock, names).await;

    loop {
        tokio::time::sleep(SAMPLE_INTERVAL).await;
        let now = Instant::now();
        if now >= deadline {
            return Err(
                "settle timeout: traffic did not quiesce within the 5s instability budget"
                    .to_string(),
            );
        }
        let counts = sample_counts(mock, names).await;
        if counts != last_counts {
            last_counts = counts;
            last_change = now;
            continue;
        }
        if now.duration_since(last_change) >= quiet {
            return Ok(());
        }
    }
}

/// Set expectations on one endpoint from its `ExpectSet`.
fn set_expectations(inner: &camel_component_mock::MockEndpointInner, set: &ExpectSet) {
    if let Some(n) = set.count {
        inner.expect_count(n);
    }
    if let Some(m) = set.min_count {
        inner.expect_minimum_count(m);
    }
    if let Some(bodies) = &set.bodies {
        for b in bodies {
            inner.expect_body(Body::Text(b.clone()));
        }
    }
    if let Some(headers) = &set.headers {
        for (k, v) in headers {
            inner.expect_header(k, v.clone());
        }
    }
}

/// Evaluate one endpoint's expectations, returning its [`EndpointResult`].
async fn evaluate_endpoint(mock: &MockComponent, name: &str, set: &ExpectSet) -> EndpointResult {
    match mock.get_endpoint(name) {
        None => EndpointResult {
            endpoint: name.to_string(),
            outcome: Err(format!("endpoint '{name}' not created by any route")),
        },
        Some(inner) => {
            set_expectations(&inner, set);
            match inner.try_assert_satisfied().await {
                Ok(()) => EndpointResult {
                    endpoint: name.to_string(),
                    outcome: Ok(()),
                },
                Err(e) => EndpointResult {
                    endpoint: name.to_string(),
                    outcome: Err(e.to_string()),
                },
            }
        }
    }
}

/// Run the load/start/deliver/settle/evaluate phases (steps b–f) of one test
/// document. Returns the outcome; the caller is responsible for stopping the
/// context afterwards.
async fn run_phases(
    ctx: &Arc<Mutex<CamelContext>>,
    mock: &MockComponent,
    doc: &TestDocument,
    doc_dir: &Path,
) -> TestDocResult {
    // (b) Load routes.
    let defs = match load_routes(doc, doc_dir).await {
        Ok(defs) => defs,
        Err(e) => {
            return TestDocResult {
                endpoint_results: vec![],
                doc_error: Some(e),
            };
        }
    };

    // (c) Register and start routes; anchor the settle deadline at
    // route-execution begin.
    let route_started_at = {
        let mut guard = ctx.lock().await;
        for def in defs {
            if let Err(e) = guard.add_route_definition(def).await {
                return TestDocResult {
                    endpoint_results: vec![],
                    doc_error: Some(format!("failed to add route: {e}")),
                };
            }
        }
        if let Err(e) = guard.start().await {
            return TestDocResult {
                endpoint_results: vec![],
                doc_error: Some(format!("failed to start routes: {e}")),
            };
        }
        Instant::now()
    };

    // (d) Deliver inputs.
    for input in &doc.inputs {
        if let Err(e) = deliver_input(ctx, input).await {
            return TestDocResult {
                endpoint_results: vec![],
                doc_error: Some(e),
            };
        }
    }

    // (e) Settle traffic.
    let names: Vec<String> = doc.expects.keys().cloned().collect();
    let quiet = doc.settle_duration().unwrap_or(DEFAULT_QUIET);
    if let Err(e) = settle(mock, &names, quiet, route_started_at).await {
        return TestDocResult {
            endpoint_results: vec![EndpointResult {
                endpoint: "<settle>".to_string(),
                outcome: Err(e),
            }],
            doc_error: None,
        };
    }

    // (f) Evaluate expectations.
    let mut endpoint_results = Vec::with_capacity(doc.expects.len());
    for (name, set) in &doc.expects {
        endpoint_results.push(evaluate_endpoint(mock, name, set).await);
    }
    TestDocResult {
        endpoint_results,
        doc_error: None,
    }
}

/// Run one test document in-process. Boots the context, runs the phases, then
/// unconditionally stops the context on every exit path after a successful
/// boot (mirrors camel-test's `TestGuard` — prevents doc N's live timers
/// polluting doc N+1 in the multi-doc driver). Returns the outcome plus the
/// shared mock handle (used by callers to sample `received_count` after
/// return, e.g. to prove the context was stopped).
pub async fn run_test_doc(doc: &TestDocument, doc_dir: &Path) -> (TestDocResult, MockComponent) {
    let (ctx, mock) = match boot_context().await {
        Ok((ctx, mock)) => (Arc::new(Mutex::new(ctx)), mock),
        Err(e) => {
            return (
                TestDocResult {
                    endpoint_results: vec![],
                    doc_error: Some(e),
                },
                MockComponent::new(),
            );
        }
    };

    let result = run_phases(&ctx, &mock, doc, doc_dir).await;

    // (g) Mandatory stop on every exit path after a successful boot.
    {
        let mut guard = ctx.lock().await;
        let _ = guard.stop().await;
    }

    (result, mock)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_camel_toml_root_strict_walk() {
        let root = tempfile::tempdir().expect("tempdir"); // allow-unwrap
        std::fs::write(root.path().join("Camel.toml"), "").expect("write Camel.toml"); // allow-unwrap
        let nested = root.path().join("a").join("b");
        std::fs::create_dir_all(&nested).expect("create nested dir"); // allow-unwrap
        assert_eq!(
            find_camel_toml_root(&nested),
            Some(root.path().to_path_buf())
        );
    }

    #[test]
    fn find_camel_toml_root_no_marker_is_none() {
        let root = tempfile::tempdir().expect("tempdir"); // allow-unwrap
        // A workspace Cargo.toml is NOT an accepted marker for this walk.
        std::fs::write(root.path().join("Cargo.toml"), "[workspace]\n").expect("write Cargo.toml"); // allow-unwrap
        let nested = root.path().join("nested");
        std::fs::create_dir_all(&nested).expect("create nested dir"); // allow-unwrap
        assert_eq!(find_camel_toml_root(&nested), None);
    }
}
