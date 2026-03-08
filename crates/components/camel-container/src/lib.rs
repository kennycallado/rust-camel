//! Camel Container Component
//!
//! This component provides integration with Docker containers, allowing Camel routes
//! to manage container lifecycle (create, start, stop, remove) and consume container events.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use async_trait::async_trait;
use bollard::Docker;
use bollard::service::{HostConfig, PortBinding};
use camel_api::{Body, BoxProcessor, CamelError, Exchange, Message};
use camel_component::{Component, Consumer, ConsumerContext, Endpoint, ProducerContext};
use camel_endpoint::parse_uri;
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
                Some(bollard::container::RemoveContainerOptions {
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
        let host = parts
            .params
            .get("host")
            .cloned()
            .or_else(|| Some("unix:///var/run/docker.sock".to_string()));

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
        })
    }

    fn docker_socket_path(&self) -> Result<&str, CamelError> {
        let host = self
            .host
            .as_deref()
            .unwrap_or("unix:///var/run/docker.sock");

        if host.starts_with("unix://") {
            return Ok(host.trim_start_matches("unix://"));
        }

        if host.contains("://") {
            return Err(CamelError::ProcessorError(format!(
                "Unsupported Docker host scheme: {} (only unix:// is supported)",
                host
            )));
        }

        Ok(host)
    }

    pub fn connect_docker_client(&self) -> Result<Docker, CamelError> {
        let socket_path = self.docker_socket_path()?;
        Docker::connect_with_unix(
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
    fn parse_ports(
        &self,
    ) -> Option<(
        HashMap<String, HashMap<(), ()>>,
        HashMap<String, Option<Vec<PortBinding>>>,
    )> {
        let ports_str = self.ports.as_ref()?;

        let mut exposed_ports: HashMap<String, HashMap<(), ()>> = HashMap::new();
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

            exposed_ports.insert(container_key.clone(), HashMap::new());

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProducerOperation {
    List,
    Run,
    Start,
    Stop,
    Remove,
}

fn parse_producer_operation(operation: &str) -> Result<ProducerOperation, CamelError> {
    match operation {
        "list" => Ok(ProducerOperation::List),
        "run" => Ok(ProducerOperation::Run),
        "start" => Ok(ProducerOperation::Start),
        "stop" => Ok(ProducerOperation::Stop),
        "remove" => Ok(ProducerOperation::Remove),
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
        .list_images::<&str>(None)
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
        Some(bollard::image::CreateImageOptions {
            from_image: image,
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
            // Extract operation from header or use config
            let operation_name = exchange
                .input
                .header(HEADER_ACTION)
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| config.operation.clone());

            let operation = parse_producer_operation(&operation_name)?;

            // Execute operation
            match operation {
                ProducerOperation::List => {
                    let containers = docker.list_containers::<String>(None).await.map_err(|e| {
                        CamelError::ProcessorError(format!("Failed to list containers: {}", e))
                    })?;

                    let json_value = serde_json::to_value(&containers).map_err(|e| {
                        CamelError::ProcessorError(format!("Failed to serialize containers: {}", e))
                    })?;

                    exchange.input.body = Body::Json(json_value);
                    exchange.input.set_header(
                        HEADER_ACTION_RESULT,
                        serde_json::Value::String("success".to_string()),
                    );
                }
                ProducerOperation::Run => {
                    // Run operation: create and start a container from an image
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

                    // Ensure image is available (auto-pull if needed)
                    let pull_timeout = 300; // 5 minutes default
                    ensure_image_available(&docker, &image, config.auto_pull, pull_timeout)
                        .await
                        .map_err(|e| {
                            CamelError::ProcessorError(format!(
                                "Image '{}' not available: {}",
                                image, e
                            ))
                        })?;

                    let container_name = resolve_container_name(&exchange, &config);
                    let container_name_ref = container_name.as_deref().unwrap_or("");
                    let cmd_parts: Option<Vec<String>> = config
                        .cmd
                        .as_ref()
                        .map(|c| c.split_whitespace().map(|s| s.to_string()).collect());
                    let auto_remove = config.auto_remove;
                    let (exposed_ports, port_bindings) = config.parse_ports().unwrap_or_default();
                    let env_vars = config.parse_env();
                    let network_mode = config.network.clone();

                    let docker_create = docker.clone();
                    let docker_start = docker.clone();
                    let docker_remove = docker.clone();

                    let container_id = run_container_with_cleanup(
                        move || async move {
                            let create_options = bollard::container::CreateContainerOptions {
                                name: container_name_ref,
                                ..Default::default()
                            };
                            let container_config = bollard::container::Config::<String> {
                                image: Some(image.clone()),
                                cmd: cmd_parts,
                                env: env_vars,
                                exposed_ports: if exposed_ports.is_empty() { None } else { Some(exposed_ports) },
                                host_config: Some(HostConfig {
                                    auto_remove: Some(auto_remove),
                                    port_bindings: if port_bindings.is_empty() { None } else { Some(port_bindings) },
                                    network_mode,
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
                                .start_container::<String>(&container_id, None)
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
                }
                ProducerOperation::Start | ProducerOperation::Stop | ProducerOperation::Remove => {
                    // Lifecycle operations require a container ID
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
                                .start_container::<String>(&container_id, None)
                                .await
                                .map_err(|e| {
                                    CamelError::ProcessorError(format!(
                                        "Failed to start container: {}",
                                        e
                                    ))
                                })?;
                        }
                        ProducerOperation::Stop => {
                            docker
                                .stop_container(&container_id, None)
                                .await
                                .map_err(|e| {
                                    CamelError::ProcessorError(format!(
                                        "Failed to stop container: {}",
                                        e
                                    ))
                                })?;
                        }
                        ProducerOperation::Remove => {
                            docker
                                .remove_container(&container_id, None)
                                .await
                                .map_err(|e| {
                                    CamelError::ProcessorError(format!(
                                        "Failed to remove container: {}",
                                        e
                                    ))
                                })?;
                            untrack_container(&container_id);
                        }
                        _ => {}
                    }

                    // Set success result
                    exchange.input.set_header(
                        HEADER_ACTION_RESULT,
                        serde_json::Value::String("success".to_string()),
                    );
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

    fn concurrency_model(&self) -> camel_component::ConcurrencyModel {
        camel_component::ConcurrencyModel::Concurrent { max: None }
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

            let mut event_stream = docker.events::<String>(None);

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

            let options = bollard::container::LogsOptions::<String> {
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
pub struct ContainerComponent;

impl ContainerComponent {
    /// Creates a new container component instance.
    pub fn new() -> Self {
        Self
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

    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
        let config = ContainerConfig::from_uri(uri)?;
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

    #[test]
    fn test_container_config() {
        let config = ContainerConfig::from_uri("container:run?image=alpine").unwrap();
        assert_eq!(config.operation, "run");
        assert_eq!(config.image.as_deref(), Some("alpine"));
        assert_eq!(config.host.as_deref(), Some("unix:///var/run/docker.sock"));
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
        let endpoint = component
            .create_endpoint("container:run?image=alpine")
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

        assert!(exposed.contains_key("80/tcp"));
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

        assert!(exposed.contains_key("80/tcp"));
        assert!(exposed.contains_key("443/tcp"));
        assert_eq!(bindings.len(), 2);
    }

    #[test]
    fn test_parse_ports_with_protocol() {
        let config =
            ContainerConfig::from_uri("container:run?image=nginx&ports=8080:80/tcp,5353:53/udp")
                .unwrap();
        let (exposed, bindings) = config.parse_ports().unwrap();

        assert!(exposed.contains_key("80/tcp"));
        assert!(exposed.contains_key("53/udp"));
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

    mod test_helpers {
        use async_trait::async_trait;
        use camel_api::CamelError;

        /// A no-op route controller for testing purposes.
        pub struct NullRouteController;

        #[async_trait]
        impl camel_api::RouteController for NullRouteController {
            async fn start_route(&mut self, _: &str) -> Result<(), CamelError> {
                Ok(())
            }
            async fn stop_route(&mut self, _: &str) -> Result<(), CamelError> {
                Ok(())
            }
            async fn restart_route(&mut self, _: &str) -> Result<(), CamelError> {
                Ok(())
            }
            async fn suspend_route(&mut self, _: &str) -> Result<(), CamelError> {
                Ok(())
            }
            async fn resume_route(&mut self, _: &str) -> Result<(), CamelError> {
                Ok(())
            }
            fn route_status(&self, _: &str) -> Option<camel_api::RouteStatus> {
                None
            }
            async fn start_all_routes(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
            async fn stop_all_routes(&mut self) -> Result<(), CamelError> {
                Ok(())
            }
        }
    }

    use camel_api::Message;
    use std::sync::Arc;
    use test_helpers::NullRouteController;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn test_container_producer_resolves_operation_from_header() {
        let component = ContainerComponent::new();
        let endpoint = component.create_endpoint("container:run").unwrap();

        let ctx = ProducerContext::new(Arc::new(Mutex::new(NullRouteController)));
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
        let endpoint = component
            .create_endpoint("container:list?host=unix:///nonexistent/docker.sock")
            .unwrap();

        let ctx = ProducerContext::new(Arc::new(Mutex::new(NullRouteController)));
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
        let endpoint = component.create_endpoint("container:start").unwrap();
        let ctx = ProducerContext::new(Arc::new(Mutex::new(NullRouteController)));
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
        let endpoint = component.create_endpoint("container:stop").unwrap();
        let ctx = ProducerContext::new(Arc::new(Mutex::new(NullRouteController)));
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
        let endpoint = component.create_endpoint("container:run").unwrap();
        let ctx = ProducerContext::new(Arc::new(Mutex::new(NullRouteController)));
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
        let endpoint = component.create_endpoint("container:run").unwrap();
        let ctx = ProducerContext::new(Arc::new(Mutex::new(NullRouteController)));
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
        let images = docker.list_images::<&str>(None).await.unwrap();
        let has_alpine = images
            .iter()
            .any(|img| img.repo_tags.iter().any(|t| t.starts_with("alpine")));

        if !has_alpine {
            eprintln!("Pulling alpine:latest image...");
            let mut stream = docker.create_image(
                Some(bollard::image::CreateImageOptions {
                    from_image: "alpine:latest",
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
        let endpoint = component.create_endpoint("container:run").unwrap();
        let ctx = ProducerContext::new(Arc::new(Mutex::new(NullRouteController)));
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
        assert_eq!(
            inspect.id.as_ref().map(|s| s.as_str()),
            Some(container_id.as_str())
        );

        // Cleanup: remove container
        docker
            .remove_container(
                &container_id,
                Some(bollard::container::RemoveContainerOptions {
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
        let endpoint = component.create_endpoint("container:run").unwrap();
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
            camel_component::ConcurrencyModel::Concurrent { max: None }
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
        let endpoint = component.create_endpoint("container:events").unwrap();
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
        let endpoint = component.create_endpoint("container:list").unwrap();

        let ctx = ProducerContext::new(Arc::new(Mutex::new(NullRouteController)));
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
            camel_api::Body::Json(json_value) => {
                assert!(
                    json_value.is_array(),
                    "Expected input body to be a JSON array, got: {:?}",
                    json_value
                );
            }
            other => panic!("Expected Body::Json with array, got: {:?}", other),
        }
    }
}
