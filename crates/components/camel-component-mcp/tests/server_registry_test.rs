//! Shared listener registry (Task 2.2): one listener per bind, reuse on the
//! same bind, rejection of conflicting configs. Tool/resource registry units
//! (Task 2.3): registration, readiness, caps, and lookup.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use camel_api::security_policy::{AccessMode, RouteSecurityPlan, TransportId};
use camel_component_mcp::McpServerRegistry;
use camel_component_mcp::config::McpServerConfig;
use camel_component_mcp::error::McpError;
use camel_component_mcp::registry::{McpResourceRegistry, McpToolRegistry};
use camel_component_mcp::types::{McpResourceRead, McpToolInvocation};

fn tool_sender() -> tokio::sync::mpsc::Sender<McpToolInvocation> {
    tokio::sync::mpsc::channel::<McpToolInvocation>(1).0
}

fn resource_sender() -> tokio::sync::mpsc::Sender<McpResourceRead> {
    tokio::sync::mpsc::channel::<McpResourceRead>(1).0
}

fn cfg(bind: &str, tls: Option<serde_json::Value>) -> McpServerConfig {
    serde_json::from_value(serde_json::json!({
        "bind": bind,
        "tls": tls,
        "security_policy": {"require": "auth"},
    }))
    .expect("valid server config")
}

#[tokio::test]
async fn first_consumer_spawns_listener() {
    let bind = "127.0.0.1:0";
    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg(bind, None))
        .await
        .unwrap();

    let result = tokio::net::TcpStream::connect(handle.local_addr).await;
    assert!(
        result.is_ok(),
        "first consumer's listener must accept connections"
    );
}

