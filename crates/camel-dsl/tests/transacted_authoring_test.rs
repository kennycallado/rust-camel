//! Authoring tests for the `transacted:` block (sqltrans, slice 1).
//!
//! Covers the three authoring legs — YAML/builder/canonical — and the
//! strictness rule that an unknown key inside a block is rejected by name.
//! Dark contract: this slice only compiles the block into
//! `BuilderStep::Transacted`. The YAML and canonical legs compile through the
//! validated seam `compile_declarative_route_with` with a test resolver;
//! production loading wires the registry resolver in Task 4.1.

use std::sync::Arc;
use std::time::Duration;

use camel_api::transaction::{OpenSessionFuture, TransactionManager};
use camel_builder::RouteBuilder;
use camel_core::route::BuilderStep;
use camel_dsl::{
    DeclarativeStep, TransactedStepDef, compile_declarative_route_to_canonical,
    compile_declarative_route_with, parse_canonical_json, parse_json_to_declarative,
    parse_yaml_to_declarative,
};

/// Minimal manager for the validated compile seam; compilation never opens a
/// session, so `open_session` is unreachable by construction.
struct FakeManager {
    datasource: &'static str,
}

impl TransactionManager for FakeManager {
    fn datasource(&self) -> &str {
        self.datasource
    }

    fn open_session(&self) -> OpenSessionFuture<'_> {
        panic!("compilation must never open a transaction session");
    }

    fn acquire_deadline(&self) -> Duration {
        Duration::from_secs(1)
    }
}

/// Test resolver for the `pg` block datasource; nothing else is registered.
fn test_resolver() -> impl Fn(&str) -> Option<Arc<dyn TransactionManager>> {
    let manager: Arc<dyn TransactionManager> = Arc::new(FakeManager { datasource: "pg" });
    move |datasource| (datasource == "pg").then(|| manager.clone())
}

const TRANSACTED_YAML: &str = r#"
routes:
  - id: tx-route
    from: direct:start
    steps:
      - transacted:
          datasource: pg
          steps:
            - to: "sql:select 1?datasource=pg"
            - to: "sql:insert into t values (1)?datasource=pg"
"#;

fn transacted_block(route: &camel_dsl::DeclarativeRoute) -> &TransactedStepDef {
    match route.steps.as_slice() {
        [DeclarativeStep::Transacted(block)] => block,
        other => panic!("expected a single transacted block, got {other:?}"),
    }
}

#[test]
fn yaml_transacted_block_parses() {
    let routes = parse_yaml_to_declarative(TRANSACTED_YAML).expect("transacted YAML must parse");
    assert_eq!(routes.len(), 1);
    let block = transacted_block(&routes[0]);
    assert_eq!(block.datasource, "pg");
    assert_eq!(block.steps.len(), 2);
    assert!(
        matches!(&block.steps[0], DeclarativeStep::To(to) if to.uri == "sql:select 1?datasource=pg")
    );
    assert!(
        matches!(&block.steps[1], DeclarativeStep::To(to) if to.uri == "sql:insert into t values (1)?datasource=pg")
    );
}

#[test]
fn transacted_unknown_key_rejected() {
    // `RouteDslStep` is untagged and would swallow the key name, so the error
    // must come from the raw-key pre-scan.
    let yaml = TRANSACTED_YAML.replace(
        "          datasource: pg",
        "          datasource: pg\n          bogus: 1",
    );
    let err = parse_yaml_to_declarative(&yaml).expect_err("unknown key must be rejected");
    let message = err.to_string();
    assert!(
        message.contains("bogus"),
        "expected the unknown key to be named, got: {message}"
    );
}

#[test]
fn json_transacted_unknown_key_rejected() {
    // The JSON loader runs the same raw pre-scan: a typo inside a block is
    // named, not swallowed by the untagged step enum.
    let json = r#"
    {
        "routes": [
            {
                "id": "tx-json",
                "from": "direct:start",
                "steps": [
                    {
                        "transacted": {
                            "datasource": "pg",
                            "bogus": 1,
                            "steps": [{"to": "sql:select 1?datasource=pg"}]
                        }
                    }
                ]
            }
        ]
    }"#;
    let err = parse_json_to_declarative(json).expect_err("unknown key must be rejected");
    let message = err.to_string();
    assert!(
        message.contains("bogus"),
        "expected the unknown key to be named, got: {message}"
    );
}

#[test]
fn canonical_transacted_round_trip() {
    let routes = parse_yaml_to_declarative(TRANSACTED_YAML).expect("transacted YAML must parse");
    let direct_def = compile_declarative_route_with(routes[0].clone(), &test_resolver())
        .expect("block must compile");

    let (spec, loss) =
        compile_declarative_route_to_canonical(routes[0].clone(), false).expect("canonical emit");
    assert!(loss.is_none(), "a transacted block drops nothing");

    let json = serde_json::to_string(&serde_json::json!({ "routes": [spec] }))
        .expect("canonical spec must serialize");
    let canonical_def = &parse_canonical_json(&json).expect("canonical JSON must compile")[0];

    assert_eq!(
        format!("{:?}", canonical_def.steps()),
        format!("{:?}", direct_def.steps()),
        "canonical round-trip must preserve the compiled steps"
    );
}

#[test]
fn canonical_transacted_unknown_key_rejected() {
    let routes = parse_yaml_to_declarative(TRANSACTED_YAML).expect("transacted YAML must parse");
    let (spec, _loss) =
        compile_declarative_route_to_canonical(routes[0].clone(), false).expect("canonical emit");

    let mut document = serde_json::json!({ "routes": [spec] });
    let config = document["routes"][0]["steps"][0]["config"]
        .as_object_mut()
        .expect("transacted step carries an object config");
    config.insert("bogus".to_string(), serde_json::json!(1));

    let err = parse_canonical_json(&document.to_string())
        .err()
        .expect("unknown key inside transacted must be rejected");
    let message = err.to_string();
    assert!(
        message.contains("bogus"),
        "expected the unknown key to be named, got: {message}"
    );
}

#[test]
fn builder_matches_yaml_model() {
    let routes = parse_yaml_to_declarative(TRANSACTED_YAML).expect("transacted YAML must parse");
    let yaml_def = compile_declarative_route_with(routes[0].clone(), &test_resolver())
        .expect("block must compile");

    let built = RouteBuilder::from("direct:start")
        .route_id("tx-route")
        .transacted(
            "pg",
            [
                BuilderStep::To("sql:select 1?datasource=pg".to_string()),
                BuilderStep::To("sql:insert into t values (1)?datasource=pg".to_string()),
            ],
        )
        .build()
        .expect("builder route must build");

    assert_eq!(
        format!("{:?}", built.steps()),
        format!("{:?}", yaml_def.steps()),
        "builder-authored block must match the YAML-compiled model"
    );
}
