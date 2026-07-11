//! ExecEndpoint — producer-only endpoint for system command execution.
//!
//! URI form: `exec:{profile-name}`. Consumers return an error — exec is
//! producer-only. The producer runs the enforcement flow defined by the
//! resolved exec profile.

use std::sync::Arc;

use camel_api::{BoxProcessor, CamelError};
use camel_component_api::{Consumer, Endpoint, ProducerContext, RuntimeObservability};
use tokio::sync::Semaphore;

use crate::config::ExecGlobalConfig;
use crate::producer::ExecProducer;

/// An endpoint backed by a named exec profile.
///
/// Each `exec:{name}` URI resolves to one profile. The endpoint caches the
/// global config Arc so every producer creation shares the same validated
/// config state (including the startup-pinned canonical workspace root).
pub struct ExecEndpoint {
    uri: String,
    profile_name: String,
    global: Arc<ExecGlobalConfig>,
}

impl ExecEndpoint {
    pub fn new(uri: String, profile_name: String, global: Arc<ExecGlobalConfig>) -> Self {
        Self {
            uri,
            profile_name,
            global,
        }
    }
}

impl Endpoint for ExecEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        // CamelError has no UnsupportedOperation variant; InvalidUri is the
        // closest classification for a producer-only component. The message
        // "producer-only" is asserted by endpoint_is_producer_only test.
        Err(CamelError::InvalidUri(
            "exec component is producer-only".into(),
        ))
    }

    fn create_producer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
        ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        let profile = self
            .global
            .profile(&self.profile_name)
            .ok_or_else(|| {
                CamelError::EndpointCreationFailed(format!(
                    "exec profile {:?} not found",
                    self.profile_name
                ))
            })?
            .clone();

        let route_id = ctx.route_id().unwrap_or("unknown").to_string();
        let semaphore = Arc::new(Semaphore::new(profile.concurrency));
        let host_env =
            Arc::new(std::env::vars().collect::<std::collections::HashMap<String, String>>());

        let producer = ExecProducer {
            profile: Arc::new(profile),
            global: Arc::clone(&self.global),
            route_id,
            host_env,
            semaphore,
            rt: Some(rt),
        };

        Ok(BoxProcessor::new(producer))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use camel_api::CamelError;
    use camel_component_api::{Component, NoOpComponentContext, RuntimeObservability};

    use crate::ExecComponent;
    use crate::config::ExecGlobalConfig;

    fn component_with_echo() -> ExecComponent {
        let cfg: ExecGlobalConfig = toml::from_str(
            r#"
[[profiles]]
name = "echo"
executable = "echo"
"#,
        )
        .unwrap();
        ExecComponent::new(cfg)
    }

    fn extract_err<T>(result: Result<T, CamelError>) -> CamelError {
        match result {
            Err(e) => e,
            Ok(_) => panic!("expected Err, got Ok"),
        }
    }

    #[test]
    fn create_endpoint_rejects_missing_prefix() {
        let c = component_with_echo();
        let ctx = NoOpComponentContext;
        let err = extract_err(c.create_endpoint("notecho:echo", &ctx));
        assert!(
            matches!(&err, CamelError::InvalidUri(_)),
            "expected InvalidUri, got {err:?}"
        );
        assert!(
            err.to_string().contains("exec:"),
            "should mention 'exec:': {err}"
        );
    }

    #[test]
    fn create_endpoint_rejects_empty_profile() {
        let c = component_with_echo();
        let ctx = NoOpComponentContext;
        let err = extract_err(c.create_endpoint("exec:", &ctx));
        assert!(
            matches!(&err, CamelError::InvalidUri(_)),
            "expected InvalidUri, got {err:?}"
        );
        assert!(
            err.to_string().contains("profile name"),
            "should mention 'profile name': {err}"
        );
    }

    #[test]
    fn create_endpoint_strips_query_string() {
        let c = component_with_echo();
        let ctx = NoOpComponentContext;
        let ep = c
            .create_endpoint("exec:echo?foo=bar", &ctx)
            .expect("should succeed with query string");
        assert_eq!(ep.uri(), "exec:echo?foo=bar");
    }

    #[test]
    fn create_endpoint_rejects_unknown_profile() {
        let c = component_with_echo();
        let ctx = NoOpComponentContext;
        let err = extract_err(c.create_endpoint("exec:nope", &ctx));
        assert!(
            matches!(&err, CamelError::EndpointCreationFailed(_)),
            "expected EndpointCreationFailed, got {err:?}"
        );
        assert!(
            err.to_string().contains("not configured"),
            "should mention 'not configured': {err}"
        );
    }

    #[test]
    fn create_endpoint_valid_profile_ok() {
        let c = component_with_echo();
        let ctx = NoOpComponentContext;
        let ep = c
            .create_endpoint("exec:echo", &ctx)
            .expect("should succeed");
        assert_eq!(ep.uri(), "exec:echo");
    }

    #[test]
    fn endpoint_is_producer_only() {
        let c = component_with_echo();
        let ctx = NoOpComponentContext;
        let ep = c
            .create_endpoint("exec:echo", &ctx)
            .expect("should succeed");
        let rt: Arc<dyn RuntimeObservability> = Arc::new(NoOpComponentContext);
        let err = extract_err(ep.create_consumer(rt));
        assert!(
            matches!(&err, CamelError::InvalidUri(_)),
            "expected InvalidUri, got {err:?}"
        );
        assert!(
            err.to_string().contains("producer-only"),
            "should mention 'producer-only': {err}"
        );
    }
}
