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

  @Inject SecurityProfileStore profileStore;

  void onStart(@Observes StartupEvent ev) {
    System.out.println("{\"status\":\"ready\",\"port\":" + grpcPort + "}");
    System.out.flush();
    if (!profileStore.isEmpty()) {
      System.out.println(
          "WS-Security: "
              + profileStore.profiles().size()
              + " profile(s) loaded: "
              + profileStore.profiles().keySet());
    } else {
      System.out.println("WS-Security: DISABLED (no security profiles configured)");
    }
    System.out.flush();
  }
}
