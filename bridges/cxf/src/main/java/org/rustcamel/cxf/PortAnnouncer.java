package org.rustcamel.cxf;

import io.quarkus.runtime.StartupEvent;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.enterprise.event.Observes;
import jakarta.inject.Inject;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import org.eclipse.microprofile.config.ConfigProvider;
import org.eclipse.microprofile.config.inject.ConfigProperty;

/**
 * Announces the bridge's actual bound ports on stdout as JSON lines: {"status":"ready","port":N}
 * for the gRPC SSL port and, when a SOAP bind is configured, {"status":"soap-ready","port":M} for
 * the SOAP listener. The Rust supervisor (camel-bridge process.rs) blocks on the ready line and
 * connects its mTLS channel to the announced port; the soap-ready line (Task 2.7) carries the SOAP
 * listener's ACTUAL bound port and is emitted at startup with no {@code OpenConsumerStream} RPC
 * involved.
 *
 * <p>Port handoff (bd rc-s7dyw): the supervisor passes QUARKUS_HTTP_SSL_PORT=0, the JVM binds an
 * OS-assigned port — held by the JVM from the first moment the port exists — and the ready line
 * reports the ACTUAL bound port. This closes the ADR-0070 bind-read-drop race: no window exists in
 * which the port number is known but unheld. A fixed port (&gt;0, standalone operators) still binds
 * exactly as configured and is announced unchanged. The ready line is emitted only after the port
 * is actually bound — the announce thread waits for the listener, so "ready" means connectable, not
 * merely "startup observer ran". The soap-ready line follows the same rule against {@link
 * SoapEndpointPublisher#soapPort()}.
 *
 * <p>The actual gRPC port is read through the config system: once the Vert.x listen handler
 * registers the real port, the ValueRegistryConfigSource resolves {@code quarkus.http.ssl-port} to
 * it (the raw value 0 never survives to the announce). The plain HTTP port is the fallback when TLS
 * is disabled (test profile); -1 (disabled) resolves as "not bound". StartupEvent fires before the
 * HTTP verticle deploys, so waiting from the observer thread itself would deadlock the boot — the
 * waits run on small daemon threads instead. The two StartupEvent observers (this class and {@link
 * SoapEndpointPublisher}) have no guaranteed order, so the soap-ready announcer waits (bounded) for
 * {@code soapPort()} instead of assuming publication already happened.
 *
 * <p>Twin-copy of bridges/jms and bridges/xml PortAnnouncer (independent gradle builds, repo
 * convention): keep the startup checks in lockstep when any copy changes. The soap-ready line is
 * CXF-only (no SOAP listener exists on the JMS/XML bridges).
 */
@ApplicationScoped
public class PortAnnouncer {

  /** Ready lines emitted at startup; observable by tests as protocol evidence. */
  static final List<String> ANNOUNCEMENTS = new CopyOnWriteArrayList<>();

  private static final long AWAIT_NANOS = 60_000_000_000L;
  private static final long POLL_MILLIS = 5;

  @ConfigProperty(name = "quarkus.http.ssl-port", defaultValue = "8443")
  int sslPort;

  @ConfigProperty(name = "quarkus.tls.bridge.key-store.pem.0.cert")
  String serverCertPath;

  @Inject SecurityProfileStore profileStore;

  @Inject BridgeConfig bridgeConfig;

  @Inject SoapEndpointPublisher endpointPublisher;

  void onStart(@Observes StartupEvent ev) {
    if (sslPort > 0 && serverCertPath != null && serverCertPath.contains("placeholder-")) {
      throw new RuntimeException(
          "Bridge started with placeholder TLS certs — runtime env vars not set. Aborting.");
    }
    Thread announcer = new Thread(PortAnnouncer::announceWhenBound, "bridge-port-announcer");
    announcer.setDaemon(true);
    announcer.start();
    // SOAP listener announcement: only when a bind is configured (no implicit default bind).
    // The startup publish() runs on the sibling StartupEvent observer in unknown order, so the
    // soap-ready announcer waits (bounded) for the actual bound port.
    String configuredAddress = bridgeConfig.address();
    if (configuredAddress != null && !configuredAddress.isBlank()) {
      Thread soapAnnouncer =
          new Thread(() -> announceSoapReadyWhenBound(endpointPublisher), "bridge-soap-announcer");
      soapAnnouncer.setDaemon(true);
      soapAnnouncer.start();
    }
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

  /**
   * Waits (bounded) for the config system to report the actual bound port, then emits the ready
   * line. The ValueRegistryConfigSource override is written by the Vert.x listen handler after the
   * socket is bound — the port is held for the whole wait, so no unheld window exists. On timeout
   * the process fails loudly (non-zero exit) instead of announcing an unverified port; the Rust
   * supervisor observes stdout-EOF-before-ready.
   */
  private static void announceWhenBound() {
    long deadline = System.nanoTime() + AWAIT_NANOS;
    int port = -1;
    while (port <= 0 && System.nanoTime() < deadline) {
      port = actualBoundPort();
      if (port > 0) {
        break;
      }
      try {
        Thread.sleep(POLL_MILLIS);
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
        return;
      }
    }
    if (port <= 0) {
      System.err.println(
          "PortAnnouncer: actual bound port not available from the config system after 60s"
              + " — aborting instead of announcing an unverified port");
      System.exit(1);
      return;
    }
    String line = "{\"status\":\"ready\",\"port\":" + port + "}";
    ANNOUNCEMENTS.add(line);
    System.out.println(line);
    System.out.flush();
  }

  /**
   * Waits (bounded, same poll pattern as {@link #announceWhenBound}) for the startup-published SOAP
   * listener to report its actual bound port, then emits EXACTLY ONE {@code
   * {"status":"soap-ready","port":M}} line and flushes. {@code M} is the real bound port from
   * {@link SoapEndpointPublisher#soapPort()} — never an unverified or probed number. On timeout the
   * process fails loudly (non-zero exit) instead of announcing an unverified port.
   */
  private static void announceSoapReadyWhenBound(SoapEndpointPublisher publisher) {
    long deadline = System.nanoTime() + AWAIT_NANOS;
    int port = -1;
    while (port <= 0 && System.nanoTime() < deadline) {
      port = publisher.soapPort().orElse(-1);
      if (port > 0) {
        break;
      }
      try {
        Thread.sleep(POLL_MILLIS);
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
        return;
      }
    }
    if (port <= 0) {
      System.err.println(
          "PortAnnouncer: SOAP listener did not report its actual bound port after 60s"
              + " — aborting instead of announcing an unverified port");
      System.exit(1);
      return;
    }
    String line = "{\"status\":\"soap-ready\",\"port\":" + port + "}";
    ANNOUNCEMENTS.add(line);
    System.out.println(line);
    System.out.flush();
  }

  /**
   * The actual bound port as resolved by the config system: SSL port when TLS is active, plain HTTP
   * port otherwise. {@code -1} when nothing relevant is bound yet.
   */
  static int actualBoundPort() {
    int ssl = configValue("quarkus.http.ssl-port");
    if (ssl > 0) {
      return ssl;
    }
    return configValue("quarkus.http.port");
  }

  private static int configValue(String key) {
    return ConfigProvider.getConfig().getOptionalValue(key, Integer.class).orElse(-1);
  }
}
