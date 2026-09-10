//! Inbound listener provisioning (feature `http`, rc-5yon).
//!
//! The document-level `inbound:` declaration names a bind variable.
//! [`boot_scenario`](crate::boot_scenario::boot_scenario) binds an OS-chosen port on
//! `127.0.0.1` (port `0`), stages the pre-bound listener on the HTTP
//! component's global ServerRegistry (ADR-0070 staged
//! consumption), and extends the layered environment with the bound
//! URL under the declared bindVar before route discovery — so route
//! consumer URIs interpolate the staged socket and never pin a port.

use camel_api::CamelError;

use crate::document::InboundListener;

/// Binds `127.0.0.1:0`, stages the listener on the HTTP component's
/// global ServerRegistry, and returns the bound address.
///
/// Staging is keyed by the listener's actual local address and
/// one-shot: the next exact-key `get_or_spawn` — the scenario route's
/// own consumer — serves this socket, and a second staging for the
/// same key surfaces as a registry error. A fresh port-0 bind resolves
/// a fresh port per call, so duplicate provisioning across documents
/// cannot collide. Call before any component can spawn a consumer for
/// the key; [`boot_scenario`](crate::boot_scenario::boot_scenario) does exactly that.
pub async fn provision_inbound(
    entry: &InboundListener,
) -> Result<std::net::SocketAddr, CamelError> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| {
            CamelError::EndpointCreationFailed(format!("inbound listener bind 127.0.0.1:0: {e}"))
        })?;
    let bound = listener.local_addr().map_err(|e| {
        CamelError::EndpointCreationFailed(format!("inbound listener local_addr: {e}"))
    })?;
    camel_component_http::ServerRegistry::global()
        .stage_listener(listener)
        .await?;
    tracing::debug!(
        bind_var = %entry.bind_var,
        %bound,
        "staged inbound listener for the scenario boot"
    );
    Ok(bound)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::boot_scenario::boot_scenario;
    use crate::document::{InboundListener, RouteSource, ScenarioDocument};
    use crate::env_layers::{LayeredEnv, ambient_std};

    /// Boots a minimal project whose route template references the
    /// inbound bindVar. Discovery resolves `${env:INBOUND}` through the
    /// environment `boot_scenario` extends, so a successful boot proves
    /// the discovery environment carries the variable (an unresolved
    /// placeholder fails discovery with a named Env error); the boot
    /// result carries the same provisioned address in `inbound_bound`.
    #[tokio::test]
    async fn inbound_binds_port_zero() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("Camel.toml"), "# minimal\n").expect("write Camel.toml");
        std::fs::write(
            dir.path().join("routes.yaml"),
            r#"
routes:
  - id: inbound-route
    from: direct:start
    steps:
      - to: "log:${env:INBOUND}"
"#,
        )
        .expect("write routes.yaml");
        let doc = ScenarioDocument {
            source_path: dir.path().join("case.test.yaml"),
            route_source: RouteSource::RouteFiles(vec!["routes.yaml".into()]),
            scenario: Vec::new(),
            partners: None,
            env: None,
            env_passthrough: None,
            profile: None,
            send_deadline: None,
            inbound: Some(InboundListener {
                bind_var: "INBOUND".to_string(),
            }),
            logs: None,
        };
        let env = LayeredEnv::new(BTreeMap::new(), BTreeMap::new(), Vec::new(), ambient_std());
        let mut run = boot_scenario(&doc, dir.path(), &env)
            .await
            .expect("boot must provision the inbound listener and extend the env");
        let bound = run
            .inbound_bound
            .expect("the boot result must carry the provisioned inbound address");
        assert!(
            bound.ip().is_loopback(),
            "the listener must bind the loopback address: {bound}"
        );
        assert_ne!(bound.port(), 0, "port 0 must resolve to a real port");
        run.boot
            .shutdown(&mut run.ctx)
            .await
            .expect("clean shutdown must complete");
    }
}
