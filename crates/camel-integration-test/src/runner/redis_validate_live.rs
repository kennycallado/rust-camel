//! Docker-backed snapshot tests; kept separate from the executor.
#[cfg(test)]
mod tests {
    use super::super::*;
    use camel_matchers::Expectation;
    use serde_json::json;
    use std::sync::{LazyLock, Mutex};
    use testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
    use testcontainers_modules::redis::Redis;
    use tokio::sync::{OnceCell, mpsc};

    static RUNTIME: LazyLock<tokio::runtime::Runtime> =
        LazyLock::new(|| tokio::runtime::Runtime::new().expect("runtime"));
    static CONTAINER: OnceCell<ContainerAsync<Redis>> = OnceCell::const_new();

    fn run<F: std::future::Future>(future: F) -> F::Output {
        RUNTIME.block_on(async {
            tokio::time::timeout(Duration::from_secs(120), future)
                .await
                .expect("Redis test, including setup and cleanup, exceeded 120s")
        })
    }

    async fn connection() -> redis::aio::MultiplexedConnection {
        let container = CONTAINER
            .get_or_init(|| async {
                Redis::default()
                    .with_tag("7-alpine")
                    .start()
                    .await
                    .expect("container")
            })
            .await;
        let port = container.get_host_port_ipv4(6379).await.expect("port");
        redis::Client::open(format!("redis://127.0.0.1:{port}/0"))
            .expect("client")
            .get_multiplexed_async_connection()
            .await
            .expect("connect")
    }

