//! Static fake JMS bridge used as a stand-in bridge process in shell-less
//! container smoke tests. Test-fixture only: it is never shipped.

use std::env;
use std::io::Write;
use std::process::ExitCode;

use tonic::transport::{Identity, Server, ServerTlsConfig};

mod proto {
    tonic::include_proto!("jms_bridge");
}

use proto::bridge_service_server::{BridgeService, BridgeServiceServer};
use proto::{
    CompletionRequest, CompletionResponse, HealthRequest, HealthResponse, JmsMessage, SendRequest,
    SendResponse, SubscribeRequest,
};

/// Serves `Health` only; every other RPC reports
/// `Status::unimplemented` (tonic 0.14 generates no trait defaults).
struct FakeBridge;

#[tonic::async_trait]
impl BridgeService for FakeBridge {
    type SubscribeStream = std::pin::Pin<
        Box<dyn tokio_stream::Stream<Item = Result<JmsMessage, tonic::Status>> + Send>,
    >;

    async fn send(
        &self,
        _request: tonic::Request<SendRequest>,
    ) -> Result<tonic::Response<SendResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented(
            "camel-fake-bridge serves Health only",
        ))
    }

    async fn subscribe(
        &self,
        _request: tonic::Request<SubscribeRequest>,
    ) -> Result<tonic::Response<Self::SubscribeStream>, tonic::Status> {
        Err(tonic::Status::unimplemented(
            "camel-fake-bridge serves Health only",
        ))
    }

    async fn health(
        &self,
        _request: tonic::Request<HealthRequest>,
    ) -> Result<tonic::Response<HealthResponse>, tonic::Status> {
        Ok(tonic::Response::new(HealthResponse {
            healthy: true,
            capabilities: Vec::new(),
            ..Default::default()
        }))
    }

    async fn complete(
        &self,
        _request: tonic::Request<CompletionRequest>,
    ) -> Result<tonic::Response<CompletionResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented(
            "camel-fake-bridge serves Health only",
        ))
    }
}

fn required_env(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    match env::var(name) {
        Ok(value) if !value.is_empty() => Ok(value),
        _ => Err(format!("missing required environment variable: {name}").into()),
    }
}

/// Reads the PEM file referenced by `name` (the bridge contract passes paths,
/// matching the real JMS bridge configuration).
fn required_pem_file(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let path = required_env(name)?;
    std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {name} at {path:?}: {error}").into())
}

/// Signal registrations armed before readiness is announced, so a SIGTERM
/// arriving immediately after the ready line is already handled.
struct ShutdownArm {
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
}

/// Registers the SIGTERM listener. Called synchronously by `run` before it
/// prints the ready line; only polling `recv()` is deferred to shutdown.
fn arm_shutdown_signal() -> std::io::Result<ShutdownArm> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        Ok(ShutdownArm {
            terminate: signal(SignalKind::terminate())?,
        })
    }
    #[cfg(not(unix))]
    {
        Ok(ShutdownArm {})
    }
}

/// Resolves when the process receives SIGTERM (or Ctrl-C on non-Unix).
async fn shutdown_signal(arm: ShutdownArm) {
    #[cfg(unix)]
    {
        let ShutdownArm { mut terminate } = arm;

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let ShutdownArm {} = arm;
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // The workspace unifies rustls' `ring` and `aws-lc-rs` providers, so the
    // process-level default must be selected explicitly.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let port: u16 = match env::var("QUARKUS_HTTP_SSL_PORT") {
        Ok(value) if !value.is_empty() => value
            .parse()
            .map_err(|_| format!("QUARKUS_HTTP_SSL_PORT must be a u16, got {value:?}"))?,
        _ => 0,
    };

    let cert_pem = required_pem_file("QUARKUS_TLS_BRIDGE_KEY_STORE_PEM_0_CERT")?;
    let key_pem = required_pem_file("QUARKUS_TLS_BRIDGE_KEY_STORE_PEM_0_KEY")?;
    // The fixture does not require client certificates, but the bridge
    // contract requires the trust store to be configured: fail loudly when it
    // is absent instead of silently accepting a misconfigured fixture.
    let _trust_pem = required_pem_file("QUARKUS_TLS_BRIDGE_TRUST_STORE_PEM_CERTS")?;

    let tls = ServerTlsConfig::new().identity(Identity::from_pem(cert_pem, key_pem));

    // Bind explicitly so the announced port is the real bound port (port 0
    // asks the kernel for an ephemeral one).
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    let bound_port = listener.local_addr()?.port();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    let server = Server::builder()
        .tls_config(tls)?
        .add_service(BridgeServiceServer::new(FakeBridge));

    // Arm the SIGTERM handler before readiness: a signal arriving right after
    // the ready line must trigger a clean shutdown, not default termination.
    let shutdown_arm = arm_shutdown_signal()?;

    // Readiness contract: exactly one stdout line, printed after the socket
    // is bound (the listener already accepts connections via its backlog).
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    writeln!(stdout, r#"{{"status":"ready","port":{bound_port}}}"#)?;
    stdout.flush()?;
    drop(stdout);

    server
        .serve_with_incoming_shutdown(incoming, shutdown_signal(shutdown_arm))
        .await?;
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("camel-fake-bridge: {error}");
            let mut source = error.source();
            while let Some(cause) = source {
                eprintln!("  caused by: {cause}");
                source = cause.source();
            }
            ExitCode::FAILURE
        }
    }
}
