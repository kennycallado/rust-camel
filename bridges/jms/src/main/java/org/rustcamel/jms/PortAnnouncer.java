package org.rustcamel.jms;

import io.quarkus.runtime.StartupEvent;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.enterprise.event.Observes;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CopyOnWriteArrayList;
import org.eclipse.microprofile.config.ConfigProvider;
import org.eclipse.microprofile.config.inject.ConfigProperty;

/**
 * Announces the bridge's actual bound SSL port on stdout as one JSON line:
 * {"status":"ready","port":N}. The Rust supervisor (camel-bridge process.rs) blocks on this line
 * and connects its mTLS channel to the announced port.
 *
 * <p>Port handoff (bd rc-s7dyw): the supervisor passes QUARKUS_HTTP_SSL_PORT=0, the JVM binds an
 * OS-assigned port — held by the JVM from the first moment the port exists — and the ready line
 * reports the ACTUAL bound port. This closes the ADR-0070 bind-read-drop race: no window exists in
 * which the port number is known but unheld. A fixed port (&gt;0, standalone operators) still binds
 * exactly as configured and is announced unchanged. The ready line is emitted only after the port
 * is actually bound — the announce thread waits for the listener, so "ready" means connectable, not
 * merely "startup observer ran".
 *
 * <p>The actual port is read through the config system: once the Vert.x listen handler registers
 * the real port, the ValueRegistryConfigSource resolves {@code quarkus.http.ssl-port} to it (the
 * raw value 0 never survives to the announce). The plain HTTP port is the fallback when TLS is
 * disabled (test profile); -1 (disabled) resolves as "not bound". StartupEvent fires before the
 * HTTP verticle deploys, so waiting from the observer thread itself would deadlock the boot — the
 * wait runs on a small daemon thread instead.
 *
 * <p>Twin-copy of bridges/xml PortAnnouncer (independent gradle builds, repo convention): keep the
 * startup checks in lockstep when either copy changes.
 */
@ApplicationScoped
public class PortAnnouncer {

  /** Ready lines emitted at startup; observable by tests as protocol evidence. */
  static final List<String> ANNOUNCEMENTS = new CopyOnWriteArrayList<>();

  /** Reserved exit code for a startup permanent failure (never a ready exit). */
  static final int PERMANENT_FAILURE_EXIT_CODE = 78;

  private static final long AWAIT_NANOS = 60_000_000_000L;
  private static final long POLL_MILLIS = 5;

  @ConfigProperty(name = "quarkus.http.ssl-port", defaultValue = "8443")
  int sslPort;

  @ConfigProperty(name = "quarkus.tls.bridge.key-store.pem.0.cert")
  String serverCertPath;

  void onStart(@Observes StartupEvent ev) {
    // Journal startup validation runs before the announcer thread: a set-but-invalid directory,
    // broker key, or numeric bound is a startup permanent failure that must never reach readiness.
    try {
      validateJournalStartup(System.getenv());
    } catch (JournalPermanentException e) {
      // Machine-readable permanent-failure signal on stdout, then the reserved exit code. The
      // Rust supervisor (camel-bridge process.rs) parses this line and must never see a ready line.
      System.out.println(permanentFailureLine(e.getMessage()));
      System.out.flush();
      System.exit(PERMANENT_FAILURE_EXIT_CODE);
      return;
    }
    if (sslPort > 0 && serverCertPath != null && serverCertPath.contains("placeholder-")) {
      throw new RuntimeException(
          "Bridge started with placeholder TLS certs — runtime env vars not set. Aborting.");
    }
    Thread announcer = new Thread(PortAnnouncer::announceWhenBound, "bridge-port-announcer");
    announcer.setDaemon(true);
    announcer.start();
  }

  /**
   * Validates the journal startup configuration from an explicit environment. Unset values pass:
   * readiness succeeds and a queue subscribe later fails {@code FAILED_PRECONDITION}. A set but
   * invalid directory, broker key, or numeric bound throws, and {@link #onStart} exits before any
   * ready line.
   *
   * <p>A subscribe-time {@code FAILED_PRECONDITION} is route-only and never emits this startup
   * signal; only {@link #onStart} does, once, before readiness.
   *
   * @param env the environment to read; production passes {@code System.getenv()}
   * @throws JournalPermanentException when a set journal value is invalid
   */
  static void validateJournalStartup(Map<String, String> env) throws JournalPermanentException {
    JournalService.validateStartup(
        env.get(JournalService.JOURNAL_DIRECTORY_ENV), env.get(JournalService.BROKER_KEY_ENV));
    // The four numeric journal bounds are frozen at startup too: an out-of-range or non-numeric
    // value is a permanent configuration failure, not a per-route error. The accessors validate on
    // read and throw; their parsed values are not needed here.
    BridgeConfig config = new BridgeConfig(env);
    config.journalRetentionMs();
    config.journalLockWaitMs();
    config.journalMaxBytes();
    config.maxDeliveryAttempts();
  }

  /**
   * Builds the one-line startup permanent-failure signal. The reason is JSON-escaped so a bridge
   * message can never break the protocol line; it names the environment variable only and never
   * echoes the raw setting.
   *
   * @param reason the failure reason; may be {@code null}
   * @return the {@code {"status":"permanent-failure","reason":"..."}} line
   */
  static String permanentFailureLine(String reason) {
    String text = reason == null ? "bridge startup failed" : reason;
    return "{\"status\":\"permanent-failure\",\"reason\":\"" + jsonEscape(text) + "\"}";
  }

  /** Escapes one string for embedding in a JSON string literal. */
  private static String jsonEscape(String value) {
    StringBuilder out = new StringBuilder(value.length() + 16);
    for (int i = 0; i < value.length(); i++) {
      char c = value.charAt(i);
      switch (c) {
        case '"' -> out.append("\\\"");
        case '\\' -> out.append("\\\\");
        case '\n' -> out.append("\\n");
        case '\r' -> out.append("\\r");
        case '\t' -> out.append("\\t");
        case '\b' -> out.append("\\b");
        case '\f' -> out.append("\\f");
        default -> {
          if (c < 0x20) {
            out.append(String.format("\\u%04x", (int) c));
          } else {
            out.append(c);
          }
        }
      }
    }
    return out.toString();
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
