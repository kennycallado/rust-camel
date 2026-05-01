//! Camel Container Component
//!
//! This component provides integration with Docker containers, allowing Camel routes
//! to manage container lifecycle (create, start, stop, remove) and consume container events.

pub mod bundle;

pub use bundle::ContainerBundle;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use async_trait::async_trait;
use bollard::Docker;
use bollard::models::{
    ContainerCreateBody, NetworkConnectRequest, NetworkCreateRequest, NetworkDisconnectRequest,
};
use bollard::query_parameters::{
    CreateContainerOptions, CreateImageOptions, EventsOptions, ListContainersOptions,
    ListImagesOptions, ListNetworksOptions, LogsOptions, RemoveContainerOptions,
    StartContainerOptions,
};
use bollard::service::{HostConfig, PortBinding};
use camel_component_api::parse_uri;
use camel_component_api::{Body, BoxProcessor, CamelError, Exchange, Message};
use camel_component_api::{Component, Consumer, ConsumerContext, Endpoint, ProducerContext};
use tower::Service;

/// Global tracker for containers created by this component.
/// Used for cleanup on shutdown (especially important for hot-reload scenarios).
static CONTAINER_TRACKER: once_cell::sync::Lazy<Arc<Mutex<HashSet<String>>>> =
    once_cell::sync::Lazy::new(|| Arc::new(Mutex::new(HashSet::new())));

/// Registers a container ID for tracking (will be cleaned up on shutdown).
fn track_container(id: String) {
    if let Ok(mut tracker) = CONTAINER_TRACKER.lock() {
        tracker.insert(id);
    }
}

/// Removes a container ID from tracking (when it's been removed naturally).
fn untrack_container(id: &str) {
    if let Ok(mut tracker) = CONTAINER_TRACKER.lock() {
        tracker.remove(id);
    }
}

/// Cleans up all tracked containers. Call this on application shutdown.
pub async fn cleanup_tracked_containers() {
    let ids: Vec<String> = {
        match CONTAINER_TRACKER.lock() {
            Ok(tracker) => tracker.iter().cloned().collect(),
            Err(_) => return,
        }
    };

    if ids.is_empty() {
        return;
    }

    tracing::info!("Cleaning up {} tracked container(s)", ids.len());

    let docker = match Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("Failed to connect to Docker for cleanup: {}", e);
            return;
        }
    };

    for id in ids {
        match docker
            .remove_container(
                &id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
        {
            Ok(_) => {
                tracing::debug!("Cleaned up container {}", id);
                untrack_container(&id);
            }
            Err(e) => {
                tracing::warn!("Failed to cleanup container {}: {}", id, e);
            }
        }
    }
}

// Header constants for container operations

/// Timeout (seconds) for connecting to the Docker daemon.
const DOCKER_CONNECT_TIMEOUT_SECS: u64 = 120;

/// Header key for specifying the container action (e.g., "list", "run", "start", "stop", "remove").
pub const HEADER_ACTION: &str = "CamelContainerAction";

/// Header key for specifying the container image to use for "run" operations.
pub const HEADER_IMAGE: &str = "CamelContainerImage";

/// Header key for specifying or receiving the container ID.
pub const HEADER_CONTAINER_ID: &str = "CamelContainerId";

/// Header key for the log stream type (stdout or stderr).
pub const HEADER_LOG_STREAM: &str = "CamelContainerLogStream";

/// Header key for the log timestamp.
pub const HEADER_LOG_TIMESTAMP: &str = "CamelContainerLogTimestamp";

/// Header key for specifying the container name for "run" operations.
pub const HEADER_CONTAINER_NAME: &str = "CamelContainerName";

/// Header key for the result status of a container operation (e.g., "success").
pub const HEADER_ACTION_RESULT: &str = "CamelContainerActionResult";

/// Header key for specifying the command to execute in a container.
pub const HEADER_CMD: &str = "CamelContainerCmd";

/// Header key for specifying the network name for network operations.
pub const HEADER_NETWORK: &str = "CamelContainerNetwork";

/// Header key for the exit code of an exec operation.
pub const HEADER_EXIT_CODE: &str = "CamelContainerExitCode";

/// Header key for specifying volume mounts.
pub const HEADER_VOLUMES: &str = "CamelContainerVolumes";

/// Header key for the exec instance ID.
pub const HEADER_EXEC_ID: &str = "CamelContainerExecId";

// ---------------------------------------------------------------------------
// ContainerGlobalConfig
// ---------------------------------------------------------------------------

/// Global configuration for Container component.
/// Supports serde deserialization with defaults and builder methods.
/// These are the fallback defaults when URI params are not set.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ContainerGlobalConfig {
    /// The Docker host URL (default: "unix:///var/run/docker.sock").
    pub docker_host: String,
}

impl Default for ContainerGlobalConfig {
    fn default() -> Self {
        Self {
            docker_host: "unix:///var/run/docker.sock".to_string(),
        }
    }
}

impl ContainerGlobalConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_docker_host(mut self, v: impl Into<String>) -> Self {
        self.docker_host = v.into();
        self
    }
}

// ---------------------------------------------------------------------------
// ContainerConfig (endpoint configuration)
// ---------------------------------------------------------------------------

/// Configuration for the container component endpoint.
///
/// This struct holds the parsed URI configuration including the operation type,
/// optional container image, and Docker host connection details.
#[derive(Debug, Clone)]
pub struct ContainerConfig {
    /// The operation to perform (e.g., "list", "run", "start", "stop", "remove", "events").
    pub operation: String,
    /// The container image to use for "run" operations (can be overridden via header).
    pub image: Option<String>,
    /// The container name to use for "run" operations (can be overridden via header).
    pub name: Option<String>,
    /// The Docker host URL (defaults to "unix:///var/run/docker.sock").
    pub host: Option<String>,
    /// Command to run in the container (e.g., "sleep 30").
    pub cmd: Option<String>,
    /// Port mappings in format "hostPort:containerPort" (e.g., "8080:80,8443:443").
    pub ports: Option<String>,
    /// Environment variables in format "KEY=value,KEY2=value2".
    pub env: Option<String>,
    /// Network mode (e.g., "bridge", "host", "none"). Default: "bridge".
    pub network: Option<String>,
    /// Container ID or name for logs consumer.
    pub container_id: Option<String>,
    /// Follow log output (default: true for consumer).
    pub follow: bool,
    /// Include timestamps in logs (default: false).
    pub timestamps: bool,
    /// Number of lines to show from the end of logs (default: all).
    pub tail: Option<String>,
    /// Automatically pull the image if not present (default: true).
    pub auto_pull: bool,
    /// Automatically remove the container when it exits (default: true).
    pub auto_remove: bool,
    /// Volume mounts in format "host:container:ro" (e.g., "./html:/usr/share/nginx/html:ro").
    pub volumes: Option<String>,
    /// User to run the container or exec command as (e.g., "root").
    pub user: Option<String>,
    /// Working directory inside the container.
    pub workdir: Option<String>,
    /// Whether to detach from the exec process (default: false).
    pub detach: bool,
    /// Network driver for network-create (e.g., "bridge", "overlay").
    pub driver: Option<String>,
    /// Whether to force the operation (default: false).
    pub force: bool,
}