#[tokio::test]
async fn second_consumer_reuses_listener() {
    let bind = "127.0.0.2:0";
    let cfg = cfg(bind, None);

    let first = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .unwrap();
    let second = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .unwrap();

    assert_eq!(first.local_addr, second.local_addr);
    assert_eq!(first.spawn_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn conflicting_bind_rejected() {
    // Since unify-transport-auth Task 2.4 the typed TLS config is CONSUMED at
    // listener construction (axum-server bind_rustls), so the first spawn
    // needs real PEM material — the conflicting second lookup then trips the
    // registry's tls conflict check as before.
    use camel_component_api::test_support::tls as tls_support;

    let _ = rustls::crypto::ring::default_provider().install_default();

    let bind = "127.0.0.3:0";
    let (_ca, cert_pem, key_pem) = tls_support::gen_server_cert();
    let cert_path = tls_support::write_pem_tmp("mcp-registry-cert.pem", &cert_pem);
    let key_path = tls_support::write_pem_tmp("mcp-registry-key.pem", &key_pem);
    let with_tls = cfg(
        bind,
        Some(serde_json::json!({
            "cert_path": cert_path.to_str().expect("cert path is utf-8"),
            "key_path": key_path.to_str().expect("key path is utf-8"),
        })),
    );
    let without_tls = cfg(bind, None);

    McpServerRegistry::global()
        .get_or_spawn(bind, &with_tls)
        .await
        .unwrap();

    let err = McpServerRegistry::global()
        .get_or_spawn(bind, &without_tls)
        .await
        .unwrap_err();

    assert!(
        matches!(err, McpError::Endpoint(ref message) if message.contains("tls")),
        "expected Endpoint error naming tls, got {err}"
    );
}

#[tokio::test]
async fn dead_listener_is_respawned() {
    let bind = "127.0.0.4:0";
    let cfg = cfg(bind, None);

    let first = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .unwrap();
    assert_eq!(first.spawn_count.load(Ordering::SeqCst), 1);

    // Kill the serve loop via the exposed JoinHandle seam, then wait for the
    // task to finish so `get_or_spawn` observes the dead server.
    first.monitor_task.abort();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !first.monitor_task.is_finished() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "aborted serve loop did not finish in time"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Same bind → the dead entry is evicted and a fresh listener is spawned.
    let second = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .unwrap();

    assert_eq!(second.spawn_count.load(Ordering::SeqCst), 2);
    assert!(
        tokio::net::TcpStream::connect(second.local_addr)
            .await
            .is_ok(),
        "respawned listener must accept connections"
    );
}

#[tokio::test]
async fn conflicting_bind_max_tools_rejected() {
    let bind = "127.0.0.5:0";
    let base = cfg(bind, None);

    McpServerRegistry::global()
        .get_or_spawn(bind, &base)
        .await
        .unwrap();

    let mut tools_diff = base.clone();
    tools_diff.max_tools = Some(1);

    let err = McpServerRegistry::global()
        .get_or_spawn(bind, &tools_diff)
        .await
        .unwrap_err();

    assert!(
        matches!(err, McpError::Endpoint(ref message) if message.contains("max_tools")),
        "expected Endpoint error naming max_tools, got {err}"
    );
}

#[tokio::test]
async fn conflicting_bind_max_resources_rejected() {
    let bind = "127.0.0.6:0";
    let base = cfg(bind, None);

    McpServerRegistry::global()
        .get_or_spawn(bind, &base)
        .await
        .unwrap();

    let mut resources_diff = base.clone();
    resources_diff.max_resources = Some(1);

    let err = McpServerRegistry::global()
        .get_or_spawn(bind, &resources_diff)
        .await
        .unwrap_err();

    assert!(
        matches!(err, McpError::Endpoint(ref message) if message.contains("max_resources")),
        "expected Endpoint error naming max_resources, got {err}"
    );
}

#[tokio::test]
async fn conflicting_allowed_hosts_rejected() {
    let bind = "127.0.0.7:0";
    let base = serde_json::from_value(serde_json::json!({
        "bind": bind,
        "security_policy": {"require": "auth"},
        "allowed_hosts": ["host-a.example"],
    }))
    .expect("valid server config with allowed_hosts");

    McpServerRegistry::global()
        .get_or_spawn(bind, &base)
        .await
        .unwrap();

    // Same bind, same tls/caps — only a stricter allowed_hosts list differs.
    let mut narrowed = base.clone();
    narrowed.allowed_hosts = Some(vec!["host-b.example".to_string()]);

    let err = McpServerRegistry::global()
        .get_or_spawn(bind, &narrowed)
        .await
        .unwrap_err();

    assert!(
        matches!(err, McpError::Endpoint(ref message) if message.contains("allowed_hosts")),
        "expected Endpoint error naming allowed_hosts, got {err}"
    );
}

#[test]
fn duplicate_tool_registration_rejected() {
    let registry = McpToolRegistry::new(128);
    let owner_a = Arc::new(());
    registry
        .register(
            "lookup".to_string(),
            "route-1".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_a),
        )
        .unwrap();

    let owner_b = Arc::new(());
    let err = registry
        .register(
            "lookup".to_string(),
            "route-1".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_b),
        )
        .unwrap_err();

    assert!(
        matches!(err, McpError::Endpoint(ref message)
            if message.contains("lookup") && message.contains("already registered")),
        "expected an Endpoint error naming the duplicate tool, got {err}"
    );
}

#[test]
fn duplicate_resource_registration_rejected() {
    let registry = McpResourceRegistry::new(128);
    let owner_a = Arc::new(());
    registry
        .register(
            "crm://customers".to_string(),
            "route-1".to_string(),
            resource_sender(),
            Arc::downgrade(&owner_a),
        )
        .unwrap();

    let owner_b = Arc::new(());
    let err = registry
        .register(
            "crm://customers".to_string(),
            "route-1".to_string(),
            resource_sender(),
            Arc::downgrade(&owner_b),
        )
        .unwrap_err();

    assert!(
        matches!(err, McpError::Endpoint(ref message)
            if message.contains("crm://customers") && message.contains("already registered")),
        "expected an Endpoint error naming the duplicate resource, got {err}"
    );
}

#[test]
fn register_129th_tool_rejected() {
    let registry = McpToolRegistry::new(128);
    // Strong tokens stay bound for the test's duration so all 128 entries
    // count as live against the cap.
    let owners: Vec<Arc<()>> = (0..128).map(|_| Arc::new(())).collect();

    for (i, owner) in owners.iter().enumerate() {
        registry
            .register(
                format!("tool_{i}"),
                format!("route-{i}"),
                tool_sender(),
                serde_json::json!({}),
                Arc::downgrade(owner),
            )
            .unwrap();
    }

    let owner_128 = Arc::new(());
    let err = registry
        .register(
            "tool_128".to_string(),
            "route-128".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_128),
        )
        .unwrap_err();

    assert!(
        matches!(err, McpError::CapExceeded { ref kind, max } if kind == "tools" && max == 128),
        "expected CapExceeded for tools at max 128, got {err}"
    );
}

