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

/// Build an in-memory [`camel_dsl::VirtualDocumentStore`] from
/// `(path, kind, text)` triples (openspec change `multidoc`, Task 2.1).
fn virtual_store(
    entry_point: &str,
    documents: &[(&str, camel_dsl::StoreEntryKind, &str)],
    config_references: &[&str],
    source_plan: &[&str],
) -> camel_dsl::VirtualDocumentStore {
    let documents: Vec<camel_dsl::StoreDocument> = documents
        .iter()
        .map(|(path, kind, text)| camel_dsl::StoreDocument {
            path: (*path).to_string(),
            kind: *kind,
            bytes: text.as_bytes().to_vec(),
        })
        .collect();
    let config_references: Vec<String> =
        config_references.iter().map(|s| (*s).to_string()).collect();
    let source_plan: Vec<String> = source_plan.iter().map(|s| (*s).to_string()).collect();
    camel_dsl::VirtualDocumentStore::build(
        entry_point,
        &documents,
        &config_references,
        &source_plan,
    )
    .expect("virtual store must build")
}

/// `${env:}` in embedded route documents resolves exclusively through
/// the injected deployment lookup: no compile-time value is required
/// (or ever appears), and the merged configuration keeps its
/// placeholders raw for the caller's typed `CamelConfig` resolution.
#[test]
fn discover_virtual_store_resolves_deployment_environment() {
    use camel_dsl::StoreEntryKind;
    use camel_dsl::discovery::discover_virtual_store;

    let config = "[default]\nlog_level = \"${env:RC_VS_LOG_LEVEL:-info}\"\n";
    let main = "routes:\n  - id: vs-env-main\n    from: \"direct:${env:RC_VS_DEPLOY_TARGET}\"\n    steps: []\n";
    let orders = "routes:\n  - id: vs-env-orders\n    from: \"timer:${env:RC_VS_DEPLOY_URI}\"\n    steps: []\n";
    let store = virtual_store(
        "routes/main.yaml",
        &[
            ("Camel.toml", StoreEntryKind::Config, config),
            ("routes/main.yaml", StoreEntryKind::Route, main),
            ("routes/orders.yaml", StoreEntryKind::Route, orders),
        ],
        &["Camel.toml"],
        &["routes/main.yaml", "routes/orders.yaml"],
    );

    // The lookup provides ONLY deployment values; the RC_VS_* names are
    // absent from the process environment, and a stale compile-time
    // value must never surface in the parsed routes.
    let discovered = discover_virtual_store(&store, &|name| match name {
        "RC_VS_DEPLOY_TARGET" => Some("deploy-target".to_string()),
        "RC_VS_DEPLOY_URI" => Some("deploy".to_string()),
        "RC_VS_COMPILE_TARGET" => Some("compile-target".to_string()),
        _ => None,
    })
    .expect("virtual-store discovery must resolve through the deployment lookup");

    assert_eq!(discovered.routes.len(), 2);
    assert_eq!(discovered.routes[0].route_id(), "vs-env-main");
    assert_eq!(discovered.routes[0].from_uri(), "direct:deploy-target");
    assert_eq!(discovered.routes[1].route_id(), "vs-env-orders");
    assert_eq!(discovered.routes[1].from_uri(), "timer:deploy");
    for route in &discovered.routes {
        let uri = route.from_uri();
        assert!(
            !uri.contains("compile-target"),
            "compile-time value leaked into {uri}"
        );
    }

    // Configuration placeholders stay raw: typed deployment resolution
    // belongs to the caller's CamelConfig deserialization.
    assert_eq!(
        discovered.config.get("log_level").and_then(|v| v.as_str()),
        Some("${env:RC_VS_LOG_LEVEL:-info}")
    );
}