impl ContainerConfig {
    /// Parses a container URI into a `ContainerConfig`.
    ///
    /// # Arguments
    /// * `uri` - The URI to parse (e.g., "container:run?image=alpine")
    ///
    /// # Errors
    /// Returns an error if the URI scheme is not "container".
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;
        if parts.scheme != "container" {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme 'container', got '{}'",
                parts.scheme
            )));
        }

        let image = parts.params.get("image").cloned();
        let name = parts.params.get("name").cloned();
        let cmd = parts.params.get("cmd").cloned();
        let ports = parts.params.get("ports").cloned();
        let env = parts.params.get("env").cloned();
        let network = parts.params.get("network").cloned();
        let container_id = parts.params.get("containerId").cloned();
        let follow = parts
            .params
            .get("follow")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);
        let timestamps = parts
            .params
            .get("timestamps")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let tail = parts.params.get("tail").cloned();
        let auto_pull = parts
            .params
            .get("autoPull")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);
        let auto_remove = parts
            .params
            .get("autoRemove")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);
        // host is only set from URI param; global config defaults are applied later
        let host = parts.params.get("host").cloned();
        let volumes = parts.params.get("volumes").cloned();
        let user = parts.params.get("user").cloned();
        let workdir = parts.params.get("workdir").cloned();
        let detach = parts
            .params
            .get("detach")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let driver = parts.params.get("driver").cloned();
        let force = parts
            .params
            .get("force")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        Ok(Self {
            operation: parts.path,
            image,
            name,
            host,
            cmd,
            ports,
            env,
            network,
            container_id,
            follow,
            timestamps,
            tail,
            auto_pull,
            auto_remove,
            volumes,
            user,
            workdir,
            detach,
            driver,
            force,
        })
    }

    /// Apply global config defaults to this endpoint config.
    /// Only sets values that are currently `None`.
    fn apply_global_defaults(&mut self, global: &ContainerGlobalConfig) {
        if self.host.is_none() {
            self.host = Some(global.docker_host.clone());
        }
    }

    fn docker_socket_path(&self) -> Result<&str, CamelError> {
        let host = self.host.as_deref().unwrap_or(if cfg!(windows) {
            "npipe:////./pipe/docker_engine"
        } else {
            "unix:///var/run/docker.sock"
        });

        if host.starts_with("unix://") || host.starts_with("npipe://") {
            return Ok(host);
        }

        if host.contains("://") {
            return Err(CamelError::ProcessorError(format!(
                "Unsupported Docker host scheme: {} (only unix:// and npipe:// are supported)",
                host
            )));
        }

        Ok(host)
    }

    pub fn connect_docker_client(&self) -> Result<Docker, CamelError> {
        let socket_path = self.docker_socket_path()?;
        Docker::connect_with_socket(
            socket_path,
            DOCKER_CONNECT_TIMEOUT_SECS,
            bollard::API_DEFAULT_VERSION,
        )
        .map_err(|e| {
            CamelError::ProcessorError(format!("Failed to connect to docker daemon: {}", e))
        })
    }

    /// Connects to the Docker daemon using the configured host.
    ///
    /// This method establishes a Unix socket connection to Docker and verifies
    /// the connection by sending a ping request.
    ///
    /// # Errors
    /// Returns an error if the connection fails or the ping request fails.
    pub async fn connect_docker(&self) -> Result<Docker, CamelError> {
        let docker = self.connect_docker_client()?;
        docker
            .ping()
            .await
            .map_err(|e| CamelError::ProcessorError(format!("Docker ping failed: {}", e)))?;
        Ok(docker)
    }

    #[allow(clippy::type_complexity)]
    fn parse_ports(&self) -> Option<(Vec<String>, HashMap<String, Option<Vec<PortBinding>>>)> {
        let ports_str = self.ports.as_ref()?;

        let mut exposed_ports: Vec<String> = Vec::new();
        let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();

        for mapping in ports_str.split(',') {
            let mapping = mapping.trim();
            if mapping.is_empty() {
                continue;
            }

            let (host_port, container_spec) = mapping.split_once(':')?;

            let (container_port, protocol) = if container_spec.contains('/') {
                let parts: Vec<&str> = container_spec.split('/').collect();
                (parts[0], parts[1])
            } else {
                (container_spec, "tcp")
            };

            let container_key = format!("{}/{}", container_port, protocol);

            exposed_ports.push(container_key.clone());

            port_bindings.insert(
                container_key,
                Some(vec![PortBinding {
                    host_ip: None,
                    host_port: Some(host_port.to_string()),
                }]),
            );
        }

        if exposed_ports.is_empty() {
            None
        } else {
            Some((exposed_ports, port_bindings))
        }
    }

    fn parse_env(&self) -> Option<Vec<String>> {
        let env_str = self.env.as_ref()?;

        let env_vars: Vec<String> = env_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if env_vars.is_empty() {
            None
        } else {
            Some(env_vars)
        }
    }

    #[cfg(test)]
    #[allow(clippy::type_complexity)]
    fn parse_volumes(&self) -> Option<(Vec<String>, Vec<String>)> {
        self.volumes.as_deref().and_then(parse_volume_str)
    }
}

