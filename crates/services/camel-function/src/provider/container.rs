use crate::pool::{RunnerHandle, RunnerPoolKey};
use crate::protocol::ProtocolClient;
use camel_api::function::*;
use camel_api::Exchange;
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use super::{FunctionProvider, HealthReport, ProviderError};

struct ContainerEntry {
    container_id: String,
    endpoint: String,
    host_port: u16,
}

#[derive(Debug, Clone)]
pub enum PullPolicy {
    Always,
    Never,
    IfMissing,
}

impl std::fmt::Debug for ContainerProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContainerProvider")
            .field("image", &self.image)
            .field("active_containers", &self.containers_by_handle.len())
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct ContainerProviderBuilder {
    image: String,
    boot_timeout: std::time::Duration,
    pull_policy: PullPolicy,
}

impl Default for ContainerProviderBuilder {
    fn default() -> Self {
        Self {
            image: "rustcamel/deno-runner:latest".to_string(),
            boot_timeout: std::time::Duration::from_secs(10),
            pull_policy: PullPolicy::IfMissing,
        }
    }
}

impl ContainerProviderBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn image(mut self, image: impl Into<String>) -> Self {
        self.image = image.into();
        self
    }

    pub fn boot_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.boot_timeout = timeout;
        self
    }

    pub fn pull_policy(mut self, policy: PullPolicy) -> Self {
        self.pull_policy = policy;
        self
    }

    pub fn build(self) -> Result<ContainerProvider, ProviderError> {
        let docker = bollard::Docker::connect_with_local_defaults()
            .map_err(|e| ProviderError::SpawnFailed(format!("docker connect: {e}")))?;
        Ok(ContainerProvider {
            docker,
            image: self.image,
            boot_timeout: self.boot_timeout,
            pull_policy: self.pull_policy,
            client: ProtocolClient::new(),
            containers_by_handle: DashMap::new(),
        })
    }
}

pub struct ContainerProvider {
    docker: bollard::Docker,
    image: String,
    boot_timeout: std::time::Duration,
    pull_policy: PullPolicy,
    client: ProtocolClient,
    containers_by_handle: DashMap<String, ContainerEntry>,
}

impl ContainerProvider {
    pub fn builder() -> ContainerProviderBuilder {
        ContainerProviderBuilder::new()
    }

    pub async fn cleanup_all(&self) {
        let entries: Vec<(String, String)> = self
            .containers_by_handle
            .iter()
            .map(|e| (e.key().clone(), e.container_id.clone()))
            .collect();
        for (handle_id, container_id) in entries {
            self.stop_and_remove_container(&container_id).await;
            self.containers_by_handle.remove(&handle_id);
        }
    }

    async fn stop_and_remove_container(&self, container_id: &str) {
        let _ = self.docker.stop_container(container_id, None).await;
        match self
            .docker
            .remove_container(
                container_id,
                Some(bollard::query_parameters::RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
        {
            Ok(()) => {}
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => {}
            Err(e) => {
                tracing::warn!(target: "camel_function::container", %container_id, "remove error: {e}");
            }
        }
    }

    async fn allocate_host_port(&self) -> Result<u16, ProviderError> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| ProviderError::SpawnFailed(format!("allocate port: {e}")))?;
        let port = listener
            .local_addr()
            .map_err(|e| ProviderError::SpawnFailed(format!("get port: {e}")))?
            .port();
        drop(listener);
        Ok(port)
    }
}

impl super::sealed::Sealed for ContainerProvider {}

impl Drop for ContainerProvider {
    fn drop(&mut self) {
        if self.containers_by_handle.is_empty() {
            return;
        }
        let docker = self.docker.clone();
        let container_ids: Vec<String> = self
            .containers_by_handle
            .iter()
            .map(|e| e.container_id.clone())
            .collect();
        let _ = tokio::spawn(async move {
            for id in container_ids {
                let _ = docker.stop_container(&id, None).await;
                let _ = docker
                    .remove_container(
                        &id,
                        Some(bollard::query_parameters::RemoveContainerOptions {
                            force: true,
                            ..Default::default()
                        }),
                    )
                    .await;
            }
        });
    }
}
