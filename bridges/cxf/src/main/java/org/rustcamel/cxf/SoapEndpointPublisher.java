package org.rustcamel.cxf;

import com.google.protobuf.ByteString;
import cxf_bridge.ConsumerRequest;
import cxf_bridge.ConsumerResponse;
import io.vertx.core.Vertx;
import io.vertx.core.http.HttpMethod;
import io.vertx.core.http.HttpServer;
import io.vertx.core.http.HttpServerOptions;
import jakarta.annotation.PreDestroy;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;
import java.io.File;
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

  @Inject BridgeConfig bridgeConfig;

  @Inject CxfServerManager cxfServerManager;

  @Inject WssSecurityProcessor wssProcessor;

  @Inject Vertx vertx;

  private HttpServer server;

  synchronized void publish() {
    if (server != null) {
      return;
    }

    try {
      String wsdlPath = bridgeConfig.wsdlPath();
      if (wsdlPath == null || wsdlPath.isBlank()) {
        LOG.info("skipping endpoint publication (producer-only mode)");
        return;
      }
      File wsdlFile = new File(wsdlPath);
      if (!wsdlFile.exists()) {
        throw new IllegalStateException("WSDL file not found: " + wsdlPath);
      }

      String configuredAddress = bridgeConfig.address();
      String address =
          configuredAddress != null && !configuredAddress.isBlank()
              ? configuredAddress
              : DEFAULT_ADDRESS;
      URI uri = URI.create(address);
      String host = uri.getHost();
      int port = uri.getPort();
      String path = uri.getPath();
      if (host == null || host.isBlank()) {
        throw new IllegalStateException("CXF address must include host: " + address);
      }
      if (port <= 0) {
        throw new IllegalStateException("CXF address must include explicit port: " + address);
      }
      if (path == null || path.isBlank()) {
        path = "/";
      }

      String finalPath = path;
      HttpServer httpServer = vertx.createHttpServer(new HttpServerOptions());
      httpServer.requestHandler(
          req -> {
            if (!finalPath.equals(req.path())) {
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

            req.bodyHandler(
                body -> {
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
                                        h -> h.getKey().toLowerCase(),
                                        Map.Entry::getValue,
                                        (a, b) -> a));

                        String soapAction = extractSoapAction(headers);
                        String soapVersion = detectSoapVersion(headers);

                        ConsumerRequest consumerRequest =
                            ConsumerRequest.newBuilder()
                                .setRequestId(UUID.randomUUID().toString())
                                .setOperation(soapAction)
                                .setPayload(ByteString.copyFromUtf8(requestBody))
                                .putAllHeaders(headers)
                                .setSoapAction(soapAction)
                                .build();

                        ConsumerResponse response =
                            cxfServerManager
                                .handleSoapRequest(consumerRequest)
                                .get(bridgeConfig.consumerTimeoutMs(), TimeUnit.MILLISECONDS);

                        String responseXml;
                        if (response.getFault()) {
                          String faultBody =
                              buildFaultBody(
                                  response.getFaultCode(), response.getFaultString(), soapVersion);
                          responseXml = wrapEnvelope(faultBody, soapVersion);
                        } else {
                          responseXml =
                              wrapEnvelope(response.getPayload().toStringUtf8(), soapVersion);
                        }

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
                                  || (cause != null
                                      && cause.getCause() instanceof WSSecurityException);

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
