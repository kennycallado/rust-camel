use camel_dsl::model::{DeclarativeStep, LoadBalanceStrategyDef, ThrottleStrategyDef};
use camel_dsl::parse_yaml_to_declarative;

mod yaml_throttle_test {
    use super::*;

    #[test]
    fn throttle_basic() {
        let yaml = r#"
routes:
  - id: "throttle-basic"
    from: "direct:start"
    steps:
      - throttle:
          max_requests: 10
          period_secs: 1
          steps:
            - to: "mock:result"
"#;

        let routes = parse_yaml_to_declarative(yaml).expect("YAML parse should succeed");
        let route = &routes[0];

        match &route.steps[0] {
            DeclarativeStep::Throttle(step) => {
                assert_eq!(step.max_requests, 10);
                assert_eq!(step.period_ms, 1000);
                assert_eq!(step.strategy, ThrottleStrategyDef::Delay);
                assert_eq!(step.steps.len(), 1);
            }
            other => panic!("expected Throttle step, got {other:?}"),
        }
    }

    #[test]
    fn throttle_with_reject_strategy() {
        let yaml = r#"
routes:
  - id: "throttle-reject"
    from: "direct:start"
    steps:
      - throttle:
          max_requests: 3
          period_secs: 2
          strategy: "reject"
          steps:
            - to: "mock:result"
"#;

        let routes = parse_yaml_to_declarative(yaml).expect("YAML parse should succeed");
        let route = &routes[0];

        match &route.steps[0] {
            DeclarativeStep::Throttle(step) => {
                assert_eq!(step.strategy, ThrottleStrategyDef::Reject);
                assert_eq!(step.period_ms, 2000);
            }
            other => panic!("expected Throttle step, got {other:?}"),
        }
    }

    #[test]
    fn throttle_with_drop_strategy() {
        let yaml = r#"
routes:
  - id: "throttle-drop"
    from: "direct:start"
    steps:
      - throttle:
          max_requests: 5
          period_secs: 1
          strategy: "drop"
          steps:
            - to: "mock:result"
"#;

        let routes = parse_yaml_to_declarative(yaml).expect("YAML parse should succeed");
        let route = &routes[0];

        match &route.steps[0] {
            DeclarativeStep::Throttle(step) => {
                assert_eq!(step.strategy, ThrottleStrategyDef::Drop);
            }
            other => panic!("expected Throttle step, got {other:?}"),
        }
    }

    #[test]
    fn throttle_unknown_strategy_returns_error() {
        let yaml = r#"
routes:
  - id: "throttle-invalid"
    from: "direct:start"
    steps:
      - throttle:
          max_requests: 1
          strategy: "invalid"
          steps:
            - to: "mock:result"
"#;

        let result = parse_yaml_to_declarative(yaml);
        assert!(result.is_err());
    }
}

mod yaml_load_balance_test {
    use super::*;

    #[test]
    fn load_balance_round_robin() {
        let yaml = r#"
routes:
  - id: "lb-rr"
    from: "direct:start"
    steps:
      - load_balance:
          strategy: "round_robin"
          steps:
            - to: "mock:a"
            - to: "mock:b"
"#;

        let routes = parse_yaml_to_declarative(yaml).expect("YAML parse should succeed");
        let route = &routes[0];

        match &route.steps[0] {
            DeclarativeStep::LoadBalance(step) => {
                assert_eq!(step.strategy, LoadBalanceStrategyDef::RoundRobin);
                assert!(!step.parallel);
                assert_eq!(step.steps.len(), 2);
            }
            other => panic!("expected LoadBalance step, got {other:?}"),
        }
    }

    #[test]
    fn load_balance_parallel_random() {
        let yaml = r#"
routes:
  - id: "lb-random"
    from: "direct:start"
    steps:
      - load_balance:
          strategy: "random"
          parallel: true
          steps:
            - to: "mock:a"
            - to: "mock:b"
"#;

        let routes = parse_yaml_to_declarative(yaml).expect("YAML parse should succeed");
        let route = &routes[0];

        match &route.steps[0] {
            DeclarativeStep::LoadBalance(step) => {
                assert_eq!(step.strategy, LoadBalanceStrategyDef::Random);
                assert!(step.parallel);
                assert_eq!(step.steps.len(), 2);
            }
            other => panic!("expected LoadBalance step, got {other:?}"),
        }
    }

    #[test]
    fn load_balance_unknown_strategy_returns_error() {
        let yaml = r#"
routes:
  - id: "lb-invalid"
    from: "direct:start"
    steps:
      - load_balance:
          strategy: "weighted"
          steps:
            - to: "mock:a"
"#;

        let result = parse_yaml_to_declarative(yaml);
        assert!(result.is_err());
    }
}

mod yaml_dynamic_router_test {
    use super::*;

    #[test]
    fn dynamic_router_simple() {
        let yaml = r#"
routes:
  - id: "dynamic-router"
    from: "direct:start"
    steps:
      - dynamic_router:
          simple: "${header.dest}"
"#;

        let routes = parse_yaml_to_declarative(yaml).expect("YAML parse should succeed");
        let route = &routes[0];

        match &route.steps[0] {
            DeclarativeStep::DynamicRouter(step) => {
                assert_eq!(step.expression.language, "simple");
                assert_eq!(step.expression.source, "${header.dest}");
                assert_eq!(step.uri_delimiter, ",");
                assert_eq!(step.cache_size, 1000);
                assert!(!step.ignore_invalid_endpoints);
                assert_eq!(step.max_iterations, 1000);
            }
            other => panic!("expected DynamicRouter step, got {other:?}"),
        }
    }

    #[test]
    fn dynamic_router_no_expression_returns_error() {
        let yaml = r#"
routes:
  - id: "dynamic-router-invalid"
    from: "direct:start"
    steps:
      - dynamic_router:
          uri_delimiter: ";"
"#;

        let result = parse_yaml_to_declarative(yaml);
        assert!(result.is_err());
    }
}

mod yaml_routing_slip_test {
    use super::*;

    #[test]
    fn routing_slip_simple() {
        let yaml = r#"
routes:
  - id: "routing-slip"
    from: "direct:start"
    steps:
      - routing_slip:
          simple: "${header.slip}"
"#;

        let routes = parse_yaml_to_declarative(yaml).expect("YAML parse should succeed");
        let route = &routes[0];

        match &route.steps[0] {
            DeclarativeStep::RoutingSlip(step) => {
                assert_eq!(step.expression.language, "simple");
                assert_eq!(step.expression.source, "${header.slip}");
                assert_eq!(step.uri_delimiter, ",");
                assert_eq!(step.cache_size, 1000);
                assert!(!step.ignore_invalid_endpoints);
            }
            other => panic!("expected RoutingSlip step, got {other:?}"),
        }
    }
}

mod yaml_bean_test {
    use super::*;

    #[test]
    fn bean_basic() {
        let yaml = r#"
routes:
  - id: "bean-basic"
    from: "direct:start"
    steps:
      - bean:
          name: "myBean"
          method: "process"
"#;

        let routes = parse_yaml_to_declarative(yaml).expect("YAML parse should succeed");
        let route = &routes[0];

        match &route.steps[0] {
            DeclarativeStep::Bean(step) => {
                assert_eq!(step.name, "myBean");
                assert_eq!(step.method, "process");
            }
            other => panic!("expected Bean step, got {other:?}"),
        }
    }
}
