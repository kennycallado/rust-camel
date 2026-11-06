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
/// Document-level `finally:` teardown (scenario-finally-hooks task
/// 1.3): the redis delete action removes exactly the listed literal
/// keys through the boot's catalog handle — a sentinel key outside the
/// list survives (no FLUSHDB, no pattern delete).
#[test]
fn finally_redis_del_removes_exactly_the_listed_keys() {
    run(async {
        let k = "rc-redis-state:finally_redis_del_removes_exactly_the_listed_keys:k1";
        let sentinel = "rc-redis-state:finally_redis_del_removes_exactly_the_listed_keys:sentinel";
        let u = url().await;
        let c = connection(&u).await;
        cmd(&c, "DEL", &[k, sentinel]).await;
        let (mut doc, mut run) = boot(k, &u).await;
        let catalog = run.boot.datasource_catalog();
        doc.finally = Some(vec![ScenarioAction::RedisDel {
            datasource: "statedb".to_string(),
            keys: vec![k.to_string()],
        }]);
        cmd(&c, "SET", &[k, "teardown-me"]).await;
        cmd(&c, "SET", &[sentinel, "keep-me"]).await;
        let Some(ScenarioAction::RedisDel { datasource, keys }) =
            doc.finally.as_ref().and_then(|items| items.first())
        else {
            panic!("the constructed finally list must hold one RedisDel");
        };
        let action = RedisDelAction {
            datasource: datasource.clone(),
            keys: keys.clone(),
        };
        execute_redis_del(&catalog, &action)
            .await
            .expect("delete through the boot's catalog");
        let deleted = redis::cmd("GET")
            .arg(k)
            .query_async::<Option<String>>(&mut c.clone())
            .await
            .expect("get deleted key");
        let kept = redis::cmd("GET")
            .arg(sentinel)
            .query_async::<String>(&mut c.clone())
            .await
            .expect("get sentinel key");
        shutdown(&mut run).await;
        cmd(&c, "DEL", &[k, sentinel]).await;
        assert_eq!(deleted, None, "the listed key must be gone");
        assert_eq!(kept, "keep-me", "the unlisted key must survive");
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

/// Document-level `finally:` `unlinkPrefix` teardown (bd rc-ql8eq,
/// task 4.1): only keys under the harness-minted namespace prefix are
/// unlinked. Sentinels outside the run id and differing only after the
/// ordinal segment survive, because the prefix-terminating `~` is part
/// of the scope.
#[test]
fn unlink_prefix_deletes_only_under_the_namespace() {
    run(async {
        let ns = "112~it~3f9a1c0e~0001~";
        let k1 = format!("{ns}state:a");
        let k2 = format!("{ns}state:b");
        // Out-of-namespace sentinel: a different run id.
        let outside = "112~it~deadbeef~0001~state:a".to_string();
        // Suffix trap: same base and run id, different ordinal.
        let trap = "112~it~3f9a1c0e~0002~state:a".to_string();
        let u = url().await;
        let c = connection(&u).await;
        for key in [&k1, &k2, &outside, &trap] {
            cmd(&c, "DEL", &[key.as_str()]).await;
            cmd(&c, "SET", &[key.as_str(), "v"]).await;
        }

        let (mut doc, mut run) = boot(
            "rc-redis-state:unlink_prefix_deletes_only_under_the_namespace",
            &u,
        )
        .await;
        let catalog = run.boot.datasource_catalog();
        doc.scenario = vec![ScenarioAction::Sleep {
            duration: Duration::from_millis(1),
        }];
        doc.finally = Some(vec![ScenarioAction::RedisUnlinkPrefix {
            datasource: "statedb".to_string(),
            template: "${env:NS}state:".to_string(),
        }]);
        doc.minted_namespaces
            .insert("NS".to_string(), ns.to_string());

        let router = common::router_for("fake:unused", FakeAdapter::scripted(Vec::new()));
        let outcome =
            run_scenario_document(&doc, &router, &mut ScenarioVars::new(), Some(&catalog)).await;

        let mut exists = redis::cmd("EXISTS");
        exists.arg(&k1).arg(&k2);
        let scoped: i64 = exists
            .query_async(&mut c.clone())
            .await
            .expect("exists in-namespace");
        let mut exists = redis::cmd("EXISTS");
        exists.arg(&outside).arg(&trap);
        let sentinels: i64 = exists
            .query_async(&mut c.clone())
            .await
            .expect("exists sentinels");

        shutdown(&mut run).await;
        for key in [&k1, &k2, &outside, &trap] {
            cmd(&c, "DEL", &[key.as_str()]).await;
        }

        assert_eq!(outcome.verdict, Some(ScenarioVerdict::Pass), "{outcome:?}");
        assert_eq!(
            outcome.per_teardown.first(),
            Some(&Ok(())),
            "the teardown must succeed: {outcome:?}"
        );
        assert_eq!(scoped, 0, "both in-namespace keys must be gone");
        assert_eq!(sentinels, 2, "both out-of-namespace sentinels must survive");
    });
}

/// Executor limits (bd rc-ql8eq, task 4.1): the key cap slices the
/// validated reply precisely and reports the deleted count; the
/// explicit zero-deadline guard fires before any SCAN and reports
/// zero deleted.
#[test]
fn unlink_prefix_executor_cap_breach() {
    run(async {
        let ns = "112~it~3f9a1c0e~0003~";
        let prefix = format!("{ns}state:");
        let keys: Vec<String> = (0..5).map(|index| format!("{prefix}{index}")).collect();
        let u = url().await;
        let c = connection(&u).await;
        let (_, mut run) = boot("rc-redis-state:unlink_prefix_executor_cap_breach", &u).await;
        let catalog = run.boot.datasource_catalog();

        for key in &keys {
            cmd(&c, "SET", &[key.as_str(), "v"]).await;
        }
        let err = execute_redis_unlink_prefix(
            &catalog,
            "statedb",
            &prefix,
            &UnlinkLimits {
                deadline: Duration::from_secs(10),
                key_cap: 2,
            },
        )
        .await
        .expect_err("a key cap of 2 over five keys must breach");
        assert!(err.contains("cap"), "error must name the cap: {err}");
        assert!(
            err.contains("2 keys deleted"),
            "error must report exactly two deleted: {err}"
        );
        let mut exists = redis::cmd("EXISTS");
        for key in &keys {
            exists.arg(key);
        }
        let remaining: i64 = exists
            .query_async(&mut c.clone())
            .await
            .expect("exists after cap breach");

        for key in &keys {
            cmd(&c, "DEL", &[key.as_str()]).await;
            cmd(&c, "SET", &[key.as_str(), "v"]).await;
        }
        let err = execute_redis_unlink_prefix(
            &catalog,
            "statedb",
            &prefix,
            &UnlinkLimits {
                deadline: Duration::ZERO,
                key_cap: 100_000,
            },
        )
        .await
        .expect_err("a zero deadline must fail immediately");
        assert!(
            err.contains("deadline"),
            "error must name the deadline: {err}"
        );
        assert!(
            err.contains("0 keys deleted"),
            "error must report zero deleted: {err}"
        );
        let mut exists = redis::cmd("EXISTS");
        for key in &keys {
            exists.arg(key);
        }
        let surviving: i64 = exists
            .query_async(&mut c.clone())
            .await
            .expect("exists after zero deadline");

        shutdown(&mut run).await;
        for key in &keys {
            cmd(&c, "DEL", &[key.as_str()]).await;
        }

        assert_eq!(remaining, 3, "cap 2 over five keys leaves exactly three");
        assert_eq!(surviving, 5, "the zero-deadline guard deletes nothing");
    });
}

/// A failing unlink-prefix teardown is apparatus-class
/// `TeardownFailure`, names the `finally` action, and never masks the
/// recorded verdict (the finally-loop contract).
#[test]
fn unlink_prefix_teardown_failure_rendering() {
    run(async {
        let u = url().await;
        let (mut doc, mut run) = boot(
            "rc-redis-state:unlink_prefix_teardown_failure_rendering",
            &u,
        )
        .await;
        let catalog = run.boot.datasource_catalog();
        doc.scenario = vec![ScenarioAction::Sleep {
            duration: Duration::from_millis(1),
        }];
        doc.finally = Some(vec![ScenarioAction::RedisUnlinkPrefix {
            datasource: "missing_db".to_string(),
            template: "${env:NS}state:".to_string(),
        }]);
        doc.minted_namespaces
            .insert("NS".to_string(), "112~it~3f9a1c0e~0004~".to_string());

        let router = common::router_for("fake:unused", FakeAdapter::scripted(Vec::new()));
        let outcome =
            run_scenario_document(&doc, &router, &mut ScenarioVars::new(), Some(&catalog)).await;
        shutdown(&mut run).await;

        assert_eq!(
            outcome.verdict,
            Some(ScenarioVerdict::Pass),
            "a teardown failure must not mask the verdict: {outcome:?}"
        );
        let Some(ScenarioFailure::TeardownFailure { message }) = outcome.final_failure.as_ref()
        else {
            panic!("expected TeardownFailure, got {outcome:?}");
        };
        assert!(
            message.contains("finally action 0"),
            "the message must name the finally position: {message}"
        );
        assert!(
            message.contains("redis unlink-prefix"),
            "the message must name the action: {message}"
        );
        assert!(
            matches!(
                outcome.per_teardown.first(),
                Some(Err(ScenarioFailure::TeardownFailure { .. }))
            ),
            "the per-item outcome must be the uniform teardown failure: {outcome:?}"
        );
    });
}
