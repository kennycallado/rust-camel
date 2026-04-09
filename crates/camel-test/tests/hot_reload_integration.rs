//! Integration test for file-watcher hot-reload.
//!
//! Writes real YAML files to a tempdir, starts a watcher task, updates the
//! file, and verifies that the pipeline is swapped without stopping the route.

use camel_component_mock::MockComponent;
use camel_component_timer::TimerComponent;
use camel_core::CamelContext;
use camel_core::reload_watcher::{resolve_watch_dirs, watch_and_reload};
use std::time::Duration;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

/// Helper: write a timer→mock route YAML file.
fn write_route_yaml(path: &std::path::Path, route_id: &str, period: u64, mock_name: &str) {
    let content = format!(
        r#"
routes:
  - id: "{route_id}"
    from: "timer:tick?period={period}&repeatCount=1000"
    steps:
      - to: "mock:{mock_name}"
"#
    );
    std::fs::write(path, content).unwrap();
}

/// Helper: write a route YAML with explicit from URI and auto_startup flag.
fn write_route_yaml_custom(
    path: &std::path::Path,
    route_id: &str,
    from_uri: &str,
    mock_name: &str,
    auto_startup: bool,
) {
    let content = format!(
        r#"
routes:
  - id: "{route_id}"
    from: "{from_uri}"
    auto_startup: {auto_startup}
    steps:
      - to: "mock:{mock_name}"
"#
    );
    std::fs::write(path, content).unwrap();
}

#[tokio::test]
async fn test_resolve_watch_dirs_from_glob_patterns() {
    let dir = tempdir().unwrap();
    let pattern = format!("{}/*.yaml", dir.path().display());
    let dirs = resolve_watch_dirs(&[pattern]);
    assert_eq!(dirs.len(), 1);
    assert_eq!(dirs[0], dir.path());
}

#[tokio::test]
async fn test_watcher_swaps_pipeline_on_file_change() {
    let dir = tempdir().unwrap();
    let route_file = dir.path().join("route.yaml");

    // Write initial route: timer → mock:v1
    write_route_yaml(&route_file, "watcher-test", 30, "v1");

    let mock = MockComponent::new();
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    // Load the initial route definitions
    let pattern = format!("{}/*.yaml", dir.path().display());
    let defs = camel_dsl::discover_routes(std::slice::from_ref(&pattern)).unwrap();
    for def in defs {
        ctx.add_route_definition(def).await.unwrap();
    }
    ctx.start().await.unwrap();

    // Let v1 receive some exchanges
    tokio::time::sleep(Duration::from_millis(200)).await;

    let v1_before = if let Some(ep) = mock.get_endpoint("v1") {
        ep.get_received_exchanges().await.len()
    } else {
        0
    };
    assert!(v1_before > 0, "v1 should receive exchanges before swap");

    // Start the file watcher with a cancellation token for graceful shutdown
    let ctrl = ctx.runtime_execution_handle();
    let patterns_clone = vec![pattern.clone()];
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();
    let watcher_handle = tokio::spawn(async move {
        let dirs = resolve_watch_dirs(&patterns_clone);
        watch_and_reload(
            dirs,
            ctrl,
            move || {
                camel_dsl::discover_routes(&patterns_clone)
                    .map_err(|e| camel_api::CamelError::RouteError(e.to_string()))
            },
            Some(shutdown_clone),
            std::time::Duration::from_secs(10),
            std::time::Duration::from_millis(50),
        )
        .await
        .ok();
    });

    // Give the watcher time to register before writing the file
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Update route: timer → mock:v2
    write_route_yaml(&route_file, "watcher-test", 30, "v2");

    // Wait for debounce (300ms) + processing time
    tokio::time::sleep(Duration::from_millis(700)).await;

    let v2_count = if let Some(ep) = mock.get_endpoint("v2") {
        ep.get_received_exchanges().await.len()
    } else {
        0
    };
    assert!(
        v2_count > 0,
        "v2 should receive exchanges after pipeline swap"
    );

    // Graceful shutdown: cancel the token instead of aborting
    shutdown.cancel();
    watcher_handle.await.ok();
    ctx.stop().await.unwrap();
}

#[tokio::test]
async fn test_watcher_removes_route_on_file_deletion() {
    let dir = tempdir().unwrap();
    let route_file = dir.path().join("route.yaml");

    // Write initial route: timer → mock:del-test
    write_route_yaml(&route_file, "deletion-test", 30, "del-test");

    let mock = MockComponent::new();
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let pattern = format!("{}/*.yaml", dir.path().display());
    let defs = camel_dsl::discover_routes(std::slice::from_ref(&pattern)).unwrap();
    for def in defs {
        ctx.add_route_definition(def).await.unwrap();
    }
    ctx.start().await.unwrap();

    // Let the route run briefly
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify route is running
    let del_count_before = if let Some(ep) = mock.get_endpoint("del-test") {
        ep.get_received_exchanges().await.len()
    } else {
        0
    };
    assert!(
        del_count_before > 0,
        "route should be running before deletion"
    );

    // Start the file watcher
    let ctrl = ctx.runtime_execution_handle();
    let patterns_clone = vec![pattern.clone()];
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();
    let watcher_handle = tokio::spawn(async move {
        let dirs = resolve_watch_dirs(&patterns_clone);
        watch_and_reload(
            dirs,
            ctrl,
            move || {
                camel_dsl::discover_routes(&patterns_clone)
                    .map_err(|e| camel_api::CamelError::RouteError(e.to_string()))
            },
            Some(shutdown_clone),
            std::time::Duration::from_secs(10),
            std::time::Duration::from_millis(50),
        )
        .await
        .ok();
    });

    // Give the watcher time to register
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Delete the route file — this should trigger a Remove action
    std::fs::remove_file(&route_file).unwrap();

    // Wait for debounce (300ms) + processing time
    tokio::time::sleep(Duration::from_millis(700)).await;

    // Record exchanges before and after deletion window
    let del_count_after = if let Some(ep) = mock.get_endpoint("del-test") {
        ep.get_received_exchanges().await.len()
    } else {
        0
    };

    // After deletion the route should be stopped; wait a bit more and check
    // that no new exchanges arrive (count stays the same)
    tokio::time::sleep(Duration::from_millis(200)).await;
    let del_count_final = if let Some(ep) = mock.get_endpoint("del-test") {
        ep.get_received_exchanges().await.len()
    } else {
        0
    };

    assert_eq!(
        del_count_after, del_count_final,
        "route should be stopped after file deletion — no new exchanges expected"
    );

    shutdown.cancel();
    watcher_handle.await.ok();
    ctx.stop().await.unwrap();
}

