//! In-process mock test runner: boots a lean `CamelContext`, loads routes,
//! delivers `direct:` inputs (capturing each producer reply for
//! `expectReply` assertions), settles traffic, and evaluates expectations.
//!
//! Spec: openspec/changes/mock-declarative-testkit (design D3-D7).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use camel_api::{Body, Exchange, Message};
use camel_component_api::NoOpComponentContext;
use camel_component_direct::DirectComponent;
use camel_component_log::LogComponent;
use camel_component_mock::HeaderMatcher;
use camel_component_mock::MockComponent;
use camel_component_seda::SedaComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;
use camel_core::cache::MemoryCacheRepository;
use camel_core::claim_check::MemoryClaimCheckRepository;
use camel_core::idempotent::MemoryIdempotentRepository;
use camel_core::intercept::InterceptRules;
use noyalib::compat::serde_yaml;
use tokio::sync::Mutex;
use tower::ServiceExt;

use super::beans::{collect_bean_calls, stub_from_decl};
use super::document::{
    ExpectReply, ExpectSet, InputBody, RepositoriesDoc, TestDocError, TestDocument, TestInput,
};

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
/// `beans`, when present, threads a stub-bean registry into the builder so
/// `bean:` steps resolve at route-add time. `repo_stubs`, when present,
/// registers the declared repository stubs (cache, idempotent, claim check)
/// after boot so `cache:` steps resolve at route-add time.
async fn boot_context(
    intercepts: Option<InterceptRules>,
    beans: Option<Arc<std::sync::Mutex<camel_bean::BeanRegistry>>>,
    repo_stubs: Option<&RepositoriesDoc>,
) -> Result<(CamelContext, MockComponent), String> {
    let mut builder = CamelContext::builder();
    if let Some(rules) = intercepts {
        builder = builder.with_intercept_rules(rules);
    }
    if let Some(registry) = beans {
        builder = builder.beans(registry);
    }
    let mut ctx = builder
        .build()
        .await
        .map_err(|e| format!("failed to boot CamelContext: {e}"))?;
    let mock = MockComponent::new();
    ctx.register_component(mock.clone());
    ctx.register_component(DirectComponent::new());
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(SedaComponent::new());
    if let Some(stubs) = repo_stubs {
        if let Some(cache) = &stubs.cache {
            for name in cache.keys() {
                ctx.register_cache_repository(
                    name.clone(),
                    Arc::new(MemoryCacheRepository::new(name.clone(), 10_000)),
                )
                .expect("repository stub registration must succeed"); // allow-unwrap
            }
        }
        if let Some(idempotent) = &stubs.idempotent {
            for name in idempotent.keys() {
                ctx.register_idempotent_repository(
                    name.clone(),
                    Arc::new(MemoryIdempotentRepository::new(name.clone())),
                )
                .expect("repository stub registration must succeed"); // allow-unwrap
            }
        }
        if let Some(claim_check) = &stubs.claim_check {
            for name in claim_check.keys() {
                ctx.register_claim_check_repository(
                    name.clone(),
                    Arc::new(MemoryClaimCheckRepository::new(name.clone())),
                )
                .expect("repository stub registration must succeed"); // allow-unwrap
            }
        }
    }
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
/// race, and return the reply exchange the producer received. A non-race
/// `Err` is a document error.
async fn deliver_input(
    ctx: &Arc<Mutex<CamelContext>>,
    input: &TestInput,
) -> Result<Exchange, String> {
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
            Ok(reply) => return Ok(reply),
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
        for matcher in bodies {
            inner.expect_body_matcher(matcher.clone());
        }
    }
    if let Some(headers) = &set.headers {
        for (key, matcher) in headers {
            inner.expect_header_matcher(key, matcher.clone());
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

/// Render a received body for reply failure messages (text verbatim, JSON
/// compactly, other variants via their debug form).
fn render_body(body: &Body) -> String {
    match body {
        Body::Text(s) => s.clone(),
        Body::Json(v) => v.to_string(),
        other => format!("{other:?}"),
    }
}

/// Evaluate one input's `expectReply` against its captured reply exchange.
/// The asserted message is the output message when present, else the input
/// (lean route steps mutate the in-message; none sets `exchange.output`).
/// `body` and `headers` are optional and independent; both evaluate through
/// the mock component's public matcher API (never a CLI-private comparison).
/// Expected headers form a submap of the reply message headers. Failure
/// details are deterministic (sorted keys) so tests can assert substrings.
fn evaluate_reply_expectation(
    expect: &ExpectReply,
    reply: &Exchange,
    label: &str,
) -> EndpointResult {
    let message = reply.output.as_ref().unwrap_or(&reply.input);
    if let Some(matcher) = &expect.body
        && !matcher.matches(&message.body)
    {
        return EndpointResult {
            endpoint: label.to_string(),
            outcome: Err(format!(
                "reply body mismatch: expected {matcher}, actual {}",
                render_body(&message.body)
            )),
        };
    }
    if let Some(expected_headers) = &expect.headers {
        let mut entries: Vec<(&String, &HeaderMatcher)> = expected_headers.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (key, matcher) in entries {
            let actual = message.headers.get(key);
            if matcher.matches(actual) {
                continue;
            }
            let actual_render = match actual {
                Some(value) => value.to_string(),
                None => "<missing>".to_string(),
            };
            return EndpointResult {
                endpoint: label.to_string(),
                outcome: Err(format!(
                    "reply header mismatch '{key}': expected {matcher}, actual {actual_render}"
                )),
            };
        }
    }
    EndpointResult {
        endpoint: label.to_string(),
        outcome: Ok(()),
    }
}

/// Build the stub-bean registry for a document, or `Ok(None)` when the
/// document declares no `beans` block (current behavior: no registry wiring).
///
/// Cross-validation runs BEFORE any registry is built: when a declaration
/// carries an explicit `methods` allowlist, every `(name, method)` the routes
/// invoke on that bean must be declared, else
/// [`TestDocError::InvalidBeans`] names the offending method. Each stub's
/// wildcard allowlist is the deduplicated set of methods the routes invoke
/// on that bean.
fn stub_registry(
    doc: &TestDocument,
    defs: &[camel_core::RouteDefinition],
) -> Result<Option<Arc<std::sync::Mutex<camel_bean::BeanRegistry>>>, String> {
    let Some(decls) = doc.bean_decls() else {
        return Ok(None);
    };
    let calls = collect_bean_calls(defs);
    let registry = camel_bean::BeanRegistry::new();
    for (name, decl) in decls {
        if let Some(declared) = decl.methods.as_ref() {
            for (bean_name, method) in &calls {
                if bean_name == name && !declared.contains(method) {
                    return Err(TestDocError::InvalidBeans(format!(
                        "bean {name}: method {method} is not declared"
                    ))
                    .to_string());
                }
            }
        }
        let invoked: Vec<String> = calls
            .iter()
            .filter(|(bean_name, _)| bean_name == name)
            .map(|(_, method)| method.clone())
            .collect();
        registry
            .register(name.clone(), stub_from_decl(name, decl, &invoked))
            .map_err(|e| format!("failed to register bean {name}: {e}"))?;
    }
    Ok(Some(Arc::new(std::sync::Mutex::new(registry))))
}

/// Run the start/deliver/settle/evaluate phases (steps c–f) of one test
/// document against the pre-parsed route definitions. Returns the outcome;
/// the caller is responsible for stopping the context afterwards.
async fn run_phases(
    ctx: &Arc<Mutex<CamelContext>>,
    mock: &MockComponent,
    doc: &TestDocument,
    defs: Vec<camel_core::RouteDefinition>,
) -> TestDocResult {
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

    // (d) Deliver inputs, capturing each reply exchange in input order
    // (delivery stays strictly sequential).
    let mut replies: Vec<Exchange> = Vec::with_capacity(doc.inputs.len());
    for input in &doc.inputs {
        match deliver_input(ctx, input).await {
            Ok(reply) => replies.push(reply),
            Err(e) => {
                return TestDocResult {
                    endpoint_results: vec![],
                    doc_error: Some(e),
                };
            }
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

    // (f) Evaluate expectations, then reply assertions in input order.
    // Reply rows reuse the `EndpointResult` shape so the driver prints one
    // PASS/FAIL line per reply and counts them into its summary with no
    // branching; the `endpoint` field holds the reply label
    // (`reply[i] <input.to>`).
    let mut endpoint_results = Vec::with_capacity(doc.expects.len());
    for (name, set) in &doc.expects {
        endpoint_results.push(evaluate_endpoint(mock, name, set).await);
    }
    for (index, (input, reply)) in doc.inputs.iter().zip(&replies).enumerate() {
        if let Some(expect) = input.expect_reply.as_ref() {
            let label = format!("reply[{index}] {}", input.to);
            endpoint_results.push(evaluate_reply_expectation(expect, reply, &label));
        }
    }
    TestDocResult {
        endpoint_results,
        doc_error: None,
    }
}

/// Run one test document in-process. Loads routes BEFORE boot (definitions
/// parse once and feed both stub-bean collection and route registration),
/// builds the stub-bean registry when the document declares `beans`, boots
/// the context, runs the phases, then unconditionally stops the context on
/// every exit path after a successful boot (mirrors camel-test's `TestGuard`
/// — prevents doc N's live timers polluting doc N+1 in the multi-doc
/// driver). Returns the outcome plus the shared mock handle (used by callers
/// to sample `received_count` after return, e.g. to prove the context was
/// stopped).
pub async fn run_test_doc(doc: &TestDocument, doc_dir: &Path) -> (TestDocResult, MockComponent) {
    // (b) Load routes before boot: ctx-free, parsed exactly once.
    let defs = match load_routes(doc, doc_dir).await {
        Ok(defs) => defs,
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

    // Stub beans: cross-validate and register BEFORE boot so `bean:` steps
    // resolve at route-add time; undeclared methods exit 2 here, not at
    // runtime.
    let beans = match stub_registry(doc, &defs) {
        Ok(beans) => beans,
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

    let (ctx, mock) = match boot_context(doc.intercept_rules(), beans, doc.repository_stubs()).await
    {
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

    let result = run_phases(&ctx, &mock, doc, defs).await;

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

    /// Output-message precedence at the reply-evaluation boundary: with a
    /// hand-built exchange carrying input body `A` and output body `B`, an
    /// expectation of `B` passes and one of `A` fails — the output message
    /// is preferred when present, regardless of DSL reachability (no
    /// lean-set step sets `exchange.output`).
    #[test]
    fn reply_output_message_precedence() {
        let mut exchange = Exchange::new(Message::new(Body::Text("A".to_string())));
        exchange.output = Some(Message::new(Body::Text("B".to_string())));

        let expect_b = ExpectReply {
            body: Some(camel_component_mock::BodyMatcher::Equals(Body::Text(
                "B".to_string(),
            ))),
            headers: None,
        };
        let row = evaluate_reply_expectation(&expect_b, &exchange, "reply[0] direct:in");
        assert_eq!(row.endpoint, "reply[0] direct:in");
        assert!(
            row.outcome.is_ok(),
            "expected B must match output body B: {:?}",
            row.outcome
        );

        let expect_a = ExpectReply {
            body: Some(camel_component_mock::BodyMatcher::Equals(Body::Text(
                "A".to_string(),
            ))),
            headers: None,
        };
        let row = evaluate_reply_expectation(&expect_a, &exchange, "reply[0] direct:in");
        assert!(
            row.outcome.is_err(),
            "expected A must NOT match output body B (output takes precedence)"
        );
    }
}
