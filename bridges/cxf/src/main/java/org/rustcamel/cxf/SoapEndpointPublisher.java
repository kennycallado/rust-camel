package org.rustcamel.cxf;

import com.google.protobuf.ByteString;
import cxf_bridge.ConsumerRequest;
import cxf_bridge.ConsumerResponse;
import io.vertx.core.Vertx;
import io.vertx.core.buffer.Buffer;
import io.vertx.core.http.HttpMethod;
import io.vertx.core.http.HttpServer;
import io.vertx.core.http.HttpServerOptions;
import io.vertx.core.http.HttpServerRequest;
import jakarta.annotation.PreDestroy;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;
import java.net.URI;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.TimeUnit;
import java.util.logging.Level;
import java.util.logging.Logger;
import org.apache.wss4j.common.ext.WSSecurityException;

@ApplicationScoped
public class SoapEndpointPublisher {
  private static final Logger LOG = Logger.getLogger(SoapEndpointPublisher.class.getName());
  private static final String DEFAULT_ADDRESS = "http://0.0.0.0:9000/cxf";
  private static final String MAX_BODY_BYTES_ENV = "CXF_MAX_BODY_BYTES";
  private static final long DEFAULT_MAX_BODY_BYTES = 16L * 1024 * 1024;

  /** Above the 16 MiB default cap, below the 18 MiB Rust gRPC decode limit. */
  static final long MAX_BODY_BYTES_CEILING = 17L * 1024 * 1024;

  private static final String BODY_LIMIT_REJECT_MESSAGE = "request body exceeds CXF_MAX_BODY_BYTES";

  @Inject BridgeConfig bridgeConfig;

  @Inject SecurityProfileStore profileStore;

  @Inject CxfServerManager cxfServerManager;

  @Inject Vertx vertx;

  private HttpServer server;

  /**
   * Test seam: pins the request-body cap deterministically. {@code -1} (production) lets each
   * request resolve the cap from {@link #maxBodyBytes()} at request time.
   */
  long pinnedMaxBodyBytes = -1;

  private long resolveMaxBodyBytes() {
    return pinnedMaxBodyBytes > 0 ? pinnedMaxBodyBytes : maxBodyBytes();
  }

  /**
   * One WSS processor per security profile: keeps Crypto instances and the inbound replay cache
   * shared across requests of the same profile.
   */
  private final java.util.concurrent.ConcurrentHashMap<String, WssSecurityProcessor>
      wssProcessorsByProfile = new java.util.concurrent.ConcurrentHashMap<>();

  WssSecurityProcessor wssProcessorFor(String profileName, SecurityProfile profile) {
    return wssProcessorsByProfile.computeIfAbsent(
        profileName, n -> new WssSecurityProcessor(profile));
  }

