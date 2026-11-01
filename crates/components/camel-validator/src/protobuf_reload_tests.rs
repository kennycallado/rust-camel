//! Protobuf descriptor hot-reload coverage (task 3.2).
//!
//! Each test registers a target on a UNIQUE route id because the
//! [`TemplateReloadRegistry`] is process-global. The producer is the
//! registration owner: it outlives endpoint recompilation and drops its
//! guard on teardown.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::BoxProcessor;
use camel_component_api::template_reload::{
    TemplateReloadRegistry, TemplateReloadStaged, TemplateReloadTarget,
};
use camel_component_api::test_support::PanicRuntimeObservability;
use camel_component_api::{
    Body, CamelError, Component, Endpoint, Exchange, Message, NoOpComponentContext, ProducerContext,
};
use tower::{Service, ServiceExt};

use crate::component::ValidatorComponent;

fn rt() -> Arc<dyn camel_component_api::RuntimeObservability> {
    Arc::new(PanicRuntimeObservability)
}

fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).expect("write proto source");
    path
}

fn proto_endpoint(schema: &Path, message: &str) -> Box<dyn Endpoint> {
    let uri = format!("validator:{}?message={message}", schema.display());
    ValidatorComponent::new()
        .create_endpoint(&uri, &NoOpComponentContext)
        .expect("protobuf endpoint creation should succeed")
}

fn producer(ep: &dyn Endpoint, route: &str) -> BoxProcessor {
    ep.create_producer(rt(), &ProducerContext::new().with_route_id(route))
        .expect("producer creation should succeed")
}

/// Drive one exchange through the long-lived producer without consuming it.
async fn validate_json(producer: &mut BoxProcessor, json: &str) -> Result<(), CamelError> {
    producer.ready().await?;
    producer
        .call(Exchange::new(Message::new(Body::Text(json.to_string()))))
        .await
        .map(|_| ())
}