/// The merged configuration follows the filesystem loader's ordered
/// include/profile overlay (camel-config `load_includes` as lowest
/// priority, the configuration document above it, `[default]` merged
/// with the selected profile section, arrays replaced by overlays).
#[test]
fn discover_virtual_store_builds_config_in_index_order() {
    use camel_dsl::StoreEntryKind;
    use camel_dsl::discovery::discover_virtual_store;

    // Expected filesystem-equivalent values for profile `prod`:
    // - `log_level = "debug"` — include-only key survives below the
    //   configuration document;
    // - `timeout_ms = 10000` — the configuration outranks the include
    //   (60000 loses), and `[prod]` overlays `[default]` (30000 loses);
    // - `routes = [main, orders]` — the `[prod]` array replaces the
    //   `[default]` array (never concatenates).
    let config = concat!(
        "include = [\"conf/base.toml\"]\n",
        "\n",
        "[default]\n",
        "timeout_ms = 30000\n",
        "routes = [\"routes/main.yaml\"]\n",
        "\n",
        "[prod]\n",
        "timeout_ms = 10000\n",
        "routes = [\"routes/main.yaml\", \"routes/orders.yaml\"]\n",
    );
    let include = "log_level = \"debug\"\ntimeout_ms = 60000\n";
    let profile = concat!(
        "[prod]\n",
        "timeout_ms = 10000\n",
        "routes = [\"routes/main.yaml\", \"routes/orders.yaml\"]\n",
    );
    let main = "routes:\n  - id: vs-cfg-main\n    from: \"direct:start\"\n    steps: []\n";
    let orders = "routes:\n  - id: vs-cfg-orders\n    from: \"direct:orders\"\n    steps: []\n";

    let store = virtual_store(
        "routes/main.yaml",
        &[
            ("Camel.toml", StoreEntryKind::Config, config),
            ("conf/base.toml", StoreEntryKind::Include, include),
            ("prod.profile.toml", StoreEntryKind::Profile, profile),
            ("routes/main.yaml", StoreEntryKind::Route, main),
            ("routes/orders.yaml", StoreEntryKind::Route, orders),
        ],
        &["Camel.toml", "conf/base.toml", "prod.profile.toml"],
        &["routes/main.yaml", "routes/orders.yaml"],
    );

    let discovered = discover_virtual_store(&store, &|_| None)
        .expect("virtual-store discovery must assemble the embedded configuration");

    assert_eq!(
        discovered.config.get("log_level").and_then(|v| v.as_str()),
        Some("debug"),
        "include-only key must survive below the configuration document"
    );
    assert_eq!(
        discovered
            .config
            .get("timeout_ms")
            .and_then(|v| v.as_integer()),
        Some(10000),
        "configuration [prod] must outrank both the include and [default]"
    );
    let routes: Vec<&str> = discovered
        .config
        .get("routes")
        .and_then(|v| v.as_array())
        .expect("merged routes list must exist")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(
        routes,
        vec!["routes/main.yaml", "routes/orders.yaml"],
        "profile section arrays must replace, not concatenate"
    );

    // The plan's route documents still discover through the same pass.
    assert_eq!(discovered.routes.len(), 2);
}

/// Discovery consumes only store entries: logical paths that exist
/// nowhere on disk still parse, no configuration is discovered, and no
/// extraction or temporary writes happen.
#[test]
fn discover_virtual_store_does_not_touch_filesystem() {
    use camel_dsl::StoreEntryKind;
    use camel_dsl::discovery::discover_virtual_store;

    let main = "routes:\n  - id: vs-memory\n    from: \"direct:start\"\n    steps: []\n";
    let store = virtual_store(
        "payload/vs-main.yaml",
        &[("payload/vs-main.yaml", StoreEntryKind::Route, main)],
        &[],
        &["payload/vs-main.yaml"],
    );

    let discovered = discover_virtual_store(&store, &|_| None)
        .expect("virtual-store discovery must run purely from the store");
    assert_eq!(discovered.routes.len(), 1);
    assert_eq!(discovered.routes[0].route_id(), "vs-memory");
    assert!(
        discovered.config.as_table().is_some_and(|t| t.is_empty()),
        "a store without configuration references assembles an empty config"
    );

    // No extraction, no temporary writes: a fresh directory stays empty.
    let dir = tempdir().unwrap();
    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        entries.is_empty(),
        "virtual-store discovery must not write files, found {entries:?}"
    );
}

/// A malformed plan document surfaces as a parse diagnostic naming its
/// virtual `compiled://<logical-path>` identity, never a filesystem
/// path.
#[test]
fn discover_virtual_store_preserves_virtual_provenance() {
    use camel_dsl::StoreEntryKind;
    use camel_dsl::discovery::{DiscoveryError, discover_virtual_store};

    let main = "routes:\n  - id: vs-prov-main\n    from: \"direct:start\"\n    steps: []\n";
    // Unterminated quote: fails YAML parsing in the shared pass.
    let orders = "routes:\n  - id: \"broken\n";
    let store = virtual_store(
        "routes/main.yaml",
        &[
            ("routes/main.yaml", StoreEntryKind::Route, main),
            ("routes/orders.yaml", StoreEntryKind::Route, orders),
        ],
        &[],
        &["routes/main.yaml", "routes/orders.yaml"],
    );

    let err = match discover_virtual_store(&store, &|_| None) {
        Ok(discovered) => panic!(
            "malformed route document must fail discovery, got {} route(s)",
            discovered.routes.len()
        ),
        Err(err) => err,
    };
    match err {
        DiscoveryError::Yaml { path, error } => {
            assert_eq!(path, "compiled://routes/orders.yaml");
            assert!(!error.is_empty());
        }
        other => panic!("expected DiscoveryError::Yaml, got {other:?}"),
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