    async fn command(conn: &redis::aio::MultiplexedConnection, name: &str, args: &[&str]) {
        redis::cmd(name)
            .arg(args)
            .query_async::<redis::Value>(&mut conn.clone())
            .await
            .expect("seed/mutate");
    }
    fn target(key: &str, kind: RedisType) -> RedisTarget {
        RedisTarget {
            datasource: "statedb".into(),
            key: key.into(),
            r#type: kind,
            ttl: None,
        }
    }
    fn rows(columns: Option<&[&str]>, values: Vec<Vec<Value>>) -> RowsExpectation {
        RowsExpectation {
            columns: columns.map(|c| c.iter().map(|s| (*s).into()).collect()),
            unordered: false,
            rows: Some(
                values
                    .into_iter()
                    .map(|row| row.into_iter().map(Expectation::Equals).collect())
                    .collect(),
            ),
            bound: None,
        }
    }
    fn count(bound: CountBound) -> RowsExpectation {
        RowsExpectation {
            columns: None,
            unordered: false,
            rows: None,
            bound: Some(bound),
        }
    }
    async fn validate(
        conn: &redis::aio::MultiplexedConnection,
        target: &RedisTarget,
        expected: &RowsExpectation,
        deadline: Option<Duration>,
    ) -> Result<(), ScenarioFailure> {
        poll_with_source(0, target, expected, deadline, || {
            eval_raw(
                conn,
                &target.key,
                snapshot_script(target.r#type),
                0,
                "statedb",
                "",
            )
        })
        .await
    }

    // Run controller and poll as borrowed futures: timeout cancels both, so
    // no detached writer or validation task can survive a failed handshake.
    async fn phased(
        conn: &redis::aio::MultiplexedConnection,
        target: &RedisTarget,
        expected: &RowsExpectation,
        mutations: Vec<Vec<(&str, Vec<&str>)>>,
    ) -> (
        Result<(), ScenarioFailure>,
        Vec<(Option<RedisType>, RedisTtlStatus)>,
    ) {
        let (result, samples) = phased_checked(conn, target, expected, mutations, None)
            .await
            .expect("coordination");
        assert!(
            samples.len() >= 2,
            "must acquire again after acknowledged first snapshot"
        );
        (result, samples)
    }

    #[derive(Clone, Copy)]
    enum CoordinationFailure {
        Controller,
        Ack,
    }
    type PhaseResult = (
        Result<(), ScenarioFailure>,
        Vec<(Option<RedisType>, RedisTtlStatus)>,
    );

    async fn phased_checked(
        conn: &redis::aio::MultiplexedConnection,
        target: &RedisTarget,
        expected: &RowsExpectation,
        mutations: Vec<Vec<(&str, Vec<&str>)>>,
        injected: Option<CoordinationFailure>,
    ) -> Result<PhaseResult, String> {
        let observations = Arc::new(Mutex::new(Vec::new()));
        let (sample_tx, mut sample_rx) = mpsc::channel(2);
        let (ack_tx, ack_rx) = mpsc::channel(2);
        let ack_rx = Arc::new(tokio::sync::Mutex::new(ack_rx));
        let number = mutations.len();
        let samples = observations.clone();
        let poll = poll_with_source(
            0,
            target,
            expected,
            Some(Duration::from_secs(3)),
            move || {
                let samples = samples.clone();
                let sample_tx = sample_tx.clone();
                let ack_rx = ack_rx.clone();
                async move {
                    let raw = eval_raw(
                        conn,
                        &target.key,
                        snapshot_script(target.r#type),
                        0,
                        "statedb",
                        "",
                    )
                    .await?;
                    let snapshot = decode_snapshot(0, target, expected, raw.clone())?;
                    let index = {
                        let mut seen = samples.lock().expect("observations");
                        seen.push((snapshot.observed, snapshot.ttl));
                        seen.len()
                    };
                    if index <= number {
                        sample_tx
                            .send(index)
                            .await
                            .map_err(|_| apparatus(0, "statedb", "controller closed".into()))?;
                        tokio::time::timeout(Duration::from_secs(1), async {
                            ack_rx.lock().await.recv().await
                        })
                        .await
                        .map_err(|_| apparatus(0, "statedb", "ack budget exceeded".into()))?
                        .ok_or_else(|| apparatus(0, "statedb", "ack closed".into()))?;
                    }
                    Ok(raw)
                }
            },
        );
        let controller = async {
            for (index, commands) in mutations.into_iter().enumerate() {
                tokio::time::timeout(Duration::from_secs(1), async {
                    if sample_rx.recv().await != Some(index + 1) {
                        return Err("unexpected acquisition index".to_string());
                    }
                    if matches!(injected, Some(CoordinationFailure::Controller)) {
                        return Err("injected controller failure".to_string());
                    }
                    if matches!(injected, Some(CoordinationFailure::Ack)) {
                        std::future::pending::<()>().await;
                    }
                    for (name, args) in commands {
                        redis::cmd(name)
                            .arg(&args)
                            .query_async::<redis::Value>(&mut conn.clone())
                            .await
                            .map_err(|e| e.to_string())?;
                    }
                    ack_tx
                        .send(())
                        .await
                        .map_err(|_| "poll closed".to_string())?;
                    Ok::<(), String>(())
                })
                .await
                .map_err(|_| "controller budget exceeded".to_string())??;
            }
            Ok::<(), String>(())
        };
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            let (result, coordination) = tokio::join!(poll, controller);
            coordination.map(|()| result)
        })
        .await;
        // Both joined futures have been dropped before bounded cleanup, including
        // every timeout/error path. Preserve the original coordination error.
        let cleanup = tokio::time::timeout(
            Duration::from_secs(1),
            redis::cmd("DEL")
                .arg(&target.key)
                .query_async::<redis::Value>(&mut conn.clone()),
        )
        .await;
        let result = result.map_err(|_| "poll budget exceeded".to_string())??;
        cleanup
            .map_err(|_| "cleanup budget exceeded".to_string())?
            .map_err(|e| e.to_string())?;
        let samples = observations.lock().expect("observations").clone();
        Ok((result, samples))
    }

    #[test]
    fn live_controller_failure_cleans_key_within_budget() {
        run(async {
            let c = connection().await;
            let k = "rc-redis-state:live_controller_failure_cleans_key_within_budget";
            command(&c, "DEL", &[k]).await;
            command(&c, "SET", &[k, "v"]).await;
            let start = std::time::Instant::now();
            let result = phased_checked(
                &c,
                &target(k, RedisType::String),
                &count(CountBound::AtLeast(0)),
                vec![vec![]],
                Some(CoordinationFailure::Controller),
            )
            .await;
            assert!(
                result
                    .expect_err("controller fails")
                    .contains("injected controller failure")
            );
            assert!(start.elapsed() < Duration::from_secs(12));
            assert_eq!(
                tokio::time::timeout(
                    Duration::from_secs(1),
                    redis::cmd("EXISTS")
                        .arg(k)
                        .query_async::<u64>(&mut c.clone())
                )
                .await
                .expect("read budget")
                .expect("exists"),
                0
            );
        });
    }
    #[test]
    fn live_ack_timeout_cleans_key_within_budget() {
        run(async {
            let c = connection().await;
            let k = "rc-redis-state:live_ack_timeout_cleans_key_within_budget";
            command(&c, "DEL", &[k]).await;
            command(&c, "SET", &[k, "v"]).await;
            let start = std::time::Instant::now();
            let result = phased_checked(
                &c,
                &target(k, RedisType::String),
                &count(CountBound::AtLeast(0)),
                vec![vec![]],
                Some(CoordinationFailure::Ack),
            )
            .await;
            assert!(
                result
                    .expect_err("ack fails")
                    .contains("controller budget exceeded")
            );
            assert!(start.elapsed() < Duration::from_secs(12));
            assert_eq!(
                tokio::time::timeout(
                    Duration::from_secs(1),
                    redis::cmd("EXISTS")
                        .arg(k)
                        .query_async::<u64>(&mut c.clone())
                )
                .await
                .expect("read budget")
                .expect("exists"),
                0
            );
        });
    }