#[tokio::test]
async fn descriptor_recompiles_on_reload() {
    let dir = tempfile::tempdir().expect("tempdir");
    let schema = write(
        dir.path(),
        "m.proto",
        "syntax = \"proto3\";\npackage reload.v1;\nmessage M { string a = 1; }\n",
    );
    let route = "proto-reload-descriptor-recompiles";
    let ep = proto_endpoint(&schema, "reload.v1.M");
    let mut producer = producer(ep.as_ref(), route);

    assert!(
        validate_json(&mut producer, r#"{"a":"x"}"#).await.is_ok(),
        "the initial descriptor must accept field a"
    );
    assert!(
        validate_json(&mut producer, r#"{"a":"x","b":7}"#)
            .await
            .is_err(),
        "field b must be unknown before the reload"
    );

    std::fs::write(
        &schema,
        "syntax = \"proto3\";\npackage reload.v1;\nmessage M { string a = 1; int32 b = 2; }\n",
    )
    .expect("rewrite proto source");
    TemplateReloadRegistry::global()
        .reload_route(route)
        .await
        .expect("reload should succeed");

    assert!(
        validate_json(&mut producer, r#"{"a":"x","b":7}"#)
            .await
            .is_ok(),
        "the swapped descriptor must accept the new field b"
    );

    drop(producer);
    assert!(
        TemplateReloadRegistry::global().find_all(route).is_empty(),
        "producer teardown must unregister the target"
    );
}

#[tokio::test]
async fn import_only_edit_recompiles() {
    let dir = tempfile::tempdir().expect("tempdir");
    let common = write(
        dir.path(),
        "common.proto",
        "syntax = \"proto3\";\npackage reload.v1;\nmessage C { string x = 1; }\n",
    );
    let root = write(
        dir.path(),
        "root.proto",
        "syntax = \"proto3\";\npackage reload.v1;\nimport \"common.proto\";\nmessage Root { C c = 1; }\n",
    );
    let route = "proto-reload-import-only-edit";
    let ep = proto_endpoint(&root, "reload.v1.C");
    let mut producer = producer(ep.as_ref(), route);

    assert!(
        validate_json(&mut producer, r#"{"x":"a"}"#).await.is_ok(),
        "the imported descriptor must accept field x"
    );

    std::fs::write(
        &common,
        "syntax = \"proto3\";\npackage reload.v1;\nmessage C { string x = 1; int32 y = 2; }\n",
    )
    .expect("edit the imported proto");
    TemplateReloadRegistry::global()
        .reload_route(route)
        .await
        .expect("reload should succeed after an import-only edit");

    assert!(
        validate_json(&mut producer, r#"{"x":"a","y":3}"#)
            .await
            .is_ok(),
        "the reloaded descriptor must accept the field added to the import"
    );

    drop(producer);
}

#[tokio::test]
async fn reload_grown_import_updates_sources() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = write(
        dir.path(),
        "root.proto",
        "syntax = \"proto3\";\npackage reload.v1;\nmessage M { string a = 1; }\n",
    );
    let route = "proto-reload-grown-import-sources";
    let ep = proto_endpoint(&root, "reload.v1.M");
    let mut producer = producer(ep.as_ref(), route);
    assert!(validate_json(&mut producer, r#"{"a":"x"}"#).await.is_ok());

    let before = TemplateReloadRegistry::global().find_all(route);
    assert!(
        before[0]
            .watch_sources()
            .iter()
            .all(|p| p.file_name() != Some(std::ffi::OsStr::new("common.proto"))),
        "the closure must not include an import the schema did not have"
    );

    let common = write(
        dir.path(),
        "common.proto",
        "syntax = \"proto3\";\npackage reload.v1;\nmessage C { string v = 1; }\n",
    );
    std::fs::write(
        &root,
        "syntax = \"proto3\";\npackage reload.v1;\nimport \"common.proto\";\nmessage M { string a = 1; C c = 2; }\n",
    )
    .expect("rewrite root proto");
    TemplateReloadRegistry::global()
        .reload_route(route)
        .await
        .expect("reload should succeed after growing the import closure");

    let after = TemplateReloadRegistry::global().find_all(route);
    let sources = after[0].watch_sources();
    let expected = std::path::absolute(&common).expect("absolute common path");
    assert!(
        sources.contains(&expected),
        "watch_sources must include the newly imported file: {sources:?}"
    );

    drop(producer);
}

#[tokio::test]
async fn failed_recompile_keeps_prior_descriptor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let schema = write(
        dir.path(),
        "m.proto",
        "syntax = \"proto3\";\npackage reload.v1;\nmessage M { string a = 1; }\n",
    );
    let route = "proto-reload-failed-recompile";
    let ep = proto_endpoint(&schema, "reload.v1.M");
    let mut producer = producer(ep.as_ref(), route);
    assert!(validate_json(&mut producer, r#"{"a":"x"}"#).await.is_ok());

    std::fs::write(&schema, "syntax = \"bogus\";").expect("corrupt the proto source");
    let result = TemplateReloadRegistry::global().reload_route(route).await;
    assert!(
        result.is_err(),
        "an invalid proto must make the reload fail, got {result:?}"
    );

    assert!(
        validate_json(&mut producer, r#"{"a":"x"}"#).await.is_ok(),
        "the prior descriptor must keep serving after a failed reload"
    );

    drop(producer);
}

#[tokio::test]
async fn bridge_route_id_registers_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let schema = write(
        dir.path(),
        "m.proto",
        "syntax = \"proto3\";\npackage reload.v1;\nmessage M { string a = 1; }\n",
    );
    let ep = proto_endpoint(&schema, "reload.v1.M");

    // Bridge path: no route id on the producer context.
    let producer = ep
        .create_producer(rt(), &ProducerContext::new())
        .expect("producer creation should succeed");
    assert!(
        TemplateReloadRegistry::global()
            .find_all("validator-bridge")
            .is_empty(),
        "the bridge fallback id must not be registered"
    );

    drop(producer);
}

#[tokio::test]
async fn registration_survives_endpoint_drop() {
    let dir = tempfile::tempdir().expect("tempdir");
    let schema = write(
        dir.path(),
        "m.proto",
        "syntax = \"proto3\";\npackage reload.v1;\nmessage M { string a = 1; }\n",
    );
    let route = "proto-reload-survives-endpoint-drop";
    let ep = proto_endpoint(&schema, "reload.v1.M");
    let producer = producer(ep.as_ref(), route);
    assert_eq!(TemplateReloadRegistry::global().find_all(route).len(), 1);

    drop(ep);
    assert_eq!(
        TemplateReloadRegistry::global().find_all(route).len(),
        1,
        "the producer must own the registration after the endpoint is dropped"
    );
    TemplateReloadRegistry::global()
        .reload_route(route)
        .await
        .expect("reload must still work without the endpoint");

    drop(producer);
    assert!(
        TemplateReloadRegistry::global().find_all(route).is_empty(),
        "producer teardown must unregister the target"
    );
}

#[tokio::test]
async fn producer_replacement_keeps_single_registration() {
    let dir = tempfile::tempdir().expect("tempdir");
    let schema = write(
        dir.path(),
        "m.proto",
        "syntax = \"proto3\";\npackage reload.v1;\nmessage M { string a = 1; }\n",
    );
    let route = "proto-reload-producer-replacement";

    // Generation 1: the pipeline that a hot reload is about to retire.
    let old_endpoint = proto_endpoint(&schema, "reload.v1.M");
    let producer1 = producer(old_endpoint.as_ref(), route);
    assert_eq!(
        TemplateReloadRegistry::global().find_all(route).len(),
        1,
        "the first producer must own the only registration"
    );

    // Generation 2: a fresh compile registers before the old pipeline is
    // retired, so both targets briefly coexist (transient swap window).
    let new_endpoint = proto_endpoint(&schema, "reload.v1.M");
    let mut producer2 = producer(new_endpoint.as_ref(), route);
    assert_eq!(
        TemplateReloadRegistry::global().find_all(route).len(),
        2,
        "the transient swap window must hold both registrations"
    );

    // Retiring the old pipeline must drop its RAII registration.
    drop(producer1);
    assert_eq!(
        TemplateReloadRegistry::global().find_all(route).len(),
        1,
        "retiring the old producer must leave exactly the new registration"
    );

    // Reload must still work against the surviving registration...
    TemplateReloadRegistry::global()
        .reload_route(route)
        .await
        .expect("reload must succeed after producer replacement");

    // ...and the replacement producer must keep serving the descriptor.
    assert!(
        validate_json(&mut producer2, r#"{"a":"x"}"#).await.is_ok(),
        "validation must still work through the replacement producer"
    );

    drop(producer2);
    assert!(
        TemplateReloadRegistry::global().find_all(route).is_empty(),
        "replacement producer teardown must unregister the target"
    );
}

struct GatedStaged;

impl TemplateReloadStaged for GatedStaged {
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

/// A target whose `build` parks a blocking worker until the test releases it.
struct GatedTarget {
    route: String,
    generation: AtomicU64,
    commit_calls: AtomicUsize,
    gate: Arc<std::sync::Mutex<std::sync::mpsc::Receiver<()>>>,
    release: std::sync::mpsc::Sender<()>,
    done: std::sync::Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl GatedTarget {
    fn new(route: &str) -> Arc<Self> {
        let (release, gate) = std::sync::mpsc::channel();
        Arc::new(Self {
            route: route.to_string(),
            generation: AtomicU64::new(0),
            commit_calls: AtomicUsize::new(0),
            gate: Arc::new(std::sync::Mutex::new(gate)),
            release,
            done: std::sync::Mutex::new(None),
        })
    }
}

#[async_trait]
impl TemplateReloadTarget for GatedTarget {
    fn route_id(&self) -> &str {
        &self.route
    }

    fn reload_timeout(&self) -> Duration {
        Duration::from_millis(50)
    }

    fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn watch_sources(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    async fn build(&self) -> Result<(Box<dyn TemplateReloadStaged>, u64), CamelError> {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        *self.done.lock().expect("done slot") = Some(done_rx);
        let read_generation = self.generation.load(Ordering::SeqCst);
        let gate = Arc::clone(&self.gate);
        tokio::task::spawn_blocking(move || {
            let _ = gate.lock().expect("gate lock").recv();
            let _ = done_tx.send(());
        })
        .await
        .map_err(|e| CamelError::TemplateReload(format!("blocking build failed: {e}")))?;
        Ok((Box::new(GatedStaged), read_generation))
    }

    fn commit(&self, _staged: Box<dyn TemplateReloadStaged>) {
        self.commit_calls.fetch_add(1, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test(start_paused = true)]
async fn stalled_build_times_out_without_commit() {
    let route = "proto-reload-stalled-build";
    let target = GatedTarget::new(route);
    let dyn_target: Arc<dyn TemplateReloadTarget> = target.clone();
    let guard = TemplateReloadRegistry::global().register(dyn_target);

    // Spawn so the test can drive the paused clock past the target deadline
    // deterministically (auto-advance does not observe a parked blocking
    // worker as idle).
    let reload =
        tokio::spawn(async move { TemplateReloadRegistry::global().reload_route(route).await });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(100)).await;
    let result = tokio::time::timeout(Duration::from_secs(1), reload)
        .await
        .expect("reload task must finish")
        .expect("reload task must not panic");
    match &result {
        Err(CamelError::TemplateReload(message)) if message == "reload timeout" => {}
        other => panic!("expected reload timeout, got {other:?}"),
    }
    assert_eq!(
        target.commit_calls.load(Ordering::SeqCst),
        0,
        "a timed-out build must never commit"
    );
    assert_eq!(
        target.current_generation(),
        0,
        "generation must not advance"
    );

    target
        .release
        .send(())
        .expect("release the blocking worker");
    let done_rx = target
        .done
        .lock()
        .expect("done slot")
        .take()
        .expect("done receiver installed by build");
    tokio::time::timeout(
        Duration::from_secs(1),
        tokio::task::spawn_blocking(move || done_rx.recv()),
    )
    .await
    .expect("completion probe must finish")
    .expect("join the completion probe")
    .expect("blocking worker must finish after release");

    drop(guard);
}

struct CountingStaged;

impl TemplateReloadStaged for CountingStaged {
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

/// A valid, counting sibling target registered on the same route as the real
/// protobuf target.
struct CountingTarget {
    route: String,
    generation: AtomicU64,
    commit_calls: AtomicUsize,
}

impl CountingTarget {
    fn new(route: &str) -> Arc<Self> {
        Arc::new(Self {
            route: route.to_string(),
            generation: AtomicU64::new(0),
            commit_calls: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl TemplateReloadTarget for CountingTarget {
    fn route_id(&self) -> &str {
        &self.route
    }

    fn reload_timeout(&self) -> Duration {
        Duration::from_millis(5000)
    }

    fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn watch_sources(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    async fn build(&self) -> Result<(Box<dyn TemplateReloadStaged>, u64), CamelError> {
        Ok((Box::new(CountingStaged), self.current_generation()))
    }

    fn commit(&self, _staged: Box<dyn TemplateReloadStaged>) {
        self.commit_calls.fetch_add(1, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn broken_proto_blocks_same_route_template_reload() {
    let dir = tempfile::tempdir().expect("tempdir");
    let schema = write(
        dir.path(),
        "m.proto",
        "syntax = \"proto3\";\npackage reload.v1;\nmessage M { string a = 1; }\n",
    );
    let route = "proto-reload-atomic-same-route";
    let ep = proto_endpoint(&schema, "reload.v1.M");
    let mut producer = producer(ep.as_ref(), route);

    let sibling = CountingTarget::new(route);
    let dyn_sibling: Arc<dyn TemplateReloadTarget> = sibling.clone();
    let sibling_guard = TemplateReloadRegistry::global().register(dyn_sibling);

    assert!(validate_json(&mut producer, r#"{"a":"x"}"#).await.is_ok());
    std::fs::write(&schema, "syntax = \"bogus\";").expect("corrupt the proto source");
    let result = TemplateReloadRegistry::global().reload_route(route).await;
    assert!(
        result.is_err(),
        "the broken protobuf build must fail the whole route reload, got {result:?}"
    );

    assert_eq!(
        sibling.commit_calls.load(Ordering::SeqCst),
        0,
        "the healthy sibling must not commit when any target fails"
    );
    assert_eq!(
        sibling.current_generation(),
        0,
        "the sibling generation must not advance"
    );
    assert!(
        validate_json(&mut producer, r#"{"a":"x"}"#).await.is_ok(),
        "the validator must keep serving its prior descriptor"
    );

    drop(sibling_guard);
    drop(producer);
}