#[test]
fn raised_cap_allows_150() {
    let registry = McpToolRegistry::new(200);
    let owners: Vec<Arc<()>> = (0..150).map(|_| Arc::new(())).collect();

    for (i, owner) in owners.iter().enumerate() {
        registry
            .register(
                format!("tool_{i}"),
                format!("route-{i}"),
                tool_sender(),
                serde_json::json!({}),
                Arc::downgrade(owner),
            )
            .unwrap();
    }
}

#[test]
fn not_ready_tool_hidden_from_list() {
    let registry = McpToolRegistry::new(128);
    let owner = Arc::new(());
    registry
        .register(
            "hidden".to_string(),
            "route-1".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner),
        )
        .unwrap();

    assert!(
        registry.list_ready().is_empty(),
        "a not-ready tool must be hidden from listing"
    );

    registry.mark_ready("hidden");

    let ready = registry.list_ready();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].0, "hidden");
}

#[test]
fn stopped_tool_unregistered() {
    let registry = McpToolRegistry::new(128);
    let owner = Arc::new(());
    registry
        .register(
            "lookup".to_string(),
            "route-1".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner),
        )
        .unwrap();
    registry.mark_ready("lookup");

    registry.unregister("lookup");

    assert!(
        registry.resolve("lookup").is_none(),
        "an unregistered tool must not resolve"
    );
}

#[test]
fn unknown_resource_uri_unresolved() {
    let registry = McpResourceRegistry::new(128);

    assert!(
        registry.resolve("crm://unknown").is_none(),
        "an unknown resource URI must not resolve"
    );
}

#[test]
fn resource_cap_enforced() {
    let registry = McpResourceRegistry::new(2);
    // Strong tokens stay bound for the test's duration so all entries count
    // as live against the cap.
    let owner_a = Arc::new(());
    let owner_b = Arc::new(());

    registry
        .register(
            "crm://a".to_string(),
            "route-a".to_string(),
            resource_sender(),
            Arc::downgrade(&owner_a),
        )
        .unwrap();
    registry
        .register(
            "crm://b".to_string(),
            "route-b".to_string(),
            resource_sender(),
            Arc::downgrade(&owner_b),
        )
        .unwrap();

    let owner_c = Arc::new(());
    let err = registry
        .register(
            "crm://c".to_string(),
            "route-c".to_string(),
            resource_sender(),
            Arc::downgrade(&owner_c),
        )
        .unwrap_err();

    assert!(
        matches!(err, McpError::CapExceeded { ref kind, max } if kind == "resources" && max == 2),
        "expected CapExceeded for resources at max 2, got {err}"
    );
}

// ── Owner-liveness (fix-mcp-dead-registry-entry Tasks 1-2) ──
//
// A registration carries a `Weak<()>` owner token; the entry dies with its
// strong `Arc`. Dropping the `Arc` in a test reads as a dead owner.

#[test]
fn dead_owner_tool_entry_is_replaced_on_register() {
    let registry = McpToolRegistry::new(128);
    let owner_a = Arc::new(());
    registry
        .register(
            "t".to_string(),
            "route-a".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_a),
        )
        .unwrap();
    drop(owner_a);

    let owner_b = Arc::new(());
    registry
        .register(
            "t".to_string(),
            "route-b".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_b),
        )
        .expect("a dead owner's tool entry must be replaced");

    assert_eq!(
        registry.resolve("t").expect("replacement entry").route_id,
        "route-b",
        "the replacement must resolve to the new owner's route"
    );
}

#[test]
fn live_duplicate_tool_registration_still_rejected() {
    let registry = McpToolRegistry::new(128);
    let owner_a = Arc::new(());
    registry
        .register(
            "t".to_string(),
            "route-a".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_a),
        )
        .unwrap();

    let owner_b = Arc::new(());
    let err = registry
        .register(
            "t".to_string(),
            "route-b".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_b),
        )
        .unwrap_err();

    assert!(
        matches!(err, McpError::Endpoint(ref message)
            if message.contains("already registered")),
        "a live owner's duplicate must still be rejected, got {err}"
    );
}

