package org.rustcamel.jms;

import io.quarkus.runtime.StartupEvent;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.enterprise.event.Observes;
import org.eclipse.microprofile.config.inject.ConfigProperty;

/**
 * Twin-copy of bridges/xml PortAnnouncer (independent gradle builds, repo convention): keep the
 * startup checks in lockstep when either copy changes.
 */
@ApplicationScoped
public class PortAnnouncer {

  @ConfigProperty(name = "quarkus.http.ssl-port", defaultValue = "8443")
  int sslPort;

  @ConfigProperty(name = "quarkus.tls.bridge.key-store.pem.0.cert")
  String serverCertPath;

  void onStart(@Observes StartupEvent ev) {
    if (sslPort > 0 && serverCertPath != null && serverCertPath.contains("placeholder-")) {
      throw new RuntimeException(
          "Bridge started with placeholder TLS certs — runtime env vars not set. Aborting.");
    }
    System.out.println("{\"status\":\"ready\",\"port\":" + sslPort + "}");
    System.out.flush();
  }
}
