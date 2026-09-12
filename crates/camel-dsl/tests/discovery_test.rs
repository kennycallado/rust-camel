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
fn embedded_text_resolves_runtime_environment() {
    use camel_dsl::discovery::{EmbeddedDocumentKind, discover_embedded_text};

    // Variable names the process environment is guaranteed not to carry:
    // only the injected runtime lookup can resolve the first one, and the
    // second simulates a stale compile-time value that must never appear.
    let text = "routes:\n  - id: embedded-env\n    from: \"${env:RC_EMBEDDED_RT_HOST}:9090\"\n    steps: []\n";
    let routes = discover_embedded_text(
        text,
        "routes/app.yaml",
        EmbeddedDocumentKind::Route,
        &|name| match name {
            "RC_EMBEDDED_RT_HOST" => Some("runtime-host".to_string()),
            "RC_EMBEDDED_COMPILE_HOST" => Some("compile-host".to_string()),
            _ => None,
        },
    )
    .expect("embedded text must resolve through the injected runtime environment");
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].route_id(), "embedded-env");
    assert_eq!(
        routes[0].from_uri(),
        "runtime-host:9090",
        "runtime value must appear in the parsed route"
    );
    assert!(
        !routes[0].from_uri().contains("compile-host"),
        "compile-time value must not appear in the parsed route"
    );
}

#[test]
fn embedded_text_preserves_typed_probe_and_source_identity() {
    use camel_dsl::discovery::{DiscoveryError, EmbeddedDocumentKind, discover_embedded_text};

    // Typed int placeholder: the strict-typed throttle field rejects the
    // substituted string "5", so success with a numeric 5 proves the
    // provenance-fed typed probe ran (env-int-placeholder-typing).
    let text = "routes:\n  - id: embedded-int\n    from: \"direct:start\"\n    steps:\n      - throttle:\n          max_requests: ${env:RC_EMBEDDED_INT:-2}\n          period_secs: 1\n";
    let routes = discover_embedded_text(
        text,
        "routes/int.yaml",
        EmbeddedDocumentKind::Route,
        &|name| (name == "RC_EMBEDDED_INT").then(|| "5".to_string()),
    )
    .expect("typed int placeholder must coerce through the probe");
    match &routes[0].steps()[0] {
        camel_core::route::BuilderStep::Throttle { config, .. } => {
            assert_eq!(config.max_requests, 5);
        }
        other => panic!("expected throttle step, got: {other:?}"),
    }
    assert!(
        routes[0].source_hash().is_some(),
        "embedded routes must carry the source hash of the raw text"
    );

    // Diagnostic identity: an unresolved variable names the virtual
    // `compiled://<source_name>` identity, not a filesystem path.
    let failing = "routes:\n  - id: embedded-missing\n    from: \"${env:RC_EMBEDDED_MISSING}:9090\"\n    steps: []\n";
    let err = match discover_embedded_text(
        failing,
        "routes/int.yaml",
        EmbeddedDocumentKind::Route,
        &|_| None,
    ) {
        Ok(routes) => panic!(
            "unresolved runtime variable must fail discovery, got {} route(s)",
            routes.len()
        ),
        Err(err) => err,
    };
    match err {
        DiscoveryError::Env { path, var_name } => {
            assert_eq!(path, "compiled://routes/int.yaml");
            assert_eq!(var_name, "RC_EMBEDDED_MISSING");
        }
        other => panic!("expected DiscoveryError::Env, got {other:?}"),
    }
}

#[test]
fn embedded_text_does_not_touch_filesystem() {
    use camel_dsl::discovery::{EmbeddedDocumentKind, discover_embedded_text};

    let text = "routes:\n  - id: memory-only\n    from: \"direct:start\"\n    steps: []\n";

    // Relative virtual source name: no such file exists under the test cwd,
    // so a successful parse proves the text was not read from disk.
    let routes = discover_embedded_text(
        text,
        "payload/memory-only.yaml",
        EmbeddedDocumentKind::Route,
        &|_| None,
    )
    .expect("embedded text must parse without any source file");
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].route_id(), "memory-only");

    // Absolute virtual source name inside a fresh directory: the directory
    // must stay empty — no extraction, no temporary writes.
    let dir = tempdir().unwrap();
    let source = dir.path().join("nested/app.yaml");
    let routes = discover_embedded_text(
        text,
        source.to_str().unwrap(),
        EmbeddedDocumentKind::Route,
        &|_| None,
    )
    .expect("embedded text must parse without any source file");
    assert_eq!(routes[0].route_id(), "memory-only");
    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        entries.is_empty(),
        "embedded discovery must not extract or write files, found {entries:?}"
    );
}

#[test]
fn embedded_text_rejects_extensionless_and_unsupported_source_names() {
    use camel_dsl::discovery::{DiscoveryError, EmbeddedDocumentKind, discover_embedded_text};

    let text = "routes:\n  - id: never-parsed\n    from: \"direct:start\"\n    steps: []\n";

    // Extensionless virtual source name: must fail with UnsupportedExtension
    // (empty extension) naming the compiled:// identity — never reach the
    // shared parse pass's unreachable arm.
    let err =
        match discover_embedded_text(text, "payload/route", EmbeddedDocumentKind::Route, &|_| {
            None
        }) {
            Ok(routes) => panic!(
                "extensionless source name must fail discovery, got {} route(s)",
                routes.len()
            ),
            Err(err) => err,
        };
    match err {
        DiscoveryError::UnsupportedExtension { path, extension } => {
            assert_eq!(path, "compiled://payload/route");
            assert!(extension.is_empty(), "extension was: {extension:?}");
        }
        other => panic!("expected UnsupportedExtension, got {other:?}"),
    }

    // Unsupported extension: same fail-closed rule, extension named.
    let err = match discover_embedded_text(
        text,
        "payload/route.xml",
        EmbeddedDocumentKind::Route,
        &|_| None,
    ) {
        Ok(routes) => panic!(
            "unsupported source extension must fail discovery, got {} route(s)",
            routes.len()
        ),
        Err(err) => err,
    };
    match err {
        DiscoveryError::UnsupportedExtension { path, extension } => {
            assert_eq!(path, "compiled://payload/route.xml");
            assert_eq!(extension, "xml");
        }
        other => panic!("expected UnsupportedExtension, got {other:?}"),
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
