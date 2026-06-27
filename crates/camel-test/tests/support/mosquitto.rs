#![allow(dead_code)]

use std::time::Duration;

use testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{ContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::sync::OnceCell;

pub const MQTT_PORT: u16 = 1883;

static MOSQUITTO: OnceCell<(ContainerAsync<GenericImage>, String)> = OnceCell::const_new();

/// Shared Mosquitto container, started once and reused across tests. Returns the
/// `host:port` (e.g. `127.0.0.1:<mapped>`) the component config should point at.
pub async fn shared_mosquitto() -> &'static (ContainerAsync<GenericImage>, String) {
    MOSQUITTO
        .get_or_init(|| async {
            super::init_tracing();
            // mosquitto 2.x needs an explicit listener + allow_anonymous to accept
            // external anonymous clients. The default ENTRYPOINT (/docker-entrypoint.sh)
            // execs "$@", so we override the CMD (not the entrypoint) to write the
            // permissive config and then start mosquitto.
            let conf = "listener 1883 0.0.0.0\nallow_anonymous true\n";
            let cmd = format!(
                "printf '{conf}' > /mosquitto/config/mosquitto.conf && exec mosquitto -c /mosquitto/config/mosquitto.conf -v"
            );
            let image = GenericImage::new("eclipse-mosquitto", "2.0.20")
                .with_exposed_port(ContainerPort::Tcp(MQTT_PORT))
                .with_wait_for(WaitFor::message_on_stdout("mosquitto version"))
                .with_cmd(["sh", "-c", &cmd])
                .with_startup_timeout(Duration::from_secs(60));

            let container = image.start().await.expect("Mosquitto container failed to start");
            let port = container
                .get_host_port_ipv4(MQTT_PORT)
                .await
                .expect("MQTT port not available");
            let host_port = format!("127.0.0.1:{port}");
            eprintln!("Mosquitto ready at: {host_port}");
            (container, host_port)
        })
        .await
}