  synchronized void publish() {
    if (server != null) {
      return;
    }

    String configuredAddress = bridgeConfig.address();
    String address =
        configuredAddress != null && !configuredAddress.isBlank()
            ? configuredAddress
            : DEFAULT_ADDRESS;
    URI uri;
    try {
      uri = URI.create(address);
    } catch (IllegalArgumentException e) {
      throw new IllegalStateException("CXF_ADDRESS invalid: " + address, e);
    }
    validateAddressScheme(uri);

    // Startup validation (ADR-0033): a malformed CXF_MAX_BODY_BYTES must fail loud before any
    // socket binds; requests resolve the cap again at request time.
    maxBodyBytes();

    try {
      String host = uri.getHost();
      int port = uri.getPort();
      if (host == null || host.isBlank()) {
        throw new IllegalStateException("CXF address must include host: " + address);
      }
      if (port < 0) {
        throw new IllegalStateException("CXF address must include explicit port: " + address);
      }

      HttpServer httpServer = vertx.createHttpServer(new HttpServerOptions());
      httpServer.requestHandler(
          req -> {
            String profileName = extractProfileName(req.path());
            if (profileName == null) {
              req.response().setStatusCode(404).end();
              return;
            }

            SecurityProfile profile;
            try {
              profile = profileStore.getProfile(profileName);
            } catch (IllegalArgumentException e) {
              req.response().setStatusCode(404).end();
              return;
            }

            if (req.method() == HttpMethod.GET || req.method() == HttpMethod.HEAD) {
              req.response().setStatusCode(200).putHeader("content-type", "text/plain").end("ok");
              return;
            }

            if (req.method() != HttpMethod.POST) {
              req.response().setStatusCode(405).end();
              return;
            }

            WssSecurityProcessor wssProcessor = wssProcessorFor(profileName, profile);

            // Upfront gate: a declared Content-Length over the cap never enters aggregation.
            long maxBodyBytes = resolveMaxBodyBytes();
            String declaredLength = req.getHeader("Content-Length");
            if (declaredLength != null && !declaredLength.isBlank()) {
              try {
                if (Long.parseLong(declaredLength.trim()) > maxBodyBytes) {
                  LOG.log(
                      Level.WARNING,
                      "Rejecting request with declared {0} body bytes on {1}",
                      new Object[] {declaredLength.trim(), req.path()});
                  rejectBodyLimit(req);
                  return;
                }
              } catch (NumberFormatException e) {
                // Malformed header cannot be trusted; the mid-stream gate still protects us.
              }
            }

            // Bounded accumulator: chunk-by-chunk accounting aborts mid-stream once the
            // aggregate would pass the cap, so memory stays bounded even for lying
            // Content-Length headers. Vert.x drives one request's handlers sequentially on a
            // single context, so plain effectively-final captures are sufficient.
            final long[] received = {0};
            final boolean[] rejected = {false};
            final Buffer[] aggregate = {Buffer.buffer()};
            req.handler(
                chunk -> {
                  if (rejected[0]) {
                    return;
                  }
                  received[0] += chunk.length();
                  if (received[0] > maxBodyBytes) {
                    rejected[0] = true;
                    aggregate[0] = null;
                    req.pause();
                    LOG.log(
                        Level.WARNING,
                        "Request streamed past the CXF_MAX_BODY_BYTES cap ({0}); closing",
                        new Object[] {req.path()});
                    rejectBodyLimit(req);
                    return;
                  }
                  aggregate[0].appendBuffer(chunk);
                });
            req.endHandler(
                v -> {
                  if (!rejected[0]) {
                    handleRequestBody(req, wssProcessor, profileName, aggregate[0]);
                  }
                });
          });
      java.util.concurrent.CountDownLatch latch = new java.util.concurrent.CountDownLatch(1);
      java.util.concurrent.atomic.AtomicReference<Throwable> listenError =
          new java.util.concurrent.atomic.AtomicReference<>();
      httpServer.listen(
          port,
          host,
          ar -> {
            if (ar.succeeded()) {
              server = httpServer;
            } else {
              listenError.set(ar.cause());
            }
            latch.countDown();
          });
      latch.await(bridgeConfig.connectionTimeoutMs(), TimeUnit.MILLISECONDS);
      if (listenError.get() != null) {
        throw new IllegalStateException("Failed to publish SOAP endpoint", listenError.get());
      }
      if (server == null) {
        throw new IllegalStateException("Timed out publishing SOAP endpoint");
      }

      LOG.log(Level.INFO, "SOAP endpoint published at {0}", address);
    } catch (Exception e) {
      LOG.log(Level.SEVERE, "SOAP endpoint publish failed", e);
      throw new IllegalStateException("Failed to publish SOAP endpoint", e);
    }
  }

  @PreDestroy
  void stop() {
    if (server != null) {
      HttpServer current = server;
      server = null;
      java.util.concurrent.CountDownLatch latch = new java.util.concurrent.CountDownLatch(1);
      current.close(ar -> latch.countDown());
      try {
        latch.await(2, TimeUnit.SECONDS);
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
      }
      LOG.info("SOAP endpoint stopped");
    }
  }

  /**
   * Rejects listener schemes other than {@code http}: TLS termination is not implemented in this
   * sidecar, so a misconfigured {@code https://} address must fail fast before any socket binds.
   */
  static void validateAddressScheme(URI address) {
    String scheme = address.getScheme();
    if (scheme == null || !scheme.equalsIgnoreCase("http")) {
      throw new IllegalStateException(
          "CXF_ADDRESS scheme not supported: "
              + (scheme == null ? "(missing)" : scheme)
              + "; TLS listener support is not yet available; use http://");
    }
  }

