package org.rustcamel.cxf;

import io.quarkus.runtime.StartupEvent;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.enterprise.event.Observes;
import jakarta.inject.Inject;
import org.eclipse.microprofile.config.inject.ConfigProperty;

@ApplicationScoped
public class PortAnnouncer {

  @ConfigProperty(name = "quarkus.http.ssl-port", defaultValue = "8443")
  int sslPort;

  @ConfigProperty(name = "quarkus.tls.bridge.key-store.pem.0.cert")
  String serverCertPath;

  @Inject SecurityProfileStore profileStore;

  void onStart(@Observes StartupEvent ev) {
    if (sslPort > 0 && serverCertPath != null && serverCertPath.contains("placeholder-")) {
      throw new RuntimeException(
          "Bridge started with placeholder TLS certs — runtime env vars not set. Aborting.");
    }
    System.out.println("{\"status\":\"ready\",\"port\":" + sslPort + "}");
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