/// Parses a volume specification string into bind mounts and anonymous volumes.
///
/// Format: `host:container:ro|rw` for bind mounts, `path` for anonymous volumes,
/// `path:ro|rw` for anonymous volumes with mode, or `name:container` for named volumes.
#[allow(clippy::type_complexity)]
fn parse_volume_str(volumes_str: &str) -> Option<(Vec<String>, Vec<String>)> {
    let mut binds: Vec<String> = Vec::new();
    let mut anonymous_volumes: Vec<String> = Vec::new();

    for entry in volumes_str.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }

        let segments: Vec<&str> = entry.split(':').collect();

        match segments.len() {
            3 => {
                let source = segments[0];
                let target = segments[1];
                let mode = segments[2];
                if mode != "ro" && mode != "rw" {
                    continue;
                }
                binds.push(format!("{}:{}:{}", source, target, mode));
            }
            2 => {
                let a = segments[0];
                let b = segments[1];
                if b == "ro" || b == "rw" {
                    anonymous_volumes.push(a.to_string());
                } else {
                    binds.push(format!("{}:{}", a, b));
                }
            }
            1 => {
                anonymous_volumes.push(segments[0].to_string());
            }
            _ => continue,
        }
    }

    if binds.is_empty() && anonymous_volumes.is_empty() {
        None
    } else {
        Some((binds, anonymous_volumes))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProducerOperation {
    List,
    Run,
    Start,
    Stop,
    Remove,
    Exec,
    NetworkCreate,
    NetworkConnect,
    NetworkDisconnect,
    NetworkRemove,
    NetworkList,
}

fn parse_producer_operation(operation: &str) -> Result<ProducerOperation, CamelError> {
    match operation {
        "list" => Ok(ProducerOperation::List),
        "run" => Ok(ProducerOperation::Run),
        "start" => Ok(ProducerOperation::Start),
        "stop" => Ok(ProducerOperation::Stop),
        "remove" => Ok(ProducerOperation::Remove),
        "exec" => Ok(ProducerOperation::Exec),
        "network-create" => Ok(ProducerOperation::NetworkCreate),
        "network-connect" => Ok(ProducerOperation::NetworkConnect),
        "network-disconnect" => Ok(ProducerOperation::NetworkDisconnect),
        "network-remove" => Ok(ProducerOperation::NetworkRemove),
        "network-list" => Ok(ProducerOperation::NetworkList),
        _ => Err(CamelError::ProcessorError(format!(
            "Unknown container operation: {}",
            operation
        ))),
    }
}

fn resolve_container_name(exchange: &Exchange, config: &ContainerConfig) -> Option<String> {
    exchange
        .input
        .header(HEADER_CONTAINER_NAME)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .or_else(|| config.name.clone())
}

async fn image_exists_locally(docker: &Docker, image: &str) -> Result<bool, CamelError> {
    let images = docker
        .list_images(None::<ListImagesOptions>)
        .await
        .map_err(|e| CamelError::ProcessorError(format!("Failed to list images: {}", e)))?;

    Ok(images.iter().any(|img| {
        img.repo_tags
            .iter()
            .any(|tag| tag == image || tag.starts_with(&format!("{}:", image)))
    }))
}

async fn pull_image_with_progress(
    docker: &Docker,
    image: &str,
    timeout_secs: u64,
) -> Result<(), CamelError> {
    use futures::StreamExt;

    tracing::info!("Pulling image: {}", image);

    let mut stream = docker.create_image(
        Some(CreateImageOptions {
            from_image: Some(image.to_string()),
            ..Default::default()
        }),
        None,
        None,
    );

    let start = std::time::Instant::now();
    let mut last_progress = std::time::Instant::now();

    while let Some(item) = stream.next().await {
        if start.elapsed().as_secs() > timeout_secs {
            return Err(CamelError::ProcessorError(format!(
                "Image pull timeout after {}s. Try manually: docker pull {}",
                timeout_secs, image
            )));
        }

        match item {
            Ok(update) => {
                // Log progress every 2 seconds
                if last_progress.elapsed().as_secs() >= 2 {
                    if let Some(status) = update.status {
                        tracing::debug!("Pull progress: {}", status);
                    }
                    last_progress = std::time::Instant::now();
                }
            }
            Err(e) => {
                let err_str = e.to_string().to_lowercase();
                if err_str.contains("unauthorized") || err_str.contains("401") {
                    return Err(CamelError::ProcessorError(format!(
                        "Authentication required for image '{}'. Configure Docker credentials: docker login",
                        image
                    )));
                }
                if err_str.contains("not found") || err_str.contains("404") {
                    return Err(CamelError::ProcessorError(format!(
                        "Image '{}' not found in registry. Check the image name and tag",
                        image
                    )));
                }
                return Err(CamelError::ProcessorError(format!(
                    "Failed to pull image '{}': {}",
                    image, e
                )));
            }
        }
    }

    tracing::info!("Successfully pulled image: {}", image);
    Ok(())
}

async fn ensure_image_available(
    docker: &Docker,
    image: &str,
    auto_pull: bool,
    timeout_secs: u64,
) -> Result<(), CamelError> {
    if image_exists_locally(docker, image).await? {
        tracing::debug!("Image '{}' already available locally", image);
        return Ok(());
    }

    if !auto_pull {
        return Err(CamelError::ProcessorError(format!(
            "Image '{}' not found locally. Set autoPull=true to pull automatically, or run: docker pull {}",
            image, image
        )));
    }

    pull_image_with_progress(docker, image, timeout_secs).await
}

fn format_docker_event(event: &bollard::models::EventMessage) -> String {
    let action = event.action.as_deref().unwrap_or("unknown");
    let actor = event.actor.as_ref();

    let container_name = actor
        .and_then(|a| a.attributes.as_ref())
        .and_then(|attrs| attrs.get("name"))
        .map(|s| s.as_str())
        .unwrap_or("unknown");

    let image = actor
        .and_then(|a| a.attributes.as_ref())
        .and_then(|attrs| attrs.get("image"))
        .map(|s| s.as_str())
        .unwrap_or("");

    let exit_code = actor
        .and_then(|a| a.attributes.as_ref())
        .and_then(|attrs| attrs.get("exitCode"))
        .map(|s| s.as_str());

    match action {
        "create" => {
            if image.is_empty() {
                format!("[CREATE] Container {}", container_name)
            } else {
                format!("[CREATE] Container {} ({})", container_name, image)
            }
        }
        "start" => format!("[START]  Container {}", container_name),
        "die" => {
            if let Some(code) = exit_code {
                format!("[DIE]    Container {} (exit: {})", container_name, code)
            } else {
                format!("[DIE]    Container {}", container_name)
            }
        }
        "destroy" => format!("[DESTROY] Container {}", container_name),
        "stop" => format!("[STOP]   Container {}", container_name),
        "pause" => format!("[PAUSE]  Container {}", container_name),
        "unpause" => format!("[UNPAUSE] Container {}", container_name),
        "restart" => format!("[RESTART] Container {}", container_name),
        _ => format!("[{}] Container {}", action.to_uppercase(), container_name),
    }
}

async fn run_container_with_cleanup<CreateFn, CreateFut, StartFn, StartFut, RemoveFn, RemoveFut>(
    create: CreateFn,
    start: StartFn,
    remove: RemoveFn,
) -> Result<String, CamelError>
where
    CreateFn: FnOnce() -> CreateFut,
    CreateFut: Future<Output = Result<String, CamelError>>,
    StartFn: FnOnce(String) -> StartFut,
    StartFut: Future<Output = Result<(), CamelError>>,
    RemoveFn: FnOnce(String) -> RemoveFut,
    RemoveFut: Future<Output = Result<(), CamelError>>,
{
    let container_id = create().await?;
    if let Err(start_err) = start(container_id.clone()).await {
        if let Err(remove_err) = remove(container_id.clone()).await {
            return Err(CamelError::ProcessorError(format!(
                "Failed to start container: {}. Cleanup failed: {}",
                start_err, remove_err
            )));
        }
        return Err(start_err);
    }

    Ok(container_id)
}

async fn handle_list(
    docker: Docker,
    _config: ContainerConfig,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    let containers = docker
        .list_containers(None::<ListContainersOptions>)
        .await
        .map_err(|e| CamelError::ProcessorError(format!("Failed to list containers: {}", e)))?;

    let json_value = serde_json::to_value(&containers).map_err(|e| {
        CamelError::ProcessorError(format!("Failed to serialize containers: {}", e))
    })?;

    exchange.input.body = Body::Json(json_value);
    exchange.input.set_header(
        HEADER_ACTION_RESULT,
        serde_json::Value::String("success".to_string()),
    );
    Ok(())
}

async fn handle_run(
    docker: Docker,
    config: ContainerConfig,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    let image = exchange
        .input
        .header(HEADER_IMAGE)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .or(config.image.clone())
        .ok_or_else(|| {
            CamelError::ProcessorError(
                "missing image for run operation. Specify in URI (image=alpine) or header (CamelContainerImage)".to_string(),
            )
        })?;

    let image = if !image.contains(':') && !image.contains('@') {
        format!("{}:latest", image)
    } else {
        image
    };

    let pull_timeout = 300;
    ensure_image_available(&docker, &image, config.auto_pull, pull_timeout)
        .await
        .map_err(|e| {
            CamelError::ProcessorError(format!("Image '{}' not available: {}", image, e))
        })?;

    let container_name = resolve_container_name(exchange, &config);
    let container_name_ref = container_name.as_deref().unwrap_or("");
    let cmd_parts: Option<Vec<String>> = config
        .cmd
        .as_ref()
        .map(|c| c.split_whitespace().map(|s| s.to_string()).collect());
    let auto_remove = config.auto_remove;
    let (exposed_ports, port_bindings) = config.parse_ports().unwrap_or_default();
    let env_vars = config.parse_env();
    let network_mode = config.network.clone();

    let volumes_str = exchange
        .input
        .header(HEADER_VOLUMES)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .or(config.volumes.clone());
    let (binds, anon_volumes) = volumes_str
        .as_deref()
        .and_then(parse_volume_str)
        .unwrap_or_default();

    let docker_create = docker.clone();
    let docker_start = docker.clone();
    let docker_remove = docker.clone();

    let container_id = run_container_with_cleanup(
        move || async move {
            let create_options = CreateContainerOptions {
                name: Some(container_name_ref.to_string()),
                ..Default::default()
            };
            let container_config = ContainerCreateBody {
                image: Some(image.clone()),
                cmd: cmd_parts,
                env: env_vars,
                exposed_ports: if exposed_ports.is_empty() { None } else { Some(exposed_ports) },
                volumes: if anon_volumes.is_empty() { None } else { Some(anon_volumes) },
                host_config: Some(HostConfig {
                    auto_remove: Some(auto_remove),
                    port_bindings: if port_bindings.is_empty() { None } else { Some(port_bindings) },
                    network_mode,
                    binds: if binds.is_empty() { None } else { Some(binds) },
                    ..Default::default()
                }),
                ..Default::default()
            };

            let create_response = docker_create
                .create_container(Some(create_options), container_config)
                .await
                .map_err(|e| {
                    let err_str = e.to_string().to_lowercase();
                    if err_str.contains("409") || err_str.contains("conflict") {
                        CamelError::ProcessorError(format!(
                            "Container name '{}' already exists. Use a unique name or remove the existing container first",
                            container_name_ref
                        ))
                    } else {
                        CamelError::ProcessorError(format!(
                            "Failed to create container: {}",
                            e
                        ))
                    }
                })?;

            Ok(create_response.id)
        },
        move |container_id| async move {
            docker_start
                .start_container(&container_id, None::<StartContainerOptions>)
                .await
                .map_err(|e| {
                    CamelError::ProcessorError(format!(
                        "Failed to start container: {}",
                        e
                    ))
                })
        },
        move |container_id| async move {
            docker_remove
                .remove_container(&container_id, None)
                .await
                .map_err(|e| {
                    CamelError::ProcessorError(format!(
                        "Failed to remove container after start failure: {}",
                        e
                    ))
                })
        },
    )
    .await?;

    track_container(container_id.clone());

    exchange
        .input
        .set_header(HEADER_CONTAINER_ID, serde_json::Value::String(container_id));
    exchange.input.set_header(
        HEADER_ACTION_RESULT,
        serde_json::Value::String("success".to_string()),
    );
    Ok(())
}

async fn handle_lifecycle(
    docker: Docker,
    _config: ContainerConfig,
    exchange: &mut Exchange,
    operation: ProducerOperation,
    operation_name: &str,
) -> Result<(), CamelError> {
    let container_id = exchange
        .input
        .header(HEADER_CONTAINER_ID)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .ok_or_else(|| {
            CamelError::ProcessorError(format!(
                "{} header is required for {} operation",
                HEADER_CONTAINER_ID, operation_name
            ))
        })?;

    match operation {
        ProducerOperation::Start => {
            docker
                .start_container(&container_id, None::<StartContainerOptions>)
                .await
                .map_err(|e| {
                    CamelError::ProcessorError(format!("Failed to start container: {}", e))
                })?;
        }
        ProducerOperation::Stop => {
            docker
                .stop_container(&container_id, None)
                .await
                .map_err(|e| {
                    CamelError::ProcessorError(format!("Failed to stop container: {}", e))
                })?;
        }
        ProducerOperation::Remove => {
            docker
                .remove_container(&container_id, None)
                .await
                .map_err(|e| {
                    CamelError::ProcessorError(format!("Failed to remove container: {}", e))
                })?;
            untrack_container(&container_id);
        }
        _ => {}
    }

    exchange.input.set_header(
        HEADER_ACTION_RESULT,
        serde_json::Value::String("success".to_string()),
    );
    Ok(())
}

async fn handle_exec(
    docker: Docker,
    config: ContainerConfig,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    let container_id = exchange
        .input
        .header(HEADER_CONTAINER_ID)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .or(config.container_id.clone())
        .ok_or_else(|| {
            CamelError::ProcessorError(format!(
                "{} header or containerId param is required for exec operation",
                HEADER_CONTAINER_ID
            ))
        })?;

    let cmd = exchange
        .input
        .header(HEADER_CMD)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .or(config.cmd.clone())
        .ok_or_else(|| {
            CamelError::ProcessorError(
                "CamelContainerCmd header or cmd param is required for exec operation".to_string(),
            )
        })?;

    let cmd_parts: Vec<String> = cmd.split_whitespace().map(|s| s.to_string()).collect();
    let env_vars = config.parse_env();

    let exec_config = bollard::exec::CreateExecOptions {
        cmd: Some(cmd_parts),
        env: env_vars,
        user: config.user.clone(),
        working_dir: config.workdir.clone(),
        attach_stdout: Some(true),
        attach_stderr: Some(true),
        ..Default::default()
    };

    let create_result = docker
        .create_exec(&container_id, exec_config)
        .await
        .map_err(|e| {
            let err_str = e.to_string().to_lowercase();
            if err_str.contains("404") || err_str.contains("no such") {
                CamelError::ProcessorError(format!(
                    "Container '{}' not found for exec",
                    container_id
                ))
            } else {
                CamelError::ProcessorError(format!("Failed to create exec: {}", e))
            }
        })?;

    let exec_id = create_result.id;

    if config.detach {
        docker
            .start_exec(
                &exec_id,
                Some(bollard::exec::StartExecOptions {
                    detach: true,
                    ..Default::default()
                }),
            )
            .await
            .map_err(|e| {
                CamelError::ProcessorError(format!("Failed to start exec (detached): {}", e))
            })?;

        exchange
            .input
            .set_header(HEADER_EXEC_ID, serde_json::Value::String(exec_id));
        exchange
            .input
            .set_header(HEADER_CONTAINER_ID, serde_json::Value::String(container_id));
    } else {
        let start_result = docker
            .start_exec(&exec_id, None)
            .await
            .map_err(|e| CamelError::ProcessorError(format!("Failed to start exec: {}", e)))?;

        let mut output = String::new();

        match start_result {
            bollard::exec::StartExecResults::Attached {
                output: mut stream, ..
            } => {
                use futures::StreamExt;
                while let Some(msg) = stream.next().await {
                    match msg {
                        Ok(bollard::container::LogOutput::StdOut { message }) => {
                            output.push_str(&String::from_utf8_lossy(&message));
                        }
                        Ok(bollard::container::LogOutput::StdErr { message }) => {
                            output.push_str(&String::from_utf8_lossy(&message));
                        }
                        Ok(_) => {}
                        Err(e) => {
                            output.push_str(&format!("[error reading stream: {}]", e));
                        }
                    }
                }
            }
            bollard::exec::StartExecResults::Detached => {}
        }

        let inspect = docker
            .inspect_exec(&exec_id)
            .await
            .map_err(|e| CamelError::ProcessorError(format!("Failed to inspect exec: {}", e)))?;

        let exit_code: i64 = inspect.exit_code.unwrap_or(0);

        let output = output.trim_end().to_string();
        exchange.input.body = Body::Text(output);
        exchange.input.set_header(
            HEADER_EXIT_CODE,
            serde_json::Value::Number(exit_code.into()),
        );
        exchange
            .input
            .set_header(HEADER_CONTAINER_ID, serde_json::Value::String(container_id));
    }

    exchange.input.set_header(
        HEADER_ACTION_RESULT,
        serde_json::Value::String("success".to_string()),
    );
    Ok(())
}

async fn handle_network_create(
    docker: Docker,
    config: ContainerConfig,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    let network_name = exchange
        .input
        .header(HEADER_CONTAINER_NAME)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .or(config.name.clone())
        .ok_or_else(|| {
            CamelError::ProcessorError(
                "CamelContainerName header or name param is required for network-create"
                    .to_string(),
            )
        })?;

    let driver = config.driver.as_deref().unwrap_or("bridge");

    let options = NetworkCreateRequest {
        name: network_name.clone(),
        driver: Some(driver.to_string()),
        ..Default::default()
    };

    let result = docker.create_network(options).await.map_err(|e| {
        let err_str = e.to_string().to_lowercase();
        if err_str.contains("409") || err_str.contains("already exists") {
            CamelError::ProcessorError(format!("Network '{}' already exists", network_name))
        } else {
            CamelError::ProcessorError(format!("Failed to create network: {}", e))
        }
    })?;

    let network_id = result.id.clone();
    let json_value = serde_json::to_value(&result).map_err(|e| {
        CamelError::ProcessorError(format!("Failed to serialize network response: {}", e))
    })?;

    exchange.input.body = Body::Json(json_value);
    exchange
        .input
        .set_header(HEADER_NETWORK, serde_json::Value::String(network_id));
    exchange.input.set_header(
        HEADER_ACTION_RESULT,
        serde_json::Value::String("success".to_string()),
    );
    Ok(())
}

async fn handle_network_connect(
    docker: Docker,
    config: ContainerConfig,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    let network = exchange
        .input
        .header(HEADER_NETWORK)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .or(config.network.clone())
        .ok_or_else(|| {
            CamelError::ProcessorError(
                "CamelContainerNetwork header or network param is required for network-connect"
                    .to_string(),
            )
        })?;

    let container = exchange
        .input
        .header(HEADER_CONTAINER_ID)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .or(config.container_id.clone())
        .ok_or_else(|| {
            CamelError::ProcessorError(
                "CamelContainerId header or container param is required for network-connect"
                    .to_string(),
            )
        })?;

    docker
        .connect_network(
            &network,
            NetworkConnectRequest {
                container,
                ..Default::default()
            },
        )
        .await
        .map_err(|e| {
            let err_str = e.to_string().to_lowercase();
            if err_str.contains("404") || err_str.contains("not found") {
                CamelError::ProcessorError(format!("Network '{}' or container not found", network))
            } else {
                CamelError::ProcessorError(format!("Failed to connect to network: {}", e))
            }
        })?;

    exchange.input.set_header(
        HEADER_ACTION_RESULT,
        serde_json::Value::String("success".to_string()),
    );
    Ok(())
}

async fn handle_network_disconnect(
    docker: Docker,
    config: ContainerConfig,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    let network = exchange
        .input
        .header(HEADER_NETWORK)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .or(config.network.clone())
        .ok_or_else(|| {
            CamelError::ProcessorError(
                "CamelContainerNetwork header or network param is required for network-disconnect"
                    .to_string(),
            )
        })?;

    let container = exchange
        .input
        .header(HEADER_CONTAINER_ID)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .or(config.container_id.clone())
        .ok_or_else(|| {
            CamelError::ProcessorError(
                "CamelContainerId header or container param is required for network-disconnect"
                    .to_string(),
            )
        })?;

    docker
        .disconnect_network(
            &network,
            NetworkDisconnectRequest {
                container,
                force: Some(config.force),
            },
        )
        .await
        .map_err(|e| {
            let err_str = e.to_string().to_lowercase();
            if err_str.contains("404") || err_str.contains("not found") {
                CamelError::ProcessorError(format!("Network '{}' or container not found", network))
            } else {
                CamelError::ProcessorError(format!("Failed to disconnect from network: {}", e))
            }
        })?;

    exchange.input.set_header(
        HEADER_ACTION_RESULT,
        serde_json::Value::String("success".to_string()),
    );
    Ok(())
}

async fn handle_network_remove(
    docker: Docker,
    config: ContainerConfig,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    let network = exchange
        .input
        .header(HEADER_NETWORK)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .or(config.network.clone())
        .ok_or_else(|| {
            CamelError::ProcessorError(
                "CamelContainerNetwork header or network param is required for network-remove"
                    .to_string(),
            )
        })?;

    docker.remove_network(&network).await.map_err(|e| {
        let err_str = e.to_string().to_lowercase();
        if err_str.contains("404") || err_str.contains("not found") {
            CamelError::ProcessorError(format!("Network '{}' not found", network))
        } else if err_str.contains("409") || err_str.contains("in use") {
            CamelError::ProcessorError(format!(
                "Network '{}' is in use and cannot be removed",
                network
            ))
        } else {
            CamelError::ProcessorError(format!("Failed to remove network: {}", e))
        }
    })?;

    exchange.input.set_header(
        HEADER_ACTION_RESULT,
        serde_json::Value::String("success".to_string()),
    );
    Ok(())
}

async fn handle_network_list(
    docker: Docker,
    _config: ContainerConfig,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    let networks = docker
        .list_networks(None::<ListNetworksOptions>)
        .await
        .map_err(|e| CamelError::ProcessorError(format!("Failed to list networks: {}", e)))?;

    let json_value = serde_json::to_value(&networks)
        .map_err(|e| CamelError::ProcessorError(format!("Failed to serialize networks: {}", e)))?;

    exchange.input.body = Body::Json(json_value);
    exchange.input.set_header(
        HEADER_ACTION_RESULT,
        serde_json::Value::String("success".to_string()),
    );
    Ok(())
}

/// Producer for executing container operations.
///
/// This producer handles synchronous container operations like listing,
/// creating, starting, stopping, and removing containers.
#[derive(Clone)]
pub struct ContainerProducer {
    config: ContainerConfig,
    docker: Docker,
}

impl Service<Exchange> for ContainerProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let config = self.config.clone();
        let docker = self.docker.clone();
        Box::pin(async move {
            let operation_name = exchange
                .input
                .header(HEADER_ACTION)
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| config.operation.clone());

            let operation = parse_producer_operation(&operation_name)?;

            match operation {
                ProducerOperation::List => {
                    handle_list(docker, config, &mut exchange).await?;
                }
                ProducerOperation::Run => {
                    handle_run(docker, config, &mut exchange).await?;
                }
                ProducerOperation::Start => {
                    handle_lifecycle(docker, config, &mut exchange, operation, &operation_name)
                        .await?;
                }
                ProducerOperation::Stop => {
                    handle_lifecycle(docker, config, &mut exchange, operation, &operation_name)
                        .await?;
                }
                ProducerOperation::Remove => {
                    handle_lifecycle(docker, config, &mut exchange, operation, &operation_name)
                        .await?;
                }
                ProducerOperation::Exec => {
                    handle_exec(docker, config, &mut exchange).await?;
                }
                ProducerOperation::NetworkCreate => {
                    handle_network_create(docker, config, &mut exchange).await?;
                }
                ProducerOperation::NetworkConnect => {
                    handle_network_connect(docker, config, &mut exchange).await?;
                }
                ProducerOperation::NetworkDisconnect => {
                    handle_network_disconnect(docker, config, &mut exchange).await?;
                }
                ProducerOperation::NetworkRemove => {
                    handle_network_remove(docker, config, &mut exchange).await?;
                }
                ProducerOperation::NetworkList => {
                    handle_network_list(docker, config, &mut exchange).await?;
                }
            }

            Ok(exchange)
        })
    }
}