#[test]
fn late_owner_unregister_does_not_remove_replacement() {
    let registry = McpToolRegistry::new(128);
    let owner_a = Arc::new(());
    let weak_a = Arc::downgrade(&owner_a);
    registry
        .register(
            "t".to_string(),
            "route-a".to_string(),
            tool_sender(),
            serde_json::json!({}),
            weak_a.clone(),
        )
        .unwrap();
    drop(owner_a);

    let owner_b = Arc::new(());
    registry
        .register(
            "t".to_string(),
            "route-b".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_b),
        )
        .expect("dead owner's entry must be replaced");

    assert!(
        !registry.unregister_owned("t", &weak_a),
        "a late old-owner unregister must not remove the replacement"
    );
    assert_eq!(
        registry
            .resolve("t")
            .expect("replacement survives")
            .route_id,
        "route-b"
    );

    assert!(
        registry.unregister_owned("t", &Arc::downgrade(&owner_b)),
        "the live owner's own unregister must remove the entry"
    );
    assert!(
        registry.resolve("t").is_none(),
        "the owner-matched unregister must have removed the entry"
    );
}

#[test]
fn dead_tool_entry_pruned_from_list_ready_and_cap_reclaimed() {
    let registry = McpToolRegistry::new(2);
    let owner_1 = Arc::new(());
    let owner_2 = Arc::new(());
    registry
        .register(
            "t1".to_string(),
            "route-1".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_1),
        )
        .unwrap();
    registry
        .register(
            "t2".to_string(),
            "route-2".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_2),
        )
        .unwrap();
    registry.mark_ready("t1");
    registry.mark_ready("t2");
    drop(owner_1);

    let ready: Vec<String> = registry
        .list_ready()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(ready, vec!["t2".to_string()]);

    let owner_3 = Arc::new(());
    registry
        .register(
            "t3".to_string(),
            "route-3".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_3),
        )
        .expect("the dead entry's cap slot must be reclaimed");
}

#[test]
fn dead_entry_under_other_name_releases_slot_on_register() {
    let registry = McpToolRegistry::new(2);
    let owner_a = Arc::new(());
    let owner_b = Arc::new(());
    registry
        .register(
            "t1".to_string(),
            "route-1".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_a),
        )
        .unwrap();
    registry
        .register(
            "t2".to_string(),
            "route-2".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_b),
        )
        .unwrap();
    registry.mark_ready("t1");
    registry.mark_ready("t2");
    // Drop A without any list/resolve call: only the prune-on-register sweep
    // can release t1's slot.
    drop(owner_a);

    let owner_c = Arc::new(());
    registry
        .register(
            "t3".to_string(),
            "route-3".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_c),
        )
        .expect("the dead entry's slot must release on register");
    registry.mark_ready("t3");

    let owner_d = Arc::new(());
    let err = registry
        .register(
            "t4".to_string(),
            "route-4".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_d),
        )
        .unwrap_err();
    assert!(
        matches!(err, McpError::CapExceeded { ref kind, max } if kind == "tools" && max == 2),
        "only the 2 live entries must occupy the cap, got {err}"
    );

    let mut ready: Vec<String> = registry
        .list_ready()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    ready.sort();
    assert_eq!(ready, vec!["t2".to_string(), "t3".to_string()]);
}

#[test]
fn takeover_at_full_cap_does_not_consume_extra_slot() {
    let registry = McpToolRegistry::new(2);
    let owner_a = Arc::new(());
    let owner_b = Arc::new(());
    registry
        .register(
            "t1".to_string(),
            "route-1".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_a),
        )
        .unwrap();
    registry
        .register(
            "t2".to_string(),
            "route-2".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_b),
        )
        .unwrap();
    drop(owner_a);

    let owner_c = Arc::new(());
    registry
        .register(
            "t1".to_string(),
            "route-3".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_c),
        )
        .expect("the takeover must replace the dead entry");

    let owner_d = Arc::new(());
    let err = registry
        .register(
            "t4".to_string(),
            "route-4".to_string(),
            tool_sender(),
            serde_json::json!({}),
            Arc::downgrade(&owner_d),
        )
        .unwrap_err();
    assert!(
        matches!(err, McpError::CapExceeded { ref kind, max } if kind == "tools" && max == 2),
        "the takeover must not consume an extra cap slot, got {err}"
    );
}

