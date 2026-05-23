use crate::pool::{RunnerHandle, RunnerPoolKey};
use crate::protocol::ProtocolClient;
use camel_api::Exchange;
use camel_api::function::*;
use dashmap::DashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use super::{FunctionProvider, HealthReport, ProviderError};

struct ContainerEntry {
    container_id: String,
    endpoint: String,
    #[allow(dead_code)]
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
    instance_id: Option<String>,
}

impl Default for ContainerProviderBuilder {
    fn default() -> Self {
        Self {
            image: "kennycallado/deno-runner:latest".to_string(),
            boot_timeout: std::time::Duration::from_secs(10),
            pull_policy: PullPolicy::IfMissing,
            instance_id: None,
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

    pub fn instance_id(mut self, id: impl Into<String>) -> Self {
        self.instance_id = Some(id.into());
        self
    }

    pub fn build(self) -> Result<ContainerProvider, ProviderError> {
        let docker = bollard::Docker::connect_with_local_defaults()
            .map_err(|e| ProviderError::SpawnFailed(format!("docker connect: {e}")))?;
        let instance_id = self.instance_id.unwrap_or_else(|| {
            let hash = blake3::hash(
                format!(
                    "{}-{}",
                    self.image,
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                )
                .as_bytes(),
            );
            format!("inst-{}", &hash.to_hex()[..12])
        });
        Ok(ContainerProvider {
            docker,
            image: self.image,
            boot_timeout: self.boot_timeout,
            pull_policy: self.pull_policy,
            client: ProtocolClient::new(),
            containers_by_handle: DashMap::new(),
            instance_id,
        })
    }
}

pub struct ContainerProvider {
    docker: bollard::Docker,
    image: String,
    instance_id: String,
    #[allow(dead_code)]
    boot_timeout: std::time::Duration,
    #[allow(dead_code)]
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

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub async fn is_clean(&self) -> bool {
        self.list_instance_containers().await.is_empty()
    }

    pub async fn list_instance_containers(&self) -> Vec<String> {
        let options = bollard::query_parameters::ListContainersOptions {
            filters: Some(std::collections::HashMap::from([(
                "label".to_string(),
                vec![format!("camel.function.instance={}", self.instance_id)],
            )])),
            ..Default::default()
        };
        match self.docker.list_containers(Some(options)).await {
            Ok(containers) => containers.into_iter().filter_map(|c| c.id).collect(),
            Err(_) => vec![],
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
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => {}
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

    pub async fn spawn_runner(&self, runtime: &str) -> Result<RunnerHandle, ProviderError> {
        let key = RunnerPoolKey {
            runtime: runtime.to_string(),
        };
        FunctionProvider::spawn(self, &key).await
    }

    pub async fn shutdown_runner(&self, handle: RunnerHandle) -> Result<(), ProviderError> {
        FunctionProvider::shutdown(self, handle).await
    }

    pub async fn health_runner(
        &self,
        handle: &RunnerHandle,
    ) -> Result<HealthReport, ProviderError> {
        FunctionProvider::health(self, handle).await
    }

    pub async fn register_function(
        &self,
        handle: &RunnerHandle,
        def: &FunctionDefinition,
    ) -> Result<(), ProviderError> {
        FunctionProvider::register(self, handle, def).await
    }

    pub async fn unregister_function(
        &self,
        handle: &RunnerHandle,
        id: &FunctionId,
    ) -> Result<(), ProviderError> {
        FunctionProvider::unregister(self, handle, id).await
    }

    pub async fn invoke_function(
        &self,
        handle: &RunnerHandle,
        function_id: &FunctionId,
        exchange: &Exchange,
    ) -> Result<ExchangePatch, ProviderError> {
        FunctionProvider::invoke(
            self,
            handle,
            function_id,
            exchange,
            std::time::Duration::from_millis(5000),
        )
        .await
    }

    fn spawn_log_forwarder(&self, container_id: String) {
        use futures::StreamExt;

        let docker = self.docker.clone();
        tokio::spawn(async move {
            let options = bollard::query_parameters::LogsOptions {
                follow: true,
                stdout: true,
                stderr: true,
                ..Default::default()
            };
            let mut stream = docker.logs(&container_id, Some(options));

            while let Some(msg) = stream.next().await {
                match msg {
                    Ok(log_output) => {
                        let text = match &log_output {
                            bollard::container::LogOutput::StdOut { message } => {
                                String::from_utf8_lossy(message).into_owned()
                            }
                            bollard::container::LogOutput::StdErr { message } => {
                                String::from_utf8_lossy(message).into_owned()
                            }
                            _ => continue,
                        };
                        let trimmed = text.trim_end();
                        if trimmed.is_empty() {
                            continue;
                        }
                        match &log_output {
                            bollard::container::LogOutput::StdOut { .. } => {
                                tracing::info!(target: "camel_function::runner", "{trimmed}");
                            }
                            bollard::container::LogOutput::StdErr { .. } => {
                                tracing::warn!(target: "camel_function::runner", "{trimmed}");
                            }
                            _ => {}
                        }
                    }
                    Err(e) => {
                        tracing::debug!(target: "camel_function::container", "log stream error: {e}");
                        break;
                    }
                }
            }
        });
    }
}

impl super::sealed::Sealed for ContainerProvider {}

#[async_trait::async_trait]
impl FunctionProvider for ContainerProvider {
    async fn spawn(&self, _key: &RunnerPoolKey) -> Result<RunnerHandle, ProviderError> {
        let hash = blake3::hash(
            format!(
                "{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            )
            .as_bytes(),
        )
        .to_hex();
        let handle_id = format!("deno-{}", &hash[..16]);
        let host_port = self.allocate_host_port().await?;

        tracing::debug!(
            target: "camel_function::container",
            %handle_id,
            image = %self.image,
            "spawning container"
        );

        let labels = std::collections::HashMap::from([
            ("camel.function.runner".to_string(), "true".to_string()),
            ("camel.function.context".to_string(), handle_id.clone()),
            (
                "camel.function.instance".to_string(),
                self.instance_id.clone(),
            ),
        ]);

        let config = bollard::models::ContainerCreateBody {
            image: Some(self.image.clone()),
            env: Some(vec![
                "PORT=8080".to_string(),
                "DENO_NO_PROMPT=1".to_string(),
            ]),
            labels: Some(labels),
            exposed_ports: Some(vec!["8080/tcp".to_string()]),
            host_config: Some(bollard::models::HostConfig {
                port_bindings: Some(std::collections::HashMap::from([(
                    "8080/tcp".to_string(),
                    Some(vec![bollard::models::PortBinding {
                        host_ip: Some("127.0.0.1".to_string()),
                        host_port: Some(host_port.to_string()),
                    }]),
                )])),
                init: Some(true),
                auto_remove: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        };

        let create_opts = bollard::query_parameters::CreateContainerOptions {
            name: Some(handle_id.clone()),
            ..Default::default()
        };

        let create_result = self
            .docker
            .create_container(Some(create_opts), config)
            .await
            .map_err(|e| ProviderError::SpawnFailed(format!("create container: {e}")))?;

        let container_id = create_result.id;

        if let Err(e) = self.docker.start_container(&container_id, None).await {
            let _ = self.stop_and_remove_container(&container_id).await;
            return Err(ProviderError::SpawnFailed(format!("start container: {e}")));
        }

        let endpoint = format!("http://127.0.0.1:{host_port}");

        self.spawn_log_forwarder(container_id.clone());

        self.containers_by_handle.insert(
            handle_id.clone(),
            ContainerEntry {
                container_id,
                endpoint,
                host_port,
            },
        );

        Ok(RunnerHandle {
            id: handle_id,
            state: Arc::new(std::sync::Mutex::new(crate::pool::RunnerState::Booting)),
            cancel: CancellationToken::new(),
        })
    }

    async fn shutdown(&self, handle: RunnerHandle) -> Result<(), ProviderError> {
        handle.cancel.cancel();
        let entry = match self.containers_by_handle.remove(&handle.id) {
            Some((_, entry)) => entry,
            None => return Ok(()),
        };
        let _ = self.client.shutdown(&entry.endpoint).await;
        self.stop_and_remove_container(&entry.container_id).await;
        Ok(())
    }

    async fn health(&self, handle: &RunnerHandle) -> Result<HealthReport, ProviderError> {
        let endpoint = self
            .containers_by_handle
            .get(&handle.id)
            .ok_or_else(|| ProviderError::HealthFailed(format!("unknown handle {}", handle.id)))?
            .endpoint
            .clone();
        self.client.health(&endpoint).await
    }

    async fn register(
        &self,
        handle: &RunnerHandle,
        def: &FunctionDefinition,
    ) -> Result<(), ProviderError> {
        let endpoint = self
            .containers_by_handle
            .get(&handle.id)
            .ok_or_else(|| ProviderError::RegisterFailed(format!("unknown handle {}", handle.id)))?
            .endpoint
            .clone();
        self.client.register(&endpoint, def).await
    }

    async fn unregister(
        &self,
        handle: &RunnerHandle,
        id: &FunctionId,
    ) -> Result<(), ProviderError> {
        let endpoint = self
            .containers_by_handle
            .get(&handle.id)
            .ok_or_else(|| {
                ProviderError::UnregisterFailed(format!("unknown handle {}", handle.id))
            })?
            .endpoint
            .clone();
        self.client.unregister(&endpoint, id).await
    }

    async fn invoke(
        &self,
        handle: &RunnerHandle,
        id: &FunctionId,
        ex: &Exchange,
        timeout: std::time::Duration,
    ) -> Result<ExchangePatch, ProviderError> {
        let endpoint = self
            .containers_by_handle
            .get(&handle.id)
            .ok_or_else(|| ProviderError::InvokeFailed(format!("unknown handle {}", handle.id)))?
            .endpoint
            .clone();
        let resp = self.client.invoke(&endpoint, id, ex, timeout).await?;
        if resp.ok {
            let patch = resp.patch.unwrap_or_default();
            Ok(patch
                .to_exchange_patch()
                .map_err(|e| ProviderError::InvokeFailed(e.to_string()))?)
        } else {
            let err = resp.error.unwrap_or_else(|| crate::protocol::ErrorWire {
                kind: "unknown".into(),
                message: "no error body".into(),
                stack: None,
            });
            Err(ProviderError::InvokeFailed(format!(
                "{}: {}",
                err.kind, err.message
            )))
        }
    }
}

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
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                drop(handle.spawn(async move {
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
                }));
            }
            Err(_) => {
                tracing::warn!(target: "camel_function::container", "container cleanup skipped: no tokio runtime");
            }
        }
    }
}
