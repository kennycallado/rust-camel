//! Full-boot Redis assertions and catalog lifetime checks.
#![cfg(feature = "redis")]
mod common;
use camel_integration_test::{runner::fill_bind_vars, *};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, LazyLock},
    time::Duration,
};
use testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use testcontainers_modules::redis::Redis;
static RUNTIME: LazyLock<tokio::runtime::Runtime> =
    LazyLock::new(|| tokio::runtime::Runtime::new().expect("runtime"));
static CONTAINER: tokio::sync::OnceCell<ContainerAsync<Redis>> = tokio::sync::OnceCell::const_new();

fn run<F: std::future::Future>(future: F) -> F::Output {
    RUNTIME.block_on(async {
        tokio::time::timeout(Duration::from_secs(120), future)
            .await
            .expect("Redis boot test, including setup and cleanup, exceeded 120s")
    })
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/redis_state")
}
async fn url() -> String {
    let c = CONTAINER
        .get_or_init(|| async {
            Redis::default()
                .with_tag("7-alpine")
                .start()
                .await
                .expect("container")
        })
        .await;
    format!(
        "redis://127.0.0.1:{}/0",
        c.get_host_port_ipv4(6379).await.expect("port")
    )
}
async fn connection(url: &str) -> redis::aio::MultiplexedConnection {
    redis::Client::open(url)
        .expect("client")
        .get_multiplexed_async_connection()
        .await
        .expect("connect")
}
async fn cmd(c: &redis::aio::MultiplexedConnection, name: &str, args: &[&str]) {
    redis::cmd(name)
        .arg(args)
        .query_async::<redis::Value>(&mut c.clone())
        .await
        .expect("seed/cleanup");
}
async fn boot(key: &str, url: &str) -> (ScenarioDocument, ScenarioRun) {
    let mut doc = parse_scenario_document(&root().join("order.test.yaml")).expect("doc");
    if let ScenarioAction::Validate {
        target: ScenarioTarget::Redis(target),
        ..
    } = &mut doc.scenario[1]
    {
        target.key = key.into();
    }
    let env = LayeredEnv::new(
        BTreeMap::new(),
        BTreeMap::from([
            ("REDIS_URL".into(), url.into()),
            (
                "REDIS_PRODUCER_URI".into(),
                format!("{}?command=SET&key={key}", url.trim_end_matches("/0")),
            ),
        ]),
        Vec::new(),
        ambient_std(),
    );
    let run = boot_scenario(&doc, &root(), &env).await.expect("boot");
    (doc, run)
}
async fn validate_only(mut doc: ScenarioDocument, run: &ScenarioRun) -> DocumentOutcome {
    doc.scenario.remove(0);
    let router = common::router_for("fake:unused", FakeAdapter::scripted(Vec::new()));
    run_scenario_document(
        &doc,
        &router,
        &mut ScenarioVars::new(),
        Some(&run.boot.datasource_catalog()),
    )
    .await
}
async fn shutdown(run: &mut ScenarioRun) {
    run.boot.shutdown(&mut run.ctx).await.expect("shutdown");
}

