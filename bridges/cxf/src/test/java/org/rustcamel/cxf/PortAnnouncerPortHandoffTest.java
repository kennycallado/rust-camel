package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;

import io.quarkus.test.junit.QuarkusTest;
import io.quarkus.test.junit.QuarkusTestProfile;
import io.quarkus.test.junit.TestProfile;
import java.io.InputStream;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.security.KeyStore;
import java.security.cert.CertificateFactory;
import java.util.Map;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.TrustManagerFactory;
import org.junit.jupiter.api.Test;

/**
 * Race-closure test for the Quarkus port env-handoff (bd rc-s7dyw, ADR-0070 bind-read-drop class).
 * With ssl-port=0 the JVM owns port selection: the announced port must be the ACTUAL bound port
 * (registry agreement), must be HELD by this JVM (rebinding fails — no unheld window), and must
 * serve TLS.
 *
 * <p>Twin-copy of bridges/jms and bridges/xml PortAnnouncerPortHandoffTest: keep in lockstep.
 */
@QuarkusTest
@TestProfile(PortAnnouncerPortHandoffTest.SslPortZeroProfile.class)
public class PortAnnouncerPortHandoffTest {

  /** Mirrors the production port handoff: supervisor passes 0, JVM binds actual. */
  public static class SslPortZeroProfile implements QuarkusTestProfile {
    @Override
    public Map<String, String> getConfigOverrides() {
      return Map.of(
          "quarkus.http.ssl-port", "0",
          "quarkus.http.insecure-requests", "disabled",
          "quarkus.http.ssl.client-auth", "none");
    }
  }

  @Test
  public void readyLineAnnouncesActualBoundPort() {
    assertFalse(PortAnnouncer.ANNOUNCEMENTS.isEmpty(), "StartupEvent must have announced");
    String line = PortAnnouncer.ANNOUNCEMENTS.get(0);
    assertTrue(line.startsWith("{\"status\":\"ready\",\"port\":"), "line: " + line);
    assertTrue(line.endsWith("}"), "line: " + line);
    int announced = announcedPort();
    assertTrue(announced > 0, "announced port must be the actual port, line: " + line);

    // The config system must agree: ValueRegistryConfigSource resolves
    // quarkus.http.ssl-port to the actual bound port after the listen handler.
    assertEquals(
        org.eclipse.microprofile.config.ConfigProvider.getConfig()
            .getValue("quarkus.http.ssl-port", Integer.class),
        announced,
        "config must resolve the actual bound port");
    assertEquals(PortAnnouncer.actualBoundPort(), announced, "resolver agrees with the ready line");
  }

  @Test
  public void announcedPortIsHeldByThisJvm() {
    int port = announcedPort();
    try (ServerSocket probe = new ServerSocket()) {
      probe.bind(new InetSocketAddress("127.0.0.1", port));
      fail("announced port " + port + " must be held: rebinding succeeded");
    } catch (java.net.BindException expected) {
      // The Quarkus listener holds the port: the number is never free while
      // known — the bind-read-drop race cannot occur.
    } catch (Exception e) {
      fail("unexpected failure probing port " + port + ": " + e);
    }
  }

  @Test
  public void announcedPortServesTls() throws Exception {
    int port = announcedPort();
    SSLContext ctx = trustTestCa();
    try (SSLSocket socket = (SSLSocket) ctx.getSocketFactory().createSocket("127.0.0.1", port)) {
      socket.setSoTimeout(5_000);
      socket.startHandshake();
    }
  }

  private static int announcedPort() {
    long deadline = System.currentTimeMillis() + 30_000;
    while (PortAnnouncer.ANNOUNCEMENTS.isEmpty() && System.currentTimeMillis() < deadline) {
      try {
        Thread.sleep(10);
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
        break;
      }
    }
    assertFalse(PortAnnouncer.ANNOUNCEMENTS.isEmpty(), "ready line must be announced");
    String line = PortAnnouncer.ANNOUNCEMENTS.get(0);
    return Integer.parseInt(line.substring(line.lastIndexOf(':') + 1, line.length() - 1));
  }

  private static SSLContext trustTestCa() throws Exception {
    CertificateFactory cf = CertificateFactory.getInstance("X.509");
    KeyStore ks = KeyStore.getInstance(KeyStore.getDefaultType());
    ks.load(null, null);
    try (InputStream ca =
        Thread.currentThread().getContextClassLoader().getResourceAsStream("tls/test-ca.pem")) {
      assertNotNull(ca, "tls/test-ca.pem must be on the classpath");
      ks.setCertificateEntry("ca", cf.generateCertificate(ca));
    }
    TrustManagerFactory tmf =
        TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
    tmf.init(ks);
    SSLContext ctx = SSLContext.getInstance("TLS");
    ctx.init(null, tmf.getTrustManagers(), null);
    return ctx;
  }
}
