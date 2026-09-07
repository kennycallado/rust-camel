use camel_dsl::discovery::discover_routes;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_discover_single_route() {
    let dir = tempdir().unwrap();
    let routes_dir = dir.path().join("routes");
    fs::create_dir(&routes_dir).unwrap();

    let route_content = r#"
routes:
  - id: "test-route"
    from: "timer:tick"
    steps:
      - to: "log:info"
"#;
    fs::write(routes_dir.join("test.yaml"), route_content).unwrap();

    let pattern = dir
        .path()
        .join("routes/*.yaml")
        .to_str()
        .unwrap()
        .to_string();
    let routes = discover_routes(&[pattern]).expect("Failed to discover routes");

    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].route_id(), "test-route");
}

#[test]
fn test_discover_multiple_routes_with_glob() {
    let dir = tempdir().unwrap();
    let routes_dir = dir.path().join("routes");
    let sub_dir = routes_dir.join("sub");
    fs::create_dir_all(&sub_dir).unwrap();

    let route1 = r#"
routes:
  - id: "route-1"
    from: "timer:tick"
    steps: []
"#;
    let route2 = r#"
routes:
  - id: "route-2"
    from: "direct:start"
    steps: []
"#;

    fs::write(routes_dir.join("route1.yaml"), route1).unwrap();
    fs::write(sub_dir.join("route2.yaml"), route2).unwrap();

    let pattern = dir
        .path()
        .join("routes/**/*.yaml")
        .to_str()
        .unwrap()
        .to_string();
    let routes = discover_routes(&[pattern]).expect("Failed to discover routes");

    assert_eq!(routes.len(), 2);
}

#[test]
fn test_discover_multiple_patterns() {
    let dir = tempdir().unwrap();
    let routes_dir = dir.path().join("routes");
    let other_dir = dir.path().join("other");
    fs::create_dir_all(&routes_dir).unwrap();
    fs::create_dir_all(&other_dir).unwrap();

    let route1 = r#"
routes:
  - id: "route-1"
    from: "timer:tick"
    steps: []
"#;
    let route2 = r#"
routes:
  - id: "route-2"
    from: "direct:start"
    steps: []
"#;

    fs::write(routes_dir.join("route1.yaml"), route1).unwrap();
    fs::write(other_dir.join("route2.yaml"), route2).unwrap();

    let pattern1 = dir
        .path()
        .join("routes/*.yaml")
        .to_str()
        .unwrap()
        .to_string();
    let pattern2 = dir
        .path()
        .join("other/*.yaml")
        .to_str()
        .unwrap()
        .to_string();
    let routes = discover_routes(&[pattern1, pattern2]).expect("Failed to discover routes");

    assert_eq!(routes.len(), 2);
}

#[test]
fn test_discover_empty_pattern() {
    let routes = discover_routes(&[]).expect("Failed with empty patterns");
    assert!(routes.is_empty());
}

#[test]
fn test_discover_no_matching_files() {
    let dir = tempdir().unwrap();
    let pattern = dir
        .path()
        .join("nonexistent/*.yaml")
        .to_str()
        .unwrap()
        .to_string();
    let routes = discover_routes(&[pattern]).expect("Failed with no matching files");
    assert!(routes.is_empty());
}

#[test]
fn unparseable_input_falls_back_to_legacy_env_error() {
    use camel_dsl::discovery::DiscoveryError;
    use camel_dsl::discovery::discover_routes_with_threshold_security_and_env;
    use camel_dsl::model::SecurityCompileContext;

    let dir = tempdir().unwrap();
    let routes_dir = dir.path().join("routes");
    fs::create_dir(&routes_dir).unwrap();
    // Unterminated quote: fails the YAML-shim parse, so discovery falls
    // back to legacy whole-text interpolation, which fails naming the var.
    fs::write(
        routes_dir.join("broken.yaml"),
        "broken: \"unterminated ${env:MISSING}\n",
    )
    .unwrap();
    let pattern = dir
        .path()
        .join("routes/*.yaml")
        .to_str()
        .unwrap()
        .to_string();
    let result = discover_routes_with_threshold_security_and_env(
        &[pattern],
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
        SecurityCompileContext::default(),
        &|_| None,
    );
    let err = match result {
        Ok(routes) => panic!(
            "unparseable file with unresolved var must fail discovery, got {} route(s)",
            routes.len()
        ),
        Err(err) => err,
    };
    match err {
        DiscoveryError::Env { var_name, .. } => assert_eq!(var_name, "MISSING"),
        other => panic!("expected DiscoveryError::Env, got {other:?}"),
    }
}

#[test]
fn comment_placeholder_file_loads_via_discovery() {
    use camel_dsl::discovery::discover_routes_with_threshold_security_and_env;
    use camel_dsl::model::SecurityCompileContext;

    let dir = tempdir().unwrap();
    let routes_dir = dir.path().join("routes");
    fs::create_dir(&routes_dir).unwrap();
    // A comment referencing an unset var must not fail the file; the valid
    // route body underneath it must still load (file-on-disk → interpolate
    // → parse path, pinned after the parse-tree walk landed).
    let route_content = r#"# TODO re-enable ${env:MISSING}
routes:
  - id: "comment-route"
    from: "direct:start"
    steps: []
"#;
    fs::write(routes_dir.join("route.yaml"), route_content).unwrap();
    let pattern = dir
        .path()
        .join("routes/*.yaml")
        .to_str()
        .unwrap()
        .to_string();
    let routes = discover_routes_with_threshold_security_and_env(
        &[pattern],
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
        SecurityCompileContext::default(),
        &|_| None, // MISSING deliberately not defined
    )
    .expect("comment placeholder must not fail file discovery");
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].route_id(), "comment-route");
}
