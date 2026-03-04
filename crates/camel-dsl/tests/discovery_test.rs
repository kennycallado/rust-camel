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