#[test]
fn dead_owner_resource_entry_is_replaced_on_register() {
    let registry = McpResourceRegistry::new(128);
    let owner_a = Arc::new(());
    registry
        .register(
            "crm://x".to_string(),
            "route-a".to_string(),
            resource_sender(),
            Arc::downgrade(&owner_a),
        )
        .unwrap();
    registry.mark_ready("crm://x");
    drop(owner_a);

    // Spec: resources/list SHALL NOT advertise a URI whose owner is dead.
    assert!(
        !registry.list_ready().contains(&"crm://x".to_string()),
        "a dead owner's resource URI must not be advertised"
    );

    let owner_b = Arc::new(());
    registry
        .register(
            "crm://x".to_string(),
            "route-b".to_string(),
            resource_sender(),
            Arc::downgrade(&owner_b),
        )
        .expect("a dead owner's resource entry must be replaced");

    assert_eq!(
        registry
            .resolve("crm://x")
            .expect("replacement entry")
            .route_id,
        "route-b",
        "the replacement must resolve to the new owner's route"
    );
}

#[test]
fn live_duplicate_resource_registration_still_rejected() {
    let registry = McpResourceRegistry::new(128);
    let owner_a = Arc::new(());
    registry
        .register(
            "crm://x".to_string(),
            "route-a".to_string(),
            resource_sender(),
            Arc::downgrade(&owner_a),
        )
        .unwrap();

    let owner_b = Arc::new(());
    let err = registry
        .register(
            "crm://x".to_string(),
            "route-b".to_string(),
            resource_sender(),
            Arc::downgrade(&owner_b),
        )
        .unwrap_err();

    assert!(
        matches!(err, McpError::Endpoint(ref message)
            if message.contains("already registered")),
        "a live owner's duplicate must still be rejected, got {err}"
    );
}

#[test]
fn late_owner_resource_unregister_does_not_remove_replacement() {
    let registry = McpResourceRegistry::new(128);
    let owner_a = Arc::new(());
    let weak_a = Arc::downgrade(&owner_a);
    registry
        .register(
            "crm://x".to_string(),
            "route-a".to_string(),
            resource_sender(),
            weak_a.clone(),
        )
        .unwrap();
    drop(owner_a);

    let owner_b = Arc::new(());
    registry
        .register(
            "crm://x".to_string(),
            "route-b".to_string(),
            resource_sender(),
            Arc::downgrade(&owner_b),
        )
        .expect("dead owner's entry must be replaced");

    assert!(
        !registry.unregister_owned("crm://x", &weak_a),
        "a late old-owner unregister must not remove the replacement"
    );
    assert_eq!(
        registry
            .resolve("crm://x")
            .expect("replacement survives")
            .route_id,
        "route-b"
    );

    assert!(
        registry.unregister_owned("crm://x", &Arc::downgrade(&owner_b)),
        "the live owner's own unregister must remove the entry"
    );
    assert!(
        registry.resolve("crm://x").is_none(),
        "the owner-matched unregister must have removed the entry"
    );
}

// --- Bind-security plan ownership (Task 3, ADR-0068) ---

fn plan(provider_ref: &str) -> RouteSecurityPlan {
    RouteSecurityPlan {
        access_mode: AccessMode::Authenticated,
        provider_ref: Some(provider_ref.to_string()),
        transport: TransportId::Mcp,
        credential_sources: vec![],
        audience_binding: None,
    }
}