/// Consumer for receiving Docker container events or logs.
///
/// This consumer subscribes to Docker events or container logs and forwards them
/// to the route as exchanges. It implements automatic reconnection on connection failures.
pub struct ContainerConsumer {
    config: ContainerConfig,
}

#[async_trait]
impl Consumer for ContainerConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        match self.config.operation.as_str() {
            "events" => self.start_events_consumer(context).await,
            "logs" => self.start_logs_consumer(context).await,
            _ => Err(CamelError::EndpointCreationFailed(format!(
                "Consumer only supports 'events' or 'logs' operations, got '{}'",
                self.config.operation
            ))),
        }
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn concurrency_model(&self) -> camel_component_api::ConcurrencyModel {
        camel_component_api::ConcurrencyModel::Concurrent { max: None }
    }
}

impl ContainerConsumer {
    async fn start_events_consumer(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        use futures::StreamExt;

        loop {
            if context.is_cancelled() {
                tracing::info!("Container events consumer shutting down");
                return Ok(());
            }

            let docker = match self.config.connect_docker().await {
                Ok(d) => d,
                Err(e) => {
                    tracing::error!(
                        "Consumer failed to connect to docker: {}. Retrying in 5s...",
                        e
                    );
                    tokio::select! {
                        _ = context.cancelled() => {
                            tracing::info!("Container events consumer shutting down");
                            return Ok(());
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                    }
                    continue;
                }
            };

            let mut event_stream = docker.events(None::<EventsOptions>);

            loop {
                tokio::select! {
                    _ = context.cancelled() => {
                        tracing::info!("Container events consumer shutting down");
                        return Ok(());
                    }

                    msg = event_stream.next() => {
                        match msg {
                            Some(Ok(event)) => {
                                let formatted = format_docker_event(&event);
                                let message = Message::new(Body::Text(formatted));
                                let exchange = Exchange::new(message);

                                if let Err(e) = context.send(exchange).await {
                                    tracing::error!("Failed to send exchange: {:?}", e);
                                    break;
                                }
                            }
                            Some(Err(e)) => {
                                tracing::error!("Docker event stream error: {}. Reconnecting...", e);
                                break;
                            }
                            None => {
                                tracing::info!("Docker event stream ended. Reconnecting...");
                                break;
                            }
                        }
                    }
                }
            }

            tokio::select! {
                _ = context.cancelled() => {
                    tracing::info!("Container events consumer shutting down");
                    return Ok(());
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
            }
        }
    }

    async fn start_logs_consumer(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        use futures::StreamExt;

        let container_id = self.config.container_id.clone().ok_or_else(|| {
            CamelError::EndpointCreationFailed(
                "containerId is required for logs consumer. Use container:logs?containerId=xxx"
                    .to_string(),
            )
        })?;

        loop {
            if context.is_cancelled() {
                tracing::info!("Container logs consumer shutting down");
                return Ok(());
            }

            let docker = match self.config.connect_docker().await {
                Ok(d) => d,
                Err(e) => {
                    tracing::error!(
                        "Logs consumer failed to connect to docker: {}. Retrying in 5s...",
                        e
                    );
                    tokio::select! {
                        _ = context.cancelled() => {
                            tracing::info!("Container logs consumer shutting down");
                            return Ok(());
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                    }
                    continue;
                }
            };

            let tail = self
                .config
                .tail
                .clone()
                .unwrap_or_else(|| "all".to_string());

            let options = LogsOptions {
                follow: self.config.follow,
                stdout: true,
                stderr: true,
                timestamps: self.config.timestamps,
                tail,
                ..Default::default()
            };

            let mut log_stream = docker.logs(&container_id, Some(options));
            let container_id_header = container_id.clone();

            loop {
                tokio::select! {
                    _ = context.cancelled() => {
                        tracing::info!("Container logs consumer shutting down");
                        return Ok(());
                    }

                    msg = log_stream.next() => {
                        match msg {
                            Some(Ok(log_output)) => {
                                let (stream_type, content) = match log_output {
                                    bollard::container::LogOutput::StdOut { message } => {
                                        ("stdout", String::from_utf8_lossy(&message).into_owned())
                                    }
                                    bollard::container::LogOutput::StdErr { message } => {
                                        ("stderr", String::from_utf8_lossy(&message).into_owned())
                                    }
                                    bollard::container::LogOutput::Console { message } => {
                                        ("console", String::from_utf8_lossy(&message).into_owned())
                                    }
                                    bollard::container::LogOutput::StdIn { message } => {
                                        ("stdin", String::from_utf8_lossy(&message).into_owned())
                                    }
                                };

                                let content = content.trim_end();
                                if content.is_empty() {
                                    continue;
                                }

                                let mut message = Message::new(Body::Text(content.to_string()));
                                message.set_header(
                                    HEADER_CONTAINER_ID,
                                    serde_json::Value::String(container_id_header.clone()),
                                );
                                message.set_header(
                                    HEADER_LOG_STREAM,
                                    serde_json::Value::String(stream_type.to_string()),
                                );

                                if self.config.timestamps
                                    && let Some(ts) = extract_timestamp(content) {
                                        message.set_header(
                                            HEADER_LOG_TIMESTAMP,
                                            serde_json::Value::String(ts),
                                        );
                                    }

                                let exchange = Exchange::new(message);

                                if let Err(e) = context.send(exchange).await {
                                    tracing::error!("Failed to send log exchange: {:?}", e);
                                    break;
                                }
                            }
                            Some(Err(e)) => {
                                tracing::error!("Docker log stream error: {}. Reconnecting...", e);
                                break;
                            }
                            None => {
                                if self.config.follow {
                                    tracing::info!("Docker log stream ended. Reconnecting...");
                                    break;
                                } else {
                                    tracing::info!("Container logs consumer finished (follow=false)");
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
            }

            tokio::select! {
                _ = context.cancelled() => {
                    tracing::info!("Container logs consumer shutting down");
                    return Ok(());
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
            }
        }
    }
}

fn extract_timestamp(log_line: &str) -> Option<String> {
    let parts: Vec<&str> = log_line.splitn(2, ' ').collect();
    if parts.len() > 1 && parts[0].contains('T') {
        Some(parts[0].to_string())
    } else {
        None
    }
}

/// Component for creating container endpoints.
///
/// This component handles URIs with the "container" scheme and creates
/// appropriate producer and consumer endpoints for Docker operations.
///
/// Containers created via `run` operation are tracked globally and can be
/// cleaned up on shutdown by calling `cleanup_tracked_containers()`.
pub struct ContainerComponent {
    config: Option<ContainerGlobalConfig>,
}

impl ContainerComponent {
    /// Creates a new container component instance without global config.
    pub fn new() -> Self {
        Self { config: None }
    }

    /// Creates a container component with the given global config.
    pub fn with_config(config: ContainerGlobalConfig) -> Self {
        Self {
            config: Some(config),
        }
    }

    /// Creates a container component with optional global config.
    pub fn with_optional_config(config: Option<ContainerGlobalConfig>) -> Self {
        Self { config }
    }
}

impl Default for ContainerComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for ContainerComponent {
    fn scheme(&self) -> &str {
        "container"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let mut config = ContainerConfig::from_uri(uri)?;
        // Apply global defaults if present and URI didn't set them
        if let Some(ref global) = self.config {
            config.apply_global_defaults(global);
        }
        Ok(Box::new(ContainerEndpoint {
            uri: uri.to_string(),
            config,
        }))
    }
}

/// Endpoint for container operations.
///
/// This endpoint creates producers for executing container operations
/// and consumers for receiving container events.
pub struct ContainerEndpoint {
    uri: String,
    config: ContainerConfig,
}

impl ContainerEndpoint {
    /// Returns the Docker host configured for this endpoint.
    /// Returns `None` if not set (for testing purposes).
    pub fn docker_host(&self) -> Option<&str> {
        self.config.host.as_deref()
    }
}

impl Endpoint for ContainerEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(ContainerConsumer {
            config: self.config.clone(),
        }))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        let docker = self.config.connect_docker_client()?;
        Ok(BoxProcessor::new(ContainerProducer {
            config: self.config.clone(),
            docker,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::NoOpComponentContext;

    #[test]
    fn test_container_config() {
        let config = ContainerConfig::from_uri("container:run?image=alpine").unwrap();
        assert_eq!(config.operation, "run");
        assert_eq!(config.image.as_deref(), Some("alpine"));
        // host is None by default; global config applies it later
        assert!(config.host.is_none());
    }

    #[test]
    fn test_global_config_applied_to_endpoint() {
        // When global config is set and URI doesn't specify host,
        // apply_global_defaults should set host from global config.
        let global =
            ContainerGlobalConfig::default().with_docker_host("unix:///custom/docker.sock");
        let mut config = ContainerConfig::from_uri("container:run?image=alpine").unwrap();
        assert!(
            config.host.is_none(),
            "URI without ?host= should leave host as None"
        );
        config.apply_global_defaults(&global);
        assert_eq!(
            config.host.as_deref(),
            Some("unix:///custom/docker.sock"),
            "global docker_host must be applied when URI did not set host"
        );
    }

    #[test]
    fn test_uri_param_wins_over_global_config() {
        // When URI explicitly sets host param, apply_global_defaults must NOT override it.
        let global =
            ContainerGlobalConfig::default().with_docker_host("unix:///custom/docker.sock");
        let mut config =
            ContainerConfig::from_uri("container:run?image=alpine&host=unix:///override.sock")
                .unwrap();
        assert_eq!(
            config.host.as_deref(),
            Some("unix:///override.sock"),
            "URI-set host should be parsed correctly"
        );
        config.apply_global_defaults(&global);
        assert_eq!(
            config.host.as_deref(),
            Some("unix:///override.sock"),
            "global config must NOT override a host already set by URI"
        );
    }

    #[test]
    fn test_container_config_parses_name() {
        let config = ContainerConfig::from_uri("container:run?name=my-container").unwrap();
        assert_eq!(config.name.as_deref(), Some("my-container"));
    }

    #[test]
    fn test_parse_producer_operation_known() {
        assert_eq!(
            parse_producer_operation("list").unwrap(),
            ProducerOperation::List
        );
        assert_eq!(
            parse_producer_operation("run").unwrap(),
            ProducerOperation::Run
        );
        assert_eq!(
            parse_producer_operation("start").unwrap(),
            ProducerOperation::Start
        );
        assert_eq!(
            parse_producer_operation("stop").unwrap(),
            ProducerOperation::Stop
        );
        assert_eq!(
            parse_producer_operation("remove").unwrap(),
            ProducerOperation::Remove
        );
    }

    #[test]
    fn test_parse_producer_operation_unknown() {
        let err = parse_producer_operation("destruir_mundo").unwrap_err();
        match err {
            CamelError::ProcessorError(msg) => {
                assert!(
                    msg.contains("Unknown container operation"),
                    "Unexpected error message: {}",
                    msg
                );
            }
            _ => panic!("Expected ProcessorError for unknown operation"),
        }
    }

    #[test]
    fn test_parse_producer_operation_new_variants() {
        assert_eq!(
            parse_producer_operation("exec").unwrap(),
            ProducerOperation::Exec
        );
        assert_eq!(
            parse_producer_operation("network-create").unwrap(),
            ProducerOperation::NetworkCreate
        );
        assert_eq!(
            parse_producer_operation("network-connect").unwrap(),
            ProducerOperation::NetworkConnect
        );
        assert_eq!(
            parse_producer_operation("network-disconnect").unwrap(),
            ProducerOperation::NetworkDisconnect
        );
        assert_eq!(
            parse_producer_operation("network-remove").unwrap(),
            ProducerOperation::NetworkRemove
        );
        assert_eq!(
            parse_producer_operation("network-list").unwrap(),
            ProducerOperation::NetworkList
        );
    }

    #[test]
    fn test_resolve_container_name_header_overrides_config() {
        let config = ContainerConfig::from_uri("container:run?name=config-name").unwrap();
        let mut exchange = Exchange::new(Message::new(""));
        exchange.input.set_header(
            HEADER_CONTAINER_NAME,
            serde_json::Value::String("header-name".to_string()),
        );

        let resolved = resolve_container_name(&exchange, &config);
        assert_eq!(resolved.as_deref(), Some("header-name"));
    }

    #[test]
    fn test_container_config_rejects_tcp_host() {
        let config = ContainerConfig::from_uri("container:list?host=tcp://localhost:2375").unwrap();
        let err = config.connect_docker_client().unwrap_err();
        match err {
            CamelError::ProcessorError(msg) => {
                assert!(
                    msg.to_lowercase().contains("tcp"),
                    "Expected TCP scheme error, got: {}",
                    msg
                );
            }
            _ => panic!("Expected ProcessorError for unsupported tcp host"),
        }
    }

    #[tokio::test]
    async fn test_run_container_with_cleanup_removes_on_start_failure() {
        let remove_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let remove_called_clone = remove_called.clone();

        let result = run_container_with_cleanup(
            || async { Ok("container-123".to_string()) },
            |_id| async move {
                Err(CamelError::ProcessorError(
                    "Failed to start container".to_string(),
                ))
            },
            move |_id| {
                let remove_called_inner = remove_called_clone.clone();
                async move {
                    remove_called_inner.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                }
            },
        )
        .await;

        assert!(result.is_err(), "Expected start failure to bubble up");
        assert!(
            remove_called.load(std::sync::atomic::Ordering::SeqCst),
            "Expected cleanup to remove container"
        );
    }

    #[test]
    fn test_container_component_creates_endpoint() {
        let component = ContainerComponent::new();
        assert_eq!(component.scheme(), "container");
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint("container:run?image=alpine", &ctx)
            .unwrap();
        assert_eq!(endpoint.uri(), "container:run?image=alpine");
    }

    #[test]
    fn test_container_config_parses_ports() {
        let config =
            ContainerConfig::from_uri("container:run?image=nginx&ports=8080:80,8443:443").unwrap();
        assert_eq!(config.ports.as_deref(), Some("8080:80,8443:443"));
    }

    #[test]
    fn test_container_config_parses_env() {
        let config =
            ContainerConfig::from_uri("container:run?image=nginx&env=FOO=bar,BAZ=qux").unwrap();
        assert_eq!(config.env.as_deref(), Some("FOO=bar,BAZ=qux"));
    }

    #[test]
    fn test_container_config_parses_logs_options() {
        let config = ContainerConfig::from_uri(
            "container:logs?containerId=my-app&follow=true&timestamps=true&tail=100",
        )
        .unwrap();
        assert_eq!(config.operation, "logs");
        assert_eq!(config.container_id.as_deref(), Some("my-app"));
        assert!(config.follow);
        assert!(config.timestamps);
        assert_eq!(config.tail.as_deref(), Some("100"));
    }

    #[test]
    fn test_container_config_logs_defaults() {
        let config = ContainerConfig::from_uri("container:logs?containerId=test").unwrap();
        assert!(config.follow); // default: true
        assert!(!config.timestamps); // default: false
        assert!(config.tail.is_none()); // default: None (all)
    }

    #[test]
    fn test_parse_ports_single() {
        let config = ContainerConfig::from_uri("container:run?image=nginx&ports=8080:80").unwrap();
        let (exposed, bindings) = config.parse_ports().unwrap();

        assert!(exposed.contains(&"80/tcp".to_string()));
        assert!(bindings.contains_key("80/tcp"));

        let binding = bindings.get("80/tcp").unwrap().as_ref().unwrap();
        assert_eq!(binding.len(), 1);
        assert_eq!(binding[0].host_port, Some("8080".to_string()));
    }

    #[test]
    fn test_parse_ports_multiple() {
        let config =
            ContainerConfig::from_uri("container:run?image=nginx&ports=8080:80,8443:443").unwrap();
        let (exposed, bindings) = config.parse_ports().unwrap();

        assert!(exposed.contains(&"80/tcp".to_string()));
        assert!(exposed.contains(&"443/tcp".to_string()));
        assert_eq!(bindings.len(), 2);
    }

    #[test]
    fn test_parse_ports_with_protocol() {
        let config =
            ContainerConfig::from_uri("container:run?image=nginx&ports=8080:80/tcp,5353:53/udp")
                .unwrap();
        let (exposed, _bindings) = config.parse_ports().unwrap();

        assert!(exposed.contains(&"80/tcp".to_string()));
        assert!(exposed.contains(&"53/udp".to_string()));
    }

    #[test]
    fn test_parse_ports_none() {
        let config = ContainerConfig::from_uri("container:run?image=nginx").unwrap();
        assert!(config.parse_ports().is_none());
    }

    #[test]
    fn test_parse_env_single() {
        let config = ContainerConfig::from_uri("container:run?image=nginx&env=FOO=bar").unwrap();
        let env = config.parse_env().unwrap();

        assert_eq!(env.len(), 1);
        assert_eq!(env[0], "FOO=bar");
    }

    #[test]
    fn test_parse_env_multiple() {
        let config =
            ContainerConfig::from_uri("container:run?image=nginx&env=FOO=bar,BAZ=qux,NUM=123")
                .unwrap();
        let env = config.parse_env().unwrap();

        assert_eq!(env.len(), 3);
        assert!(env.contains(&"FOO=bar".to_string()));
        assert!(env.contains(&"BAZ=qux".to_string()));
        assert!(env.contains(&"NUM=123".to_string()));
    }

    #[test]
    fn test_parse_env_none() {
        let config = ContainerConfig::from_uri("container:run?image=nginx").unwrap();
        assert!(config.parse_env().is_none());
    }

    use camel_component_api::Message;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_container_producer_resolves_operation_from_header() {
        // Try to connect to Docker - if fails, return early
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => d,
            Err(_) => {
                eprintln!("Skipping test: Could not connect to Docker daemon");
                return;
            }
        };

        if docker.ping().await.is_err() {
            eprintln!("Skipping test: Docker daemon not responding to ping");
            return;
        }

        let component = ContainerComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component.create_endpoint("container:run", &ctx).unwrap();

        let ctx = ProducerContext::new();
        let mut producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::new(""));
        exchange
            .input
            .set_header(HEADER_ACTION, serde_json::Value::String("list".into()));

        use tower::ServiceExt;
        let result = producer
            .ready()
            .await
            .unwrap()
            .call(exchange)
            .await
            .unwrap();

        // For list operation, the result header should be on the input message
        assert_eq!(
            result
                .input
                .header(HEADER_ACTION_RESULT)
                .map(|v| v.as_str().unwrap()),
            Some("success")
        );
    }

    #[tokio::test]
    async fn test_container_producer_connection_error_on_invalid_host() {
        // Test that an invalid host (nonexistent socket) results in a connection error
        let component = ContainerComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint("container:list?host=unix:///nonexistent/docker.sock", &ctx)
            .unwrap();

        let ctx = ProducerContext::new();
        let result = endpoint.create_producer(&ctx);

        // The producer should return an error because it cannot connect to the invalid socket
        assert!(
            result.is_err(),
            "Expected error when connecting to invalid host"
        );
        let err = result.unwrap_err();
        match &err {
            CamelError::ProcessorError(msg) => {
                assert!(
                    msg.to_lowercase().contains("connection")
                        || msg.to_lowercase().contains("connect")
                        || msg.to_lowercase().contains("socket")
                        || msg.contains("docker"),
                    "Error message should indicate connection failure, got: {}",
                    msg
                );
            }
            _ => panic!("Expected ProcessorError, got: {:?}", err),
        }
    }

    /// Test that start, stop, remove operations return an error when CamelContainerId header is missing.
    #[tokio::test]
    async fn test_container_producer_lifecycle_operations_missing_id() {
        // Try to connect to Docker - if fails, return early
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => d,
            Err(_) => {
                eprintln!("Skipping test: Could not connect to Docker daemon");
                return;
            }
        };

        if docker.ping().await.is_err() {
            eprintln!("Skipping test: Docker daemon not responding to ping");
            return;
        }

        let component = ContainerComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component.create_endpoint("container:start", &ctx).unwrap();
        let ctx = ProducerContext::new();
        let mut producer = endpoint.create_producer(&ctx).unwrap();

        // Test each lifecycle operation without CamelContainerId header
        for operation in ["start", "stop", "remove"] {
            let mut exchange = Exchange::new(Message::new(""));
            exchange.input.set_header(
                HEADER_ACTION,
                serde_json::Value::String(operation.to_string()),
            );
            // Deliberately NOT setting CamelContainerId header

            use tower::ServiceExt;
            let result = producer.ready().await.unwrap().call(exchange).await;

            assert!(
                result.is_err(),
                "Expected error for {} operation without CamelContainerId",
                operation
            );
            let err = result.unwrap_err();
            match &err {
                CamelError::ProcessorError(msg) => {
                    assert!(
                        msg.contains(HEADER_CONTAINER_ID),
                        "Error message should mention {}, got: {}",
                        HEADER_CONTAINER_ID,
                        msg
                    );
                }
                _ => panic!("Expected ProcessorError for {}, got: {:?}", operation, err),
            }
        }
    }

    /// Test that stop operation returns an error for a nonexistent container.
    #[tokio::test]
    async fn test_container_producer_stop_nonexistent() {
        // Try to connect to Docker - if fails, return early
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => d,
            Err(_) => {
                eprintln!("Skipping test: Could not connect to Docker daemon");
                return;
            }
        };

        if docker.ping().await.is_err() {
            eprintln!("Skipping test: Docker daemon not responding to ping");
            return;
        }

        let component = ContainerComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component.create_endpoint("container:stop", &ctx).unwrap();
        let ctx = ProducerContext::new();
        let mut producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::new(""));
        exchange
            .input
            .set_header(HEADER_ACTION, serde_json::Value::String("stop".into()));
        exchange.input.set_header(
            HEADER_CONTAINER_ID,
            serde_json::Value::String("nonexistent-container-123".into()),
        );

        use tower::ServiceExt;
        let result = producer.ready().await.unwrap().call(exchange).await;

        assert!(
            result.is_err(),
            "Expected error when stopping nonexistent container"
        );
        let err = result.unwrap_err();
        match &err {
            CamelError::ProcessorError(msg) => {
                // Docker API returns 404 with "No such container" message
                assert!(
                    msg.to_lowercase().contains("no such container")
                        || msg.to_lowercase().contains("not found")
                        || msg.contains("404"),
                    "Error message should indicate container not found, got: {}",
                    msg
                );
            }
            _ => panic!("Expected ProcessorError, got: {:?}", err),
        }
    }

    /// Test that run operation returns an error when no image is provided.
    #[tokio::test]
    async fn test_container_producer_run_missing_image() {
        // Try to connect to Docker - if fails, return early
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => d,
            Err(_) => {
                eprintln!("Skipping test: Could not connect to Docker daemon");
                return;
            }
        };

        if docker.ping().await.is_err() {
            eprintln!("Skipping test: Docker daemon not responding to ping");
            return;
        }

        // Create producer without an image in the URI
        let component = ContainerComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component.create_endpoint("container:run", &ctx).unwrap();
        let ctx = ProducerContext::new();
        let mut producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::new(""));
        exchange
            .input
            .set_header(HEADER_ACTION, serde_json::Value::String("run".into()));
        // Deliberately NOT setting CamelContainerImage header

        use tower::ServiceExt;
        let result = producer.ready().await.unwrap().call(exchange).await;

        assert!(
            result.is_err(),
            "Expected error for run operation without image"
        );
        let err = result.unwrap_err();
        match &err {
            CamelError::ProcessorError(msg) => {
                assert!(
                    msg.to_lowercase().contains("image"),
                    "Error message should mention 'image', got: {}",
                    msg
                );
            }
            _ => panic!("Expected ProcessorError, got: {:?}", err),
        }
    }

    /// Test that run operation uses image from header.
    #[tokio::test]
    async fn test_container_producer_run_image_from_header() {
        // Try to connect to Docker - if fails, return early
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => d,
            Err(_) => {
                eprintln!("Skipping test: Could not connect to Docker daemon");
                return;
            }
        };

        if docker.ping().await.is_err() {
            eprintln!("Skipping test: Docker daemon not responding to ping");
            return;
        }

        // Create producer without an image in the URI
        let component = ContainerComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component.create_endpoint("container:run", &ctx).unwrap();
        let ctx = ProducerContext::new();
        let mut producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::new(""));
        exchange
            .input
            .set_header(HEADER_ACTION, serde_json::Value::String("run".into()));
        // Set a non-existent image to test that the run operation attempts to use it
        exchange.input.set_header(
            HEADER_IMAGE,
            serde_json::Value::String("nonexistent-image-xyz-12345:latest".into()),
        );

        use tower::ServiceExt;
        let result = producer.ready().await.unwrap().call(exchange).await;

        // The operation should fail because the image doesn't exist
        assert!(
            result.is_err(),
            "Expected error when running container with nonexistent image"
        );
        let err = result.unwrap_err();
        match &err {
            CamelError::ProcessorError(msg) => {
                // Docker API returns an error about the missing image
                assert!(
                    msg.to_lowercase().contains("no such image")
                        || msg.to_lowercase().contains("not found")
                        || msg.to_lowercase().contains("image")
                        || msg.to_lowercase().contains("pull")
                        || msg.contains("404"),
                    "Error message should indicate image issue, got: {}",
                    msg
                );
            }
            _ => panic!("Expected ProcessorError, got: {:?}", err),
        }
    }

    /// Integration test: Actually run a container with alpine:latest.
    /// This test verifies the full flow: create → start → set headers.
    #[tokio::test]
    async fn test_container_producer_run_alpine_container() {
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => d,
            Err(_) => {
                eprintln!("Skipping test: Could not connect to Docker daemon");
                return;
            }
        };

        if docker.ping().await.is_err() {
            eprintln!("Skipping test: Docker daemon not responding to ping");
            return;
        }

        // Pull alpine:latest if not present
        let images = docker.list_images(None::<ListImagesOptions>).await.unwrap();
        let has_alpine = images
            .iter()
            .any(|img| img.repo_tags.iter().any(|t| t.starts_with("alpine")));

        if !has_alpine {
            eprintln!("Pulling alpine:latest image...");
            let mut stream = docker.create_image(
                Some(CreateImageOptions {
                    from_image: Some("alpine:latest".to_string()),
                    ..Default::default()
                }),
                None,
                None,
            );

            use futures::StreamExt;
            while let Some(_item) = stream.next().await {
                // Wait for pull to complete
            }
            eprintln!("Image pulled successfully");
        }

        // Create producer
        let component = ContainerComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component.create_endpoint("container:run", &ctx).unwrap();
        let ctx = ProducerContext::new();
        let mut producer = endpoint.create_producer(&ctx).unwrap();

        // Run container with unique name
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let container_name = format!("test-rust-camel-{}", timestamp);
        let mut exchange = Exchange::new(Message::new(""));
        exchange.input.set_header(
            HEADER_IMAGE,
            serde_json::Value::String("alpine:latest".into()),
        );
        exchange.input.set_header(
            HEADER_CONTAINER_NAME,
            serde_json::Value::String(container_name.clone()),
        );

        use tower::ServiceExt;
        let result = producer
            .ready()
            .await
            .unwrap()
            .call(exchange)
            .await
            .expect("Container run should succeed");

        // Verify container ID was set
        let container_id = result
            .input
            .header(HEADER_CONTAINER_ID)
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .expect("Expected container ID header");
        assert!(!container_id.is_empty(), "Container ID should not be empty");

        // Verify success header
        assert_eq!(
            result
                .input
                .header(HEADER_ACTION_RESULT)
                .and_then(|v| v.as_str()),
            Some("success")
        );

        // Verify container exists in Docker
        let inspect = docker
            .inspect_container(&container_id, None)
            .await
            .expect("Container should exist");
        assert_eq!(inspect.id.as_deref(), Some(container_id.as_str()));

        // Cleanup: remove container
        docker
            .remove_container(
                &container_id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            .ok();

        eprintln!("✅ Container {} created and cleaned up", container_id);
    }

    /// Test that consumer returns an error for unsupported operations.
    #[tokio::test]
    async fn test_container_consumer_unsupported_operation() {
        use tokio::sync::mpsc;

        let component = ContainerComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component.create_endpoint("container:run", &ctx).unwrap();
        let mut consumer = endpoint.create_consumer().unwrap();

        // Create a minimal ConsumerContext
        let (tx, _rx) = mpsc::channel(16);
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let context = ConsumerContext::new(tx, cancel_token);

        let result = consumer.start(context).await;

        // Should return error because "run" is not a supported consumer operation
        assert!(
            result.is_err(),
            "Expected error for unsupported consumer operation"
        );
        let err = result.unwrap_err();
        match &err {
            CamelError::EndpointCreationFailed(msg) => {
                assert!(
                    msg.contains("Consumer only supports 'events' or 'logs'"),
                    "Error message should mention events or logs support, got: {}",
                    msg
                );
            }
            _ => panic!("Expected EndpointCreationFailed error, got: {:?}", err),
        }
    }

    #[test]
    fn test_container_consumer_concurrency_model_is_concurrent() {
        let consumer = ContainerConsumer {
            config: ContainerConfig::from_uri("container:events").unwrap(),
        };

        assert_eq!(
            consumer.concurrency_model(),
            camel_component_api::ConcurrencyModel::Concurrent { max: None }
        );
    }

    /// Test that consumer gracefully shuts down when cancellation is requested.
    /// This test requires a running Docker daemon. If Docker is not available, the test
    /// will be ignored.
    #[tokio::test]
    async fn test_container_consumer_cancellation() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::sync::mpsc;

        // Try to connect to Docker - if fails, return early
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => d,
            Err(_) => {
                eprintln!("Skipping test: Could not connect to Docker daemon");
                return;
            }
        };

