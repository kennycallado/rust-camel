//! Dark-landing proof and authoring equivalence for the `transacted:` block
//! (sqltrans, slice 1, task 5.1).
//!
//! Slice 1 lands the block surface dark: production loading never registers a
//! `TransactionManager`, so a route carrying a `transacted:` block always fails
//! at load with the typed `ManagerMissing` error. This file pins that contract
//! (`transacted_block_fails_at_load_dark`) and proves that the three authoring
//! legs — YAML, canonical round-trip, and builder — lower to the same compiled
//! step model (`authoring_equivalence_under_fake_manager`).

use std::sync::Arc;
use std::time::Duration;

use camel_api::CamelError;
use camel_api::transaction::{OpenSessionFuture, TransactionError, TransactionManager};
use camel_builder::RouteBuilder;
use camel_core::route::BuilderStep;
use camel_dsl::{
    compile_declarative_route, compile_declarative_route_to_canonical,
    compile_declarative_route_with, parse_canonical_json, parse_yaml_to_declarative,
};

/// Full YAML route: `from` plus a block with two joined sql steps on `pg`.
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

/// Test-only manager. Compilation never opens a session (slice 1 has no
/// execution), so `open_session` is unreachable by construction.
struct FakeManager {
    datasource: &'static str,
}

impl TransactionManager for FakeManager {
    fn datasource(&self) -> &str {
        self.datasource
    }

    fn open_session(&self) -> OpenSessionFuture<'_> {
        panic!("not implemented in slice 1");
    }

    fn acquire_deadline(&self) -> Duration {
        Duration::from_secs(1)
    }
}

/// Resolver registering only the `pg` block datasource.
fn test_resolver() -> impl Fn(&str) -> Option<Arc<dyn TransactionManager>> {
    let manager: Arc<dyn TransactionManager> = Arc::new(FakeManager { datasource: "pg" });
    move |datasource| (datasource == "pg").then(|| manager.clone())
}

#[test]
fn transacted_block_fails_at_load_dark() {
    let routes = parse_yaml_to_declarative(TRANSACTED_YAML).expect("fixture YAML must parse");
    assert_eq!(routes.len(), 1, "fixture must author exactly one route");
    let route = routes.into_iter().next().expect("one route");

    match compile_declarative_route(route) {
        Ok(_) => panic!("a transacted block must not load while dark"),
        Err(CamelError::Transaction(TransactionError::ManagerMissing { datasource })) => {
            assert_eq!(
                datasource, "pg",
                "the typed error must name the block datasource"
            );
        }
        Err(other) => panic!("expected CamelError::Transaction(ManagerMissing), got {other:?}"),
    }
}

#[test]
fn authoring_equivalence_under_fake_manager() {
    // Production never registers a manager in slice 1, so the dark test above
    // is the real loading contract. This fake is registered only in the test's
    // resolver closure: it is test-only evidence for family req 1's THEN clause
    // (the three authorings compile to the same model). End-to-end runtime
    // loading completes in slice 2, when a manager is actually registered.
    let resolver = test_resolver();

    let routes = parse_yaml_to_declarative(TRANSACTED_YAML).expect("fixture YAML must parse");
    assert_eq!(routes.len(), 1, "fixture must author exactly one route");
    let yaml_def = compile_declarative_route_with(routes[0].clone(), &resolver)
        .expect("YAML leg must compile under the fake manager");

    let (spec, loss) =
        compile_declarative_route_to_canonical(routes[0].clone(), false).expect("canonical emit");
    assert!(loss.is_none(), "a transacted block drops nothing");
    let json = serde_json::to_string(&serde_json::json!({ "routes": [spec] }))
        .expect("canonical spec must serialize");
    let canonical_routes = parse_canonical_json(&json).expect("canonical JSON must compile");
    assert_eq!(
        canonical_routes.len(),
        1,
        "canonical parse-back must yield exactly one route"
    );
    let canonical_def = &canonical_routes[0];

    let builder_def = RouteBuilder::from("direct:start")
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

    // `BuilderStep` has no `PartialEq`; debug-format equality of the compiled
    // `steps()` accessor is the pinned equivalence mechanism.
    let expected = format!("{:?}", yaml_def.steps());
    assert_eq!(
        format!("{:?}", canonical_def.steps()),
        expected,
        "canonical round-trip must lower to the same steps as YAML"
    );
    assert_eq!(
        format!("{:?}", builder_def.steps()),
        expected,
        "builder authoring must lower to the same steps as YAML"
    );
}