#[test]
fn live_owner_plan_not_overwritten_and_dead_plan_replaced() {
    // Unique never-bound address: `bind_security` opens no socket, it only
    // creates this test's private per-bind security book in the global
    // registry.
    let security = McpServerRegistry::global().bind_security("127.0.0.1:19471");
    let owner_a = Arc::new(());
    let weak_a = Arc::downgrade(&owner_a);
    let owner_b = Arc::new(());

    security.register_plan("r1", plan("plan-a"), None, Arc::downgrade(&owner_a));
    security.register_plan("r1", plan("plan-b"), None, Arc::downgrade(&owner_b));
    assert_eq!(
        security
            .plan_for("r1")
            .expect("incumbent plan must survive the second registration")
            .provider_ref,
        Some("plan-a".to_string()),
        "a live owner's plan must not be overwritten"
    );

    drop(owner_a);
    assert!(
        !security.plans_snapshot().iter().any(|(id, _)| id == "r1"),
        "a dead owner's plan must leave the exposure-gate input"
    );

    security.register_plan("r1", plan("plan-b"), None, Arc::downgrade(&owner_b));
    assert_eq!(
        security
            .plan_for("r1")
            .expect("replacement plan")
            .provider_ref,
        Some("plan-b".to_string()),
        "a dead owner's plan must be replaced"
    );

    security.unregister_plan_owned("r1", &weak_a);
    assert_eq!(
        security
            .plan_for("r1")
            .expect("replacement plan")
            .provider_ref,
        Some("plan-b".to_string()),
        "a late stop of the dead owner must keep the replacement's plan"
    );
}

#[test]
fn failed_unregister_plan_keeps_incumbent() {
    let security = McpServerRegistry::global().bind_security("127.0.0.1:19472");
    let owner_a = Arc::new(());
    let owner_b = Arc::new(());

    security.register_plan("r1", plan("plan-a"), None, Arc::downgrade(&owner_a));
    assert!(
        !security.unregister_plan_owned("r1", &Arc::downgrade(&owner_b)),
        "a non-owner unregister must not report a removal"
    );
    assert_eq!(
        security
            .plan_for("r1")
            .expect("incumbent plan")
            .provider_ref,
        Some("plan-a".to_string()),
        "a failed unregister must keep the incumbent's plan"
    );
}

#[test]
fn loser_cleanup_cannot_strip_winner_plan() {
    // Deterministic reproduction of the concurrent-start interleaving
    // (bd rc-apvm): A dies holding entry "t" + plan r1. B and C start on
    // the same route identity: B's `register_plan` overwrites A's dead
    // plan; C's `register_plan` keeps live B's plan; C wins the entry
    // `register`; B fails the duplicate guard and runs its
    // owner-conditional cleanup.
    let security = McpServerRegistry::global().bind_security("127.0.0.1:19473");
    let owner_a = Arc::new(());
    let owner_b = Arc::new(());
    let weak_b = Arc::downgrade(&owner_b);
    let owner_c = Arc::new(());

    security.register_plan("r1", plan("plan-a"), None, Arc::downgrade(&owner_a));
    drop(owner_a);
    // B's register_plan takes over A's dead plan.
    security.register_plan("r1", plan("plan-b"), None, weak_b.clone());
    // C's register_plan sees live B and keeps B's plan.
    security.register_plan("r1", plan("plan-c"), None, Arc::downgrade(&owner_c));
    assert_eq!(
        security
            .plan_for("r1")
            .expect("live incumbent plan")
            .provider_ref,
        Some("plan-b".to_string()),
        "C's keep-incumbent registration must keep B's plan"
    );

    // C wins the entry registration and re-asserts the plan (ADR-0068
    // winner re-assertion).
    security.register_plan_takeover("r1", plan("plan-c"), None, Arc::downgrade(&owner_c));
    // B fails the duplicate guard; its failure cleanup runs.
    security.unregister_plan_owned("r1", &weak_b);
    assert_eq!(
        security.plan_for("r1").expect("winner plan").provider_ref,
        Some("plan-c".to_string()),
        "the entry winner's plan must own the route after the loser's cleanup"
    );
}

#[test]
fn takeover_by_incumbent_owner_keeps_working_plan() {
    // The no-op shape: re-asserting the plan under the incumbent owner
    // itself overwrites with the same content and must leave a working
    // plan behind.
    let security = McpServerRegistry::global().bind_security("127.0.0.1:19474");
    let owner = Arc::new(());

    security.register_plan("r1", plan("plan-a"), None, Arc::downgrade(&owner));
    security.register_plan_takeover("r1", plan("plan-a"), None, Arc::downgrade(&owner));
    assert_eq!(
        security
            .plan_for("r1")
            .expect("plan after own-owner takeover")
            .provider_ref,
        Some("plan-a".to_string()),
        "a same-owner takeover must keep a working plan"
    );
}
