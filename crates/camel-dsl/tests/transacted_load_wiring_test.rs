//! Production load-path wiring for `transacted:` blocks (sqltrans, slice 1).
//!
//! The YAML and JSON loaders that bundle boot and `parse_yaml`/`parse_json`
//! funnel through both delegate to
//! `compile_declarative_route_with_stream_cache_threshold`, whose
//! always-absent resolver is the dark contract: no production registration
//! exists, so a clean block must fail at load with the typed
//! `TransactionError::ManagerMissing` naming its datasource — after the
//! structural checks pass and before any lowering runs.

use camel_api::CamelError;
use camel_api::transaction::TransactionError;
use camel_dsl::{
    SecurityCompileContext, parse_json_with_threshold_and_security,
    parse_yaml_with_threshold_and_security,
};

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

const TRANSACTED_JSON: &str = r#"
{
  "routes": [
    {
      "id": "tx-route",
      "from": "direct:start",
      "steps": [
        {
          "transacted": {
            "datasource": "pg",
            "steps": [
              { "to": "sql:select 1?datasource=pg" },
              { "to": "sql:insert into t values (1)?datasource=pg" }
            ]
          }
        }
      ]
    }
  ]
}
"#;

fn assert_manager_missing(result: Result<Vec<camel_core::route::RouteDefinition>, CamelError>) {
    match result {
        Err(CamelError::Transaction(TransactionError::ManagerMissing { datasource })) => {
            assert_eq!(datasource, "pg");
        }
        Err(other) => panic!("expected typed ManagerMissing('pg'), got {other:?}"),
        Ok(_) => panic!("expected typed ManagerMissing('pg'), got Ok routes"),
    }
}

#[test]
fn yaml_production_loader_rejects_block() {
    // The loader bundle boot calls (`camel_dsl::parse_yaml_with_threshold_and_security`).
    let result = parse_yaml_with_threshold_and_security(
        TRANSACTED_YAML,
        1024,
        SecurityCompileContext::default(),
    );
    assert_manager_missing(result);
}

#[test]
fn json_production_loader_rejects_block() {
    // Same choke point through the JSON entry (`json.rs` calls the threshold
    // compile fn whose body now delegates to the resolver seam).
    let result = parse_json_with_threshold_and_security(
        TRANSACTED_JSON,
        1024,
        SecurityCompileContext::default(),
    );
    assert_manager_missing(result);
}