    #[test]
    fn live_initially_missing_key_appears_within_deadline_passes() {
        run(async {
            let c = connection().await;
            let k = "rc-redis-state:live_initially_missing_key_appears_within_deadline_passes";
            command(&c, "DEL", &[k]).await;
            let (result, seen) = phased(
                &c,
                &target(k, RedisType::String),
                &rows(None, vec![vec![json!("v")]]),
                vec![vec![("SET", vec![k, "v"])]],
            )
            .await;
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(seen[0].0, None);
        });
    }
    #[test]
    fn live_transient_wrong_type_corrected_within_deadline_passes() {
        run(async {
            let c = connection().await;
            let k = "rc-redis-state:live_transient_wrong_type_corrected_within_deadline_passes";
            command(&c, "DEL", &[k]).await;
            command(&c, "SET", &[k, "v"]).await;
            let (result, seen) = phased(
                &c,
                &target(k, RedisType::Hash),
                &rows(Some(&["value"]), vec![vec![json!("alice")]]),
                vec![vec![("DEL", vec![k]), ("HSET", vec![k, "name", "alice"])]],
            )
            .await;
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(seen[0].0, Some(RedisType::String));
        });
    }
    #[test]
    fn live_no_early_settle_first_present_then_deleted_final_snapshot_fails() {
        run(async {
            let c = connection().await;
            let k = "rc-redis-state:live_no_early_settle_first_present_then_deleted_final_snapshot_fails";
            command(&c, "DEL", &[k]).await;
            command(&c, "SET", &[k, "v"]).await;
            let (result, seen) = phased(
                &c,
                &target(k, RedisType::String),
                &rows(None, vec![vec![json!("v")]]),
                vec![vec![("DEL", vec![k])]],
            )
            .await;
            assert!(matches!(
                result,
                Err(ScenarioFailure::ValidationMismatch { .. })
            ));
            assert_eq!(seen[0].0, Some(RedisType::String));
            assert_eq!(seen.last().expect("final").0, None);
        });
    }
    #[test]
    fn live_ttl_decided_at_deadline_final_snapshot() {
        run(async {
            let c = connection().await;
            let k = "rc-redis-state:live_ttl_decided_at_deadline_final_snapshot";
            command(&c, "DEL", &[k]).await;
            command(&c, "SET", &[k, "v", "PX", "60000"]).await;
            let mut t = target(k, RedisType::String);
            t.ttl = Some(CountBound::AtLeast(30000));
            let (result, seen) = phased(
                &c,
                &t,
                &rows(None, vec![vec![json!("v")]]),
                vec![vec![("PERSIST", vec![k])]],
            )
            .await;
            assert!(matches!(seen[0].1,RedisTtlStatus::Remaining(n) if n>=30000));
            assert_eq!(seen.last().expect("final").1, RedisTtlStatus::Persistent);
            let Err(ScenarioFailure::ValidationMismatch { detail, .. }) = result else {
                panic!("expected mismatch")
            };
            assert!(detail.contains("at least 30000"));
            assert!(detail.contains("persistent"));
        });
    }
    #[test]
    fn live_mutated_key_is_observed_as_one_coherent_snapshot() {
        run(async {
            let c = connection().await;
            let k = "rc-redis-state:live_mutated_key_is_observed_as_one_coherent_snapshot";
            command(&c, "DEL", &[k]).await;
            command(&c, "SET", &[k, "pre"]).await;
            let (result, seen) = phased(
                &c,
                &target(k, RedisType::String),
                &count(CountBound::AtLeast(0)),
                vec![vec![("DEL", vec![k]), ("HSET", vec![k, "f", "v"])], vec![]],
            )
            .await;
            assert_eq!(seen[0].0, Some(RedisType::String));
            assert_eq!(seen[1].0, Some(RedisType::Hash));
            assert!(
                result.is_ok() || matches!(result, Err(ScenarioFailure::ValidationMismatch { .. })),
                "{result:?}"
            );
        });
    }
    #[test]
    fn live_projection_failure_is_apparatus_and_stops_immediately() {
        run(async {
            let c = connection().await;
            let k = "rc-redis-state:live_projection_failure_is_apparatus_and_stops_immediately";
            command(&c, "DEL", &[k]).await;
            redis::cmd("SET")
                .arg(k)
                .arg(&[0xffu8, 0xfe][..])
                .query_async::<()>(&mut c.clone())
                .await
                .expect("binary seed");
            let calls = std::cell::Cell::new(0);
            let t = target(k, RedisType::String);
            let expected = count(CountBound::AtLeast(0));
            let start = std::time::Instant::now();
            let result = poll_with_source(0, &t, &expected, Some(Duration::from_secs(2)), || {
                calls.set(calls.get() + 1);
                eval_raw(&c, k, snapshot_script(t.r#type), 0, "statedb", "")
            })
            .await;
            command(&c, "DEL", &[k]).await;
            assert!(matches!(
                result,
                Err(ScenarioFailure::ActionTransport { .. })
            ));
            assert_eq!(calls.get(), 1);
            assert!(start.elapsed() < Duration::from_secs(1));
        });
    }

    #[test]
    fn live_projections_all_types() {
        run(async {
            let c = connection().await;
            let cases = [
                (
                    RedisType::String,
                    "SET",
                    vec!["alice"],
                    vec![vec![json!("alice")]],
                ),
                (
                    RedisType::Hash,
                    "HSET",
                    vec!["name", "alice"],
                    vec![vec![json!("name"), json!("alice")]],
                ),
                (
                    RedisType::List,
                    "RPUSH",
                    vec!["a", "b"],
                    vec![vec![json!(0), json!("a")], vec![json!(1), json!("b")]],
                ),
                (
                    RedisType::Set,
                    "SADD",
                    vec!["a", "b", "c"],
                    vec![vec![json!("a")], vec![json!("b")], vec![json!("c")]],
                ),
                (
                    RedisType::Zset,
                    "ZADD",
                    vec!["1.5", "bob", "2", "alice"],
                    vec![
                        vec![json!("bob"), json!(1.5)],
                        vec![json!("alice"), json!(2)],
                    ],
                ),
            ];
            for (kind, name, args, values) in cases {
                let k = format!(
                    "rc-redis-state:live_projections_all_types:{}",
                    kind.as_str()
                );
                command(&c, "DEL", &[&k]).await;
                let mut seed = vec![k.as_str()];
                seed.extend(args);
                command(&c, name, &seed).await;
                let result = validate(&c, &target(&k, kind), &rows(None, values), None).await;
                command(&c, "DEL", &[&k]).await;
                assert!(result.is_ok(), "{result:?}");
            }
        });
    }
    #[test]
    fn live_columns_subset_projection() {
        run(async {
            let c = connection().await;
            let k = "rc-redis-state:live_columns_subset_projection";
            command(&c, "DEL", &[k]).await;
            command(&c, "HSET", &[k, "name", "alice", "age", "42"]).await;
            let mut e = rows(
                Some(&["value"]),
                vec![vec![json!("42")], vec![json!("alice")]],
            );
            e.unordered = true;
            let result = validate(&c, &target(k, RedisType::Hash), &e, None).await;
            command(&c, "DEL", &[k]).await;
            assert!(result.is_ok(), "{result:?}");
        });
    }
    #[test]
    fn live_columns_reorder_projection() {
        run(async {
            let c = connection().await;
            let k = "rc-redis-state:live_columns_reorder_projection";
            command(&c, "DEL", &[k]).await;
            command(&c, "HSET", &[k, "name", "alice", "age", "42"]).await;
            let result = validate(
                &c,
                &target(k, RedisType::Hash),
                &rows(
                    Some(&["value", "field"]),
                    vec![
                        vec![json!("42"), json!("age")],
                        vec![json!("alice"), json!("name")],
                    ],
                ),
                None,
            )
            .await;
            command(&c, "DEL", &[k]).await;
            assert!(result.is_ok(), "{result:?}");
        });
    }
    #[test]
    fn live_wildcard_ignore_cell_matches_any_value() {
        run(async {
            let c = connection().await;
            let k = "rc-redis-state:live_wildcard_ignore_cell_matches_any_value";
            command(&c, "DEL", &[k]).await;
            command(&c, "HSET", &[k, "name", "alice"]).await;
            let mut e = rows(None, vec![vec![json!("unused"), json!("alice")]]);
            e.rows.as_mut().expect("rows")[0][0] = Expectation::Any;
            let result = validate(&c, &target(k, RedisType::Hash), &e, None).await;
            command(&c, "DEL", &[k]).await;
            assert!(result.is_ok(), "{result:?}");
        });
    }
    #[test]
    fn live_integral_and_non_integral_scores_match() {
        run(async {
            let c = connection().await;
            let k = "rc-redis-state:live_integral_and_non_integral_scores_match";
            command(&c, "DEL", &[k]).await;
            command(&c, "ZADD", &[k, "1.5", "bob", "2", "alice"]).await;
            let result = validate(
                &c,
                &target(k, RedisType::Zset),
                &rows(
                    None,
                    vec![
                        vec![json!("bob"), json!(1.5)],
                        vec![json!("alice"), json!(2)],
                    ],
                ),
                None,
            )
            .await;
            command(&c, "DEL", &[k]).await;
            assert!(result.is_ok(), "{result:?}");
        });
    }
    #[test]
    fn live_ttl_at_least_passes_and_at_most_fails() {
        run(async {
            let c = connection().await;
            let k = "rc-redis-state:live_ttl_at_least_passes_and_at_most_fails";
            command(&c, "DEL", &[k]).await;
            command(&c, "SET", &[k, "SECRET_VALUE", "EX", "60"]).await;
            let mut t = target(k, RedisType::String);
            let e = count(CountBound::AtLeast(1));
            t.ttl = Some(CountBound::AtLeast(30000));
            assert!(validate(&c, &t, &e, None).await.is_ok());
            t.ttl = Some(CountBound::AtMost(30000));
            let result = validate(&c, &t, &e, None).await;
            command(&c, "DEL", &[k]).await;
            let Err(ScenarioFailure::ValidationMismatch { detail, .. }) = result else {
                panic!("expected mismatch")
            };
            assert!(detail.contains("at most 30000"));
            assert!(detail.contains("remaining"));
            assert!(!detail.contains("SECRET_VALUE"));
        });
    }
    #[test]
    fn live_persistent_and_missing_keys_fail_ttl_bound() {
        run(async {
            let c = connection().await;
            let k = "rc-redis-state:live_persistent_and_missing_keys_fail_ttl_bound";
            command(&c, "DEL", &[k]).await;
            command(&c, "SET", &[k, "v"]).await;
            let mut t = target(k, RedisType::String);
            t.ttl = Some(CountBound::AtLeast(1000));
            let e = count(CountBound::AtLeast(0));
            let Err(ScenarioFailure::ValidationMismatch { detail, .. }) =
                validate(&c, &t, &e, None).await
            else {
                panic!("persistent mismatch")
            };
            assert!(detail.contains("persistent"));
            command(&c, "DEL", &[k]).await;
            let Err(ScenarioFailure::ValidationMismatch { detail, .. }) =
                validate(&c, &t, &e, None).await
            else {
                panic!("missing mismatch")
            };
            assert!(detail.contains(k));
            assert!(detail.contains("observed none"));
        });
    }
    #[test]
    fn live_ceiling_breach_fails_immediately() {
        run(async {
            let c = connection().await;
            let k = "rc-redis-state:live_ceiling_breach_fails_immediately";
            command(&c, "DEL", &[k]).await;
            command(&c, "SADD", &[k, "a", "b"]).await;
            let start = std::time::Instant::now();
            let result = validate(
                &c,
                &target(k, RedisType::Set),
                &count(CountBound::AtMost(1)),
                Some(Duration::from_secs(2)),
            )
            .await;
            command(&c, "DEL", &[k]).await;
            assert!(matches!(
                result,
                Err(ScenarioFailure::ValidationMismatch { .. })
            ));
            assert!(start.elapsed() < Duration::from_secs(1));
        });
    }
}