        if docker.ping().await.is_err() {
            eprintln!("Skipping test: Docker daemon not responding to ping");
            return;
        }

        let component = ContainerComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component.create_endpoint("container:events", &ctx).unwrap();
        let mut consumer = endpoint.create_consumer().unwrap();

        // Create a ConsumerContext
        let (tx, _rx) = mpsc::channel(16);
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let context = ConsumerContext::new(tx, cancel_token.clone());

        // Track if the consumer task has completed
        let completed = Arc::new(AtomicBool::new(false));
        let completed_clone = completed.clone();

        // Spawn consumer in background
        let handle = tokio::spawn(async move {
            let result = consumer.start(context).await;
            // Mark as completed when done
            completed_clone.store(true, Ordering::SeqCst);
            result
        });

        // Wait a bit for the consumer to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Consumer should still be running (not completed)
        assert!(
            !completed.load(Ordering::SeqCst),
            "Consumer should still be running before cancellation"
        );

        // Request cancellation
        cancel_token.cancel();

        // Wait for the task to complete (with timeout)
        let result = tokio::time::timeout(tokio::time::Duration::from_millis(500), handle).await;

        // Task should have completed (not timed out)
        assert!(
            result.is_ok(),
            "Consumer should gracefully shut down after cancellation"
        );