#[test]
fn redis_state_e2e_route_write_validate() {
    run(async {
        let k = "rc-redis-state:redis_state_e2e_route_write_validate";
        let u = url().await;
        let c = connection(&u).await;
        cmd(&c, "DEL", &[k]).await;
        let (doc, run) = boot(k, &u).await;
        let catalog = run.boot.datasource_catalog();
        let ctx = Arc::new(tokio::sync::Mutex::new(run.ctx));
        let router = common::router_for("direct:order", DirectStimulus::new(ctx.clone()));
        let mut vars = ScenarioVars::new();
        fill_bind_vars(&common::wired_refs(&doc), &router, &mut vars);
        let outcome = run_scenario_document(&doc, &router, &mut vars, Some(&catalog)).await;
        {
            let mut guard = camel_component_api::test_support::acquire_deadline(
                &ctx,
                "redis state ctx",
                Duration::from_secs(10),
            )
            .await;
            run.boot.shutdown(&mut guard).await.expect("shutdown");
        }
        cmd(&c, "DEL", &[k]).await;
        assert!(outcome.per_action.iter().all(Result::is_ok), "{outcome:?}");
        assert_eq!(outcome.verdict, Some(ScenarioVerdict::Pass));
        assert!(outcome.final_failure.is_none());
    });
}
#[test]
fn redis_datasource_create_succeeds_and_handles_ping() {
    run(async {
        let u = url().await;
        let (_, mut run) = boot(
            "rc-redis-state:redis_datasource_create_succeeds_and_handles_ping",
            &u,
        )
        .await;
        let handle = run
            .boot
            .datasource_catalog()
            .get_pool("statedb")
            .await
            .expect("pool");
        let c = handle
            .downcast::<redis::aio::MultiplexedConnection>()
            .expect("driver");
        assert_eq!(
            redis::cmd("PING")
                .query_async::<String>(&mut (*c).clone())
                .await
                .expect("ping"),
            "PONG"
        );
        shutdown(&mut run).await;
    });
}
#[test]
fn missing_key_is_validation_mismatch_at_expiry() {
    run(async {
        let k = "rc-redis-state:missing_key_is_validation_mismatch_at_expiry";
        let u = url().await;
        let c = connection(&u).await;
        cmd(&c, "DEL", &[k]).await;
        let (mut doc, mut run) = boot(k, &u).await;
        if let ScenarioAction::Validate { deadline, .. } = &mut doc.scenario[1] {
            *deadline = None;
        }
        let outcome = validate_only(doc, &run).await;
        shutdown(&mut run).await;
        cmd(&c, "DEL", &[k]).await;
        let Err(ScenarioFailure::ValidationMismatch { detail, .. }) = &outcome.per_action[0] else {
            panic!("{outcome:?}")
        };
        assert!(detail.contains(k));
        assert!(detail.contains("declared string, observed none"));
    });
}
#[test]
fn wrong_type_at_expiry_is_validation_mismatch() {
    run(async {
        let k = "rc-redis-state:wrong_type_at_expiry_is_validation_mismatch";
        let u = url().await;
        let c = connection(&u).await;
        cmd(&c, "DEL", &[k]).await;
        cmd(&c, "SET", &[k, "TYPE_SECRET_VALUE"]).await;
        let (mut doc, mut run) = boot(k, &u).await;
        if let ScenarioAction::Validate {
            target: ScenarioTarget::Redis(t),
            expectation: ValidateExpectation::Rows(e),
            deadline,
            ..
        } = &mut doc.scenario[1]
        {
            t.r#type = RedisType::Hash;
            e.columns = Some(vec!["value".into()]);
            *deadline = None;
        }
        let outcome = validate_only(doc, &run).await;
        shutdown(&mut run).await;
        cmd(&c, "DEL", &[k]).await;
        let Err(ScenarioFailure::ValidationMismatch { detail, .. }) = &outcome.per_action[0] else {
            panic!("{outcome:?}")
        };
        assert!(detail.contains("declared hash, observed string"));
        assert!(!detail.contains("TYPE_SECRET_VALUE"));
    });
}
#[test]
fn driver_error_is_sanitized_live() {
    run(async {
        // ADR-0070 reserved-address exception: no port-probe TOCTOU.
        let u = "redis://user:URL_SECRET@127.0.0.1:1/0".to_string();
        let (doc, mut run) = boot("rc-redis-state:driver_error_is_sanitized_live", &u).await;
        let outcome = tokio::time::timeout(Duration::from_secs(10), validate_only(doc, &run))
            .await
            .expect("failure budget");
        shutdown(&mut run).await;
        let Err(ScenarioFailure::ActionTransport { source, .. }) = &outcome.per_action[0] else {
            panic!("{outcome:?}")
        };
        let detail = source.to_string();
        assert!(detail.contains("statedb"));
        assert!(!detail.contains(&u));
        assert!(!detail.contains("URL_SECRET"));
    });
}
#[test]
fn redis_single_catalog_invariant() {
    run(async {
        let u = url().await;
        let (_, mut run) = boot("rc-redis-state:redis_single_catalog_invariant", &u).await;
        let catalog = run.boot.datasource_catalog();
        let a = catalog
            .get_pool("statedb")
            .await
            .expect("a")
            .downcast::<redis::aio::MultiplexedConnection>()
            .expect("driver");
        let b = catalog
            .get_pool("statedb")
            .await
            .expect("b")
            .downcast::<redis::aio::MultiplexedConnection>()
            .expect("driver");
        assert!(Arc::ptr_eq(&a, &b));
        shutdown(&mut run).await;
    });
}
#[test]
fn redis_handle_is_drop_scoped_not_close_scoped() {
    run(async {
        let k = "rc-redis-state:redis_handle_is_drop_scoped_not_close_scoped";
        let u = url().await;
        let c = connection(&u).await;
        cmd(&c, "DEL", &[k]).await;
        cmd(&c, "SET", &[k, "v"]).await;
        let (_, mut first) = boot(k, &u).await;
        let a = first
            .boot
            .datasource_catalog()
            .get_pool("statedb")
            .await
            .expect("a")
            .downcast::<redis::aio::MultiplexedConnection>()
            .expect("driver");
        shutdown(&mut first).await;
        drop(first);
        let (_, mut second) = boot(k, &u).await;
        let b = second
            .boot
            .datasource_catalog()
            .get_pool("statedb")
            .await
            .expect("b")
            .downcast::<redis::aio::MultiplexedConnection>()
            .expect("driver");
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(
            redis::cmd("GET")
                .arg(k)
                .query_async::<String>(&mut (*b).clone())
                .await
                .expect("get"),
            "v"
        );
        shutdown(&mut second).await;
        cmd(&c, "DEL", &[k]).await;
    });
}
#[test]
fn shutdown_closes_pools_without_releasing_redis() {
    run(async {
        let k = "rc-redis-state:shutdown_closes_pools_without_releasing_redis";
        let u = url().await;
        let c = connection(&u).await;
        cmd(&c, "DEL", &[k]).await;
        cmd(&c, "SET", &[k, "order-value"]).await;
        let (doc, mut run) = boot(k, &u).await;
        assert_eq!(
            validate_only(doc, &run).await.verdict,
            Some(ScenarioVerdict::Pass)
        );
        let handle = run
            .boot
            .datasource_catalog()
            .get_pool("statedb")
            .await
            .expect("pool")
            .downcast::<redis::aio::MultiplexedConnection>()
            .expect("driver");
        shutdown(&mut run).await;
        assert_eq!(
            redis::cmd("GET")
                .arg(k)
                .query_async::<String>(&mut (*handle).clone())
                .await
                .expect("get after shutdown"),
            "order-value"
        );
        cmd(&c, "DEL", &[k]).await;
    });
}