  /**
   * Blocked-pipeline processing of one fully accumulated request body: WSS-in, extract, IPC call to
   * the consumer, wrap, WSS-out, respond.
   */
  private void handleRequestBody(
      HttpServerRequest req, WssSecurityProcessor wssProcessor, String profileName, Buffer body) {
    vertx.executeBlocking(
        () -> {
          String requestXml = body.toString(java.nio.charset.StandardCharsets.UTF_8);

          if (wssProcessor.canVerifyInbound()) {
            requestXml = wssProcessor.processInbound(requestXml);
          }

          String requestBody = extractSoapBody(requestXml);

          Map<String, String> headers =
              req.headers().entries().stream()
                  .collect(
                      java.util.stream.Collectors.toMap(
                          h -> h.getKey().toLowerCase(), Map.Entry::getValue, (a, b) -> a));

          String soapAction = extractSoapAction(headers);
          String soapVersion = detectSoapVersion(headers);

          ConsumerRequest consumerRequest =
              ConsumerRequest.newBuilder()
                  .setRequestId(UUID.randomUUID().toString())
                  .setOperation(soapAction)
                  .setPayload(ByteString.copyFromUtf8(requestBody))
                  .putAllHeaders(headers)
                  .setSoapAction(soapAction)
                  .setSecurityProfile(profileName)
                  .build();

          ConsumerResponse response =
              cxfServerManager
                  .handleSoapRequest(consumerRequest)
                  .get(bridgeConfig.consumerTimeoutMs(), TimeUnit.MILLISECONDS);

          // Defense in depth: assert profile echo matches
          if (!profileName.equals(response.getSecurityProfile())) {
            LOG.log(
                Level.WARNING,
                "Security profile mismatch: expected={0}, got={1}",
                new Object[] {profileName, response.getSecurityProfile()});
          }

          String responseXml;
          if (response.getFault()) {
            String faultBody =
                buildFaultBody(response.getFaultCode(), response.getFaultString(), soapVersion);
            responseXml = wrapEnvelope(faultBody, soapVersion);
          } else {
            responseXml = wrapEnvelope(response.getPayload().toStringUtf8(), soapVersion);
          }

          // Sign with the resolved profile (defense in depth)
          if (wssProcessor.canSignOutbound()) {
            responseXml = wssProcessor.processOutbound(responseXml);
          }

          return responseXml;
        },
        ar -> {
          if (ar.succeeded()) {
            req.response()
                .setStatusCode(200)
                .putHeader("content-type", "text/xml; charset=utf-8")
                .end((String) ar.result());
          } else {
            Throwable cause = ar.cause();
            LOG.log(Level.SEVERE, "SOAP request processing failed", cause);

            boolean isSecurityFailure =
                cause instanceof WSSecurityException
                    || (cause != null && cause.getCause() instanceof WSSecurityException);

            String faultCode = isSecurityFailure ? "soap:Client" : "soap:Server";
            String faultString =
                isSecurityFailure
                    ? "WS-Security processing failed: " + sanitize(cause.getMessage())
                    : "Internal server error";

            req.response()
                .setStatusCode(isSecurityFailure ? 400 : 500)
                .putHeader("content-type", "text/xml; charset=utf-8")
                .end(buildSoapFault(faultCode, faultString));
          }
        });
  }

  /** Aborts a request past the body cap: HTTP 413 with a short plain-text reason. */
  private static void rejectBodyLimit(HttpServerRequest req) {
    req.response().setStatusCode(413).end(BODY_LIMIT_REJECT_MESSAGE);
    req.connection().close();
  }

  /**
   * Reads the listener request-body cap from {@code CXF_MAX_BODY_BYTES} in bytes. Fails loud on a
   * malformed or non-positive value so a typo never silently disables the cap.
   */
  static long maxBodyBytes() {
    String raw = System.getenv(MAX_BODY_BYTES_ENV);
    if (raw == null || raw.isBlank()) {
      return DEFAULT_MAX_BODY_BYTES;
    }
    return parseCap(raw);
  }

  /**
   * Parses one {@code CXF_MAX_BODY_BYTES} value, failing loud with the env name on malformed,
   * non-positive, or above-ceiling input.
   */
  static long parseCap(String raw) {
    try {
      long parsed = Long.parseLong(raw.trim());
      if (parsed <= 0) {
        throw new IllegalStateException(
            MAX_BODY_BYTES_ENV + " must be a positive byte count: " + raw);
      }
      if (parsed > MAX_BODY_BYTES_CEILING) {
        throw new IllegalStateException(
            MAX_BODY_BYTES_ENV
                + " exceeds its "
                + MAX_BODY_BYTES_CEILING
                + "-byte ceiling: "
                + parsed
                + "; caps above 17 MiB invert the decode-limit ordering (cap <= 17 MiB ceiling"
                + " < 18 MiB Rust decode limit), bodies pass this Java cap only to fail at the"
                + " 18 MiB Rust gRPC decode limit");
      }
      return parsed;
    } catch (NumberFormatException e) {
      throw new IllegalStateException(MAX_BODY_BYTES_ENV + " invalid: " + raw, e);
    }
  }

