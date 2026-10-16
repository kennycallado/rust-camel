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
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.TrustManagerFactory;
import org.junit.jupiter.api.Test;

/**
 * Race-closure test for the Quarkus port env-handoff (bd rc-s7dyw, ADR-0070 bind-read-drop class).
 * With ssl-port=0 the JVM owns port selection: the announced port must be the ACTUAL bound port
 * (registry agreement), must be HELD by this JVM (rebinding fails — no unheld window), and must
 * serve TLS. A configured SOAP bind adds the {@code soap-ready} line with the SOAP listener's
 * actual bound port, emitted at startup with no consumer RPC (Task 2.7).
 *
 * <p>Twin-copy of bridges/jms and bridges/xml PortAnnouncerPortHandoffTest: keep in lockstep. The
 * soap-ready line is CXF-only; line ORDER between {@code ready} and {@code soap-ready} is not
 * guaranteed (independent startup observers), so every lookup selects by status prefix.
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
          "quarkus.http.ssl.client-auth", "none",
          // A configured SOAP bind (port 0 = OS-assigned, announced actual): the soap-ready
          // line must appear at startup with no OpenConsumerStream RPC.
          "cxf.address", "http://127.0.0.1:0/cxf");
    }
  }

  @Test
  public void readyLineAnnouncesActualBoundPort() {
    String line = announcement("{\"status\":\"ready\"");
    assertTrue(line.endsWith("}"), "line: " + line);
    int announced = announcedPort(line);
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
    int port = announcedPort(announcement("{\"status\":\"ready\""));
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
    int port = announcedPort(announcement("{\"status\":\"ready\""));
    SSLContext ctx = trustTestCa();
    try (SSLSocket socket = (SSLSocket) ctx.getSocketFactory().createSocket("127.0.0.1", port)) {
      socket.setSoTimeout(5_000);
      socket.startHandshake();
    }
  }

  /**
   * Task 2.7: exactly one {@code soap-ready} line carrying the SOAP listener's REAL bound port,
   * emitted at startup without any {@code OpenConsumerStream} RPC. The announced port serves this
   * JVM's SOAP listener (connected directly — no reserve-bind-drop probe).
   */
  @Test
  public void emitsSoapReadyLine() throws Exception {
    List<String> soapLines = new ArrayList<>();
    long deadline = System.currentTimeMillis() + 30_000;
    while (System.currentTimeMillis() < deadline) {
      for (String line : PortAnnouncer.ANNOUNCEMENTS) {
        if (line.startsWith("{\"status\":\"soap-ready\"") && !soapLines.contains(line)) {
          soapLines.add(line);
        }
      }
      if (!soapLines.isEmpty()) {
        break;
      }
      Thread.sleep(10);
    }
    assertEquals(1, soapLines.size(), "exactly one soap-ready line: " + soapLines);
    String line = soapLines.get(0);
    assertTrue(line.endsWith("}"), "line: " + line);
    int port = announcedPort(line);
    assertTrue(port > 0, "soap-ready must carry the actual bound port, line: " + line);

    // The announced port is the REAL bound port: it serves this JVM's SOAP listener.
    // No consumer stream exists in this test — the line must not depend on one.
    java.net.http.HttpClient client = java.net.http.HttpClient.newHttpClient();
    java.net.http.HttpResponse<String> response =
        client.send(
            java.net.http.HttpRequest.newBuilder()
                .uri(java.net.URI.create("http://127.0.0.1:" + port + "/cxf/unknown-profile"))
                .GET()
                .build(),
            java.net.http.HttpResponse.BodyHandlers.ofString());
    assertEquals(
        404,
        response.statusCode(),
        "the announced SOAP port must serve this JVM's listener (unknown profile → 404)");
  }

  /** The (single) announcement line with the given {@code {"status":...} prefix. */
  private static String announcement(String prefix) {
    long deadline = System.currentTimeMillis() + 30_000;
    while (System.currentTimeMillis() < deadline) {
      for (String line : PortAnnouncer.ANNOUNCEMENTS) {
        if (line.startsWith(prefix)) {
          return line;
        }
      }
      try {
        Thread.sleep(10);
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
        break;
      }
    }
    assertFalse(PortAnnouncer.ANNOUNCEMENTS.isEmpty(), "announcements must be emitted");
    fail("no announcement with prefix " + prefix + " in " + PortAnnouncer.ANNOUNCEMENTS);
    return null;
  }

  private static int announcedPort(String line) {
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