        // Verify the consumer completed
        assert!(
            completed.load(Ordering::SeqCst),
            "Consumer should have completed after cancellation"
        );
    }

    /// Integration test for listing containers.
    /// This test requires a running Docker daemon. If Docker is not available, the test
    /// will return early and be effectively ignored.
    #[tokio::test]
    async fn test_container_producer_list_containers() {
        // Try to connect to Docker using default config
        // If ping fails, return early (effectively ignoring this test)
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => d,
            Err(_) => {
                eprintln!("Skipping test: Could not connect to Docker daemon");
                return;
            }
        };

        if docker.ping().await.is_err() {
            eprintln!("Skipping test: Docker daemon not responding to ping");
            return;
        }

        // Create producer with list operation
        let component = ContainerComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component.create_endpoint("container:list", &ctx).unwrap();

        let ctx = ProducerContext::new();
        let mut producer = endpoint.create_producer(&ctx).unwrap();

        // Create exchange with list operation in header
        let mut exchange = Exchange::new(Message::new(""));
        exchange
            .input
            .set_header(HEADER_ACTION, serde_json::Value::String("list".into()));

        // Call the producer
        use tower::ServiceExt;
        let result = producer
            .ready()
            .await
            .unwrap()
            .call(exchange)
            .await
            .expect("Producer should succeed when Docker is available");

        // Assert that the input exchange body is a JSON array
        // (because list_containers should put the JSON result in the body)
        match &result.input.body {
            camel_component_api::Body::Json(json_value) => {
                assert!(
                    json_value.is_array(),
                    "Expected input body to be a JSON array, got: {:?}",
                    json_value
                );
            }
            other => panic!("Expected Body::Json with array, got: {:?}", other),
        }
    }

    #[test]
    fn test_container_config_parses_volumes() {
        let config = ContainerConfig::from_uri(
            "container:run?image=nginx&volumes=./html:/usr/share/nginx/html:ro",
        )
        .unwrap();
        assert_eq!(
            config.volumes.as_deref(),
            Some("./html:/usr/share/nginx/html:ro")
        );
    }

    #[test]
    fn test_container_config_parses_exec_params() {
        let config = ContainerConfig::from_uri(
            "container:exec?containerId=my-app&cmd=ls /app&user=root&workdir=/tmp&detach=true",
        )
        .unwrap();
        assert_eq!(config.operation, "exec");
        assert_eq!(config.container_id.as_deref(), Some("my-app"));
        assert_eq!(config.cmd.as_deref(), Some("ls /app"));
        assert_eq!(config.user.as_deref(), Some("root"));
        assert_eq!(config.workdir.as_deref(), Some("/tmp"));
        assert!(config.detach);
    }

    #[test]
    fn test_container_config_parses_network_create_params() {
        let config =
            ContainerConfig::from_uri("container:network-create?name=my-net&driver=bridge")
                .unwrap();
        assert_eq!(config.operation, "network-create");
        assert_eq!(config.name.as_deref(), Some("my-net"));
        assert_eq!(config.driver.as_deref(), Some("bridge"));
    }

    #[test]
    fn test_container_config_defaults_new_fields() {
        let config = ContainerConfig::from_uri("container:list").unwrap();
        assert!(config.volumes.is_none());
        assert!(config.user.is_none());
        assert!(config.workdir.is_none());
        assert!(!config.detach);
        assert!(config.driver.is_none());
        assert!(!config.force);
    }

    #[test]
    fn test_parse_volumes_bind_mount() {
        let config = ContainerConfig::from_uri(
            "container:run?image=nginx&volumes=./html:/usr/share/nginx/html:ro",
        )
        .unwrap();
        let (binds, anon) = config.parse_volumes().unwrap();
        assert_eq!(binds, vec!["./html:/usr/share/nginx/html:ro"]);
        assert!(anon.is_empty());
    }

    #[test]
    fn test_parse_volumes_named_volume() {
        let config =
            ContainerConfig::from_uri("container:run?image=postgres&volumes=data:/var/lib/data")
                .unwrap();
        let (binds, anon) = config.parse_volumes().unwrap();
        assert_eq!(binds, vec!["data:/var/lib/data"]);
        assert!(anon.is_empty());
    }

    #[test]
    fn test_parse_volumes_anonymous() {
        let config =
            ContainerConfig::from_uri("container:run?image=alpine&volumes=/tmp/app-data").unwrap();
        let (binds, anon) = config.parse_volumes().unwrap();
        assert!(binds.is_empty());
        assert!(anon.contains(&"/tmp/app-data".to_string()));
    }

    #[test]
    fn test_parse_volumes_anonymous_with_mode() {
        let config =
            ContainerConfig::from_uri("container:run?image=alpine&volumes=/tmp/app-data:ro")
                .unwrap();
        let (binds, anon) = config.parse_volumes().unwrap();
        assert!(binds.is_empty());
        assert!(anon.contains(&"/tmp/app-data".to_string()));
    }

    #[test]
    fn test_parse_volumes_multiple() {
        let config = ContainerConfig::from_uri(
            "container:run?image=nginx&volumes=./html:/usr/share/nginx/html:ro,data:/var/log/app",
        )
        .unwrap();
        let (binds, anon) = config.parse_volumes().unwrap();
        assert_eq!(binds.len(), 2);
        assert!(binds.contains(&"./html:/usr/share/nginx/html:ro".to_string()));
        assert!(binds.contains(&"data:/var/log/app".to_string()));
        assert!(anon.is_empty());
    }

    #[test]
    fn test_parse_volumes_mixed() {
        let config = ContainerConfig::from_uri(
            "container:run?image=nginx&volumes=./html:/usr/share/nginx/html:ro,/tmp/cache",
        )
        .unwrap();
        let (binds, anon) = config.parse_volumes().unwrap();
        assert_eq!(binds.len(), 1);
        assert!(anon.contains(&"/tmp/cache".to_string()));
    }

    #[test]
    fn test_parse_volumes_none() {
        let config = ContainerConfig::from_uri("container:run?image=nginx").unwrap();
        assert!(config.parse_volumes().is_none());
    }

    #[test]
    fn test_parse_volumes_empty_entry_skipped() {
        let config = ContainerConfig::from_uri("container:run?image=nginx&volumes=,,").unwrap();
        assert!(config.parse_volumes().is_none());
    }

    #[test]
    fn test_parse_volumes_rw_mode() {
        let config =
            ContainerConfig::from_uri("container:run?image=nginx&volumes=./data:/app/data:rw")
                .unwrap();
        let (binds, _) = config.parse_volumes().unwrap();
        assert_eq!(binds, vec!["./data:/app/data:rw"]);
    }

    #[tokio::test]
    async fn test_container_producer_network_lifecycle() {
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => d,
            Err(_) => {
                eprintln!("Skipping test: Could not connect to Docker daemon");
                return;
            }
        };
        if docker.ping().await.is_err() {
            eprintln!("Skipping test: Docker daemon not responding to ping");
            return;
        }

        let network_name = format!("camel-test-{}", std::process::id());

        let component = ContainerComponent::new();
        let component_ctx = NoOpComponentContext;

        // Create network
        let endpoint = component
            .create_endpoint(
                &format!("container:network-create?name={}", network_name),
                &component_ctx,
            )
            .unwrap();
        let ctx = ProducerContext::new();
        let mut producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::new(""));
        exchange.input.set_header(
            HEADER_ACTION,
            serde_json::Value::String("network-create".into()),
        );

        use tower::ServiceExt;
        let result = producer
            .ready()
            .await
            .unwrap()
            .call(exchange)
            .await
            .expect("Network create should succeed");

        let network_id = result
            .input
            .header(HEADER_NETWORK)
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .expect("Should have network ID");

        assert!(!network_id.is_empty());

        // List networks
        let endpoint = component
            .create_endpoint("container:network-list", &component_ctx)
            .unwrap();
        let mut producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::new(""));
        exchange.input.set_header(
            HEADER_ACTION,
            serde_json::Value::String("network-list".into()),
        );

        let list_result = producer
            .ready()
            .await
            .unwrap()
            .call(exchange)
            .await
            .expect("Network list should succeed");

        match &list_result.input.body {
            Body::Json(json_value) => {
                assert!(json_value.is_array(), "Expected JSON array");
            }
            other => panic!("Expected Body::Json, got: {:?}", other),
        }

        // Remove network
        let endpoint = component
            .create_endpoint(
                &format!("container:network-remove?network={}", network_name),
                &component_ctx,
            )
            .unwrap();
        let mut producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::new(""));
        exchange.input.set_header(
            HEADER_ACTION,
            serde_json::Value::String("network-remove".into()),
        );

        let remove_result = producer
            .ready()
            .await
            .unwrap()
            .call(exchange)
            .await
            .expect("Network remove should succeed");

        let action_result = remove_result
            .input
            .header(HEADER_ACTION_RESULT)
            .and_then(|v| v.as_str());
        assert_eq!(action_result, Some("success"));
    }
}
