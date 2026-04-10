package org.rustcamel.jms;

import io.quarkus.runtime.StartupEvent;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.enterprise.event.Observes;
import org.eclipse.microprofile.config.inject.ConfigProperty;

@ApplicationScoped
public class PortAnnouncer {

  // Reads the configured gRPC port. The Rust BridgeProcess sets this via the
  // QUARKUS_GRPC_SERVER_PORT env var before launching the JVM. If the port is
  // already bound (race condition), Quarkus will fail to start entirely — which
  // is an acceptable failure mode since the Rust side handles process restart.
  // Using @ConfigProperty is correct here because Quarkus reads the env var and
  // binds to the specified port at startup.
  @ConfigProperty(name = "quarkus.grpc.server.port", defaultValue = "9090")
  int grpcPort;

  void onStart(@Observes StartupEvent ev) {
    // Rust BridgeProcess reads this JSON line from stdout to discover the gRPC port
    System.out.println("{\"status\":\"ready\",\"port\":" + grpcPort + "}");
    System.out.flush();
  }
}