#[tokio::test]
async fn test_watcher_restart_preserves_non_running_route_state() {
    let dir = tempdir().unwrap();
    let route_file = dir.path().join("route.yaml");

    // Initial route is not auto-started by startup policy.
    write_route_yaml_custom(
        &route_file,
        "stopped-restart-test",
        "timer:tick?period=30&repeatCount=1000",
        "stopped-v1",
        false,
    );

    let mock = MockComponent::new();
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let pattern = format!("{}/*.yaml", dir.path().display());
    let defs = camel_dsl::discover_routes(std::slice::from_ref(&pattern)).unwrap();
    for def in defs {
        ctx.add_route_definition(def).await.unwrap();
    }
    ctx.start().await.unwrap();

    assert_eq!(
        ctx.runtime_route_status("stopped-restart-test")
            .await
            .unwrap(),
        Some("Registered".to_string())
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    let v1_count = if let Some(ep) = mock.get_endpoint("stopped-v1") {
        ep.get_received_exchanges().await.len()
    } else {
        0
    };
    assert_eq!(v1_count, 0, "stopped route must not process exchanges");

    let ctrl = ctx.runtime_execution_handle();
    let patterns_clone = vec![pattern.clone()];
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();
    let watcher_handle = tokio::spawn(async move {
        let dirs = resolve_watch_dirs(&patterns_clone);
        watch_and_reload(
            dirs,
            ctrl,
            move || {
                camel_dsl::discover_routes(&patterns_clone)
                    .map_err(|e| camel_api::CamelError::RouteError(e.to_string()))
            },
            Some(shutdown_clone),
            std::time::Duration::from_secs(10),
            std::time::Duration::from_millis(50),
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Change from_uri to force Restart action; route must stay non-running.
    write_route_yaml_custom(
        &route_file,
        "stopped-restart-test",
        "timer:tock?period=30&repeatCount=1000",
        "stopped-v2",
        false,
    );

    tokio::time::sleep(Duration::from_millis(700)).await;

    assert_eq!(
        ctx.runtime_route_status("stopped-restart-test")
            .await
            .unwrap(),
        Some("Registered".to_string())
    );

    let v2_count = if let Some(ep) = mock.get_endpoint("stopped-v2") {
        ep.get_received_exchanges().await.len()
    } else {
        0
    };
    assert_eq!(
        v2_count, 0,
        "restart of a non-running route must preserve non-running state"
    );

    shutdown.cancel();
    watcher_handle.await.ok();
    ctx.stop().await.unwrap();
}

#[tokio::test]
async fn test_restart_with_from_uri_change_processes_exchanges_via_new_endpoint() {
    let dir = tempdir().unwrap();
    let route_file = dir.path().join("route.yaml");

    write_route_yaml(&route_file, "drain-test", 30, "drain-v1");

    let mock = MockComponent::new();
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(TimerComponent::new());
    ctx.register_component(mock.clone());

    let pattern = format!("{}/*.yaml", dir.path().display());
    let defs = camel_dsl::discover_routes(std::slice::from_ref(&pattern)).unwrap();
    for def in defs {
        ctx.add_route_definition(def).await.unwrap();
    }
    ctx.start().await.unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    let v1_count = if let Some(ep) = mock.get_endpoint("drain-v1") {
        ep.get_received_exchanges().await.len()
    } else {
        0
    };
    assert!(v1_count > 0, "route should have processed exchanges");

    let ctrl = ctx.runtime_execution_handle();
    let patterns_clone = vec![pattern.clone()];
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();
    let watcher_handle = tokio::spawn(async move {
        let dirs = resolve_watch_dirs(&patterns_clone);
        watch_and_reload(
            dirs,
            ctrl,
            move || {
                camel_dsl::discover_routes(&patterns_clone)
                    .map_err(|e| camel_api::CamelError::RouteError(e.to_string()))
            },
            Some(shutdown_clone),
            Duration::from_secs(10),
            Duration::from_millis(50),
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Change from_uri to force Restart action (requires consumer stop + drain)
    write_route_yaml_custom(
        &route_file,
        "drain-test",
        "timer:tock?period=30",
        "drain-v2",
        true,
    );

    tokio::time::sleep(Duration::from_millis(700)).await;

    // Route should be running with new from_uri
    let status = ctx.runtime_route_status("drain-test").await.unwrap();
    assert!(
        matches!(status.as_deref(), Some("Started")),
        "route should be running after restart, got: {:?}",
        status
    );

    let v2_count = if let Some(ep) = mock.get_endpoint("drain-v2") {
        ep.get_received_exchanges().await.len()
    } else {
        0
    };
    assert!(
        v2_count > 0,
        "restarted route should process exchanges via v2"
    );

    shutdown.cancel();
    watcher_handle.await.ok();
    ctx.stop().await.unwrap();
}