  /**
   * Extracts profile name from URL path. Path format: /cxf/&lt;profile_name&gt;/... Returns null
   * for /cxf, /cxf/, non-matching paths, or null path.
   */
  static String extractProfileName(String path) {
    if (path == null) return null;
    if (!path.startsWith("/cxf")) return null;
    String after = path.substring("/cxf".length());
    if (after.isEmpty()) return null;
    if (!after.startsWith("/")) return null;
    String rest = after.substring(1);
    if (rest.isEmpty()) return null;
    int slash = rest.indexOf('/');
    String segment = slash >= 0 ? rest.substring(0, slash) : rest;
    return segment.isEmpty() ? null : segment;
  }

  private static String sanitize(String msg) {
    if (msg == null) return "unknown";
    return msg.length() > 200 ? msg.substring(0, 200) : msg;
  }

  private static String buildSoapFault(String faultCode, String faultString) {
    return "<soapenv:Envelope xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\">"
        + "<soapenv:Header/><soapenv:Body><soapenv:Fault>"
        + "<faultcode>"
        + escapeXml(faultCode)
        + "</faultcode>"
        + "<faultstring>"
        + escapeXml(faultString)
        + "</faultstring>"
        + "</soapenv:Fault></soapenv:Body></soapenv:Envelope>";
  }

  private static String extractSoapAction(Map<String, String> headers) {
    String action = headers.getOrDefault("soapaction", "");
    if (action != null && !action.isBlank()) {
      return action.replace("\"", "").trim();
    }
    String contentType = headers.getOrDefault("content-type", "");
    int idx = contentType.toLowerCase().indexOf("action=");
    if (idx >= 0) {
      String tail = contentType.substring(idx + "action=".length()).trim();
      int semi = tail.indexOf(';');
      String raw = semi >= 0 ? tail.substring(0, semi) : tail;
      return raw.replace("\"", "").trim();
    }
    return "";
  }

  private static String detectSoapVersion(Map<String, String> headers) {
    String contentType = headers.getOrDefault("content-type", "").toLowerCase();
    if (contentType.contains("application/soap+xml")) {
      return "1.2";
    }
    return "1.1";
  }

  private static String buildFaultBody(String faultCode, String faultString, String soapVersion) {
    String code = escapeXml(faultCode == null || faultCode.isBlank() ? "soap:Server" : faultCode);
    String text = escapeXml(faultString == null ? "SOAP fault" : faultString);
    if ("1.2".equals(soapVersion)) {
      return "<soapenv:Fault xmlns:soapenv=\"http://www.w3.org/2003/05/soap-envelope\">"
          + "<soapenv:Code><soapenv:Value>"
          + code
          + "</soapenv:Value></soapenv:Code>"
          + "<soapenv:Reason><soapenv:Text>"
          + text
          + "</soapenv:Text></soapenv:Reason>"
          + "</soapenv:Fault>";
    }
    return "<soapenv:Fault xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\">"
        + "<faultcode>"
        + code
        + "</faultcode><faultstring>"
        + text
        + "</faultstring></soapenv:Fault>";
  }

  private static String escapeXml(String value) {
    return value
        .replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace("\"", "&quot;")
        .replace("'", "&apos;");
  }

  private static String extractSoapBody(String requestXml) {
    String xml = requestXml == null ? "" : requestXml.trim();
    String lower = xml.toLowerCase();
    int bodyStart = lower.indexOf(":body");
    if (bodyStart < 0) {
      bodyStart = lower.indexOf("<body");
    }
    if (bodyStart < 0) {
      return xml;
    }
    int open = lower.lastIndexOf('<', bodyStart);
    int openEnd = lower.indexOf('>', bodyStart);
    if (open < 0 || openEnd < 0 || openEnd <= open) {
      return xml;
    }
    int close = lower.indexOf("</", openEnd);
    int closeBody = lower.indexOf(":body>", openEnd);
    if (closeBody < 0) {
      closeBody = lower.indexOf("</body>", openEnd);
    }
    if (closeBody < 0) {
      return xml.substring(openEnd + 1).trim();
    }
    int bodyEndOpen = lower.lastIndexOf("</", closeBody);
    if (bodyEndOpen < 0) {
      bodyEndOpen = close;
    }
    if (bodyEndOpen <= openEnd) {
      return "";
    }
    return xml.substring(openEnd + 1, bodyEndOpen).trim();
  }

  private static String wrapEnvelope(String xmlBody, String soapVersion) {
    String ns =
        "1.2".equals(soapVersion)
            ? "http://www.w3.org/2003/05/soap-envelope"
            : "http://schemas.xmlsoap.org/soap/envelope/";
    return "<soapenv:Envelope xmlns:soapenv=\""
        + ns
        + "\"><soapenv:Header/><soapenv:Body>"
        + (xmlBody == null ? "" : xmlBody)
        + "</soapenv:Body></soapenv:Envelope>";
  }
}
