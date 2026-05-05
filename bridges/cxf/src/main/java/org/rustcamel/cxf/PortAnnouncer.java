package org.rustcamel.cxf;

import io.quarkus.runtime.StartupEvent;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.enterprise.event.Observes;
import jakarta.inject.Inject;
import org.eclipse.microprofile.config.inject.ConfigProperty;

@ApplicationScoped
public class PortAnnouncer {

  @ConfigProperty(name = "quarkus.grpc.server.port", defaultValue = "9090")
  int grpcPort;

  @Inject WssSecurityProcessor wssProcessor;

  void onStart(@Observes StartupEvent ev) {
    System.out.println("{\"status\":\"ready\",\"port\":" + grpcPort + "}");
    System.out.flush();
    if (wssProcessor.isEnabled()) {
      System.out.println("WS-Security: ENABLED (signing/verification active)");
    } else {
      System.out.println("WS-Security: DISABLED (no keystore configured)");
    }
    System.out.flush();
  }
}
