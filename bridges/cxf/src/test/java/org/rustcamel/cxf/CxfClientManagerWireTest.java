package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import io.vertx.core.MultiMap;
import io.vertx.core.Vertx;
import io.vertx.core.http.HttpServer;
import jakarta.xml.ws.Dispatch;
import java.io.StringReader;
import java.nio.file.Path;
import java.time.Duration;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.TimeUnit;
import javax.xml.transform.Source;
import javax.xml.transform.stream.StreamSource;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.w3c.dom.Document;

class CxfClientManagerWireTest {
  private static final String NS = "http://example.com/hello";
  private static final String SOAP11 = "http://schemas.xmlsoap.org/soap/envelope/";
  private static final String SOAP12 = "http://www.w3.org/2003/05/soap-envelope";
  private final List<CapturedRequest> captured = new CopyOnWriteArrayList<>();
  private CxfClientManager manager;
  private Vertx vertx;
  private HttpServer server;

  private record CapturedRequest(MultiMap headers, String body) {}

  @BeforeEach
  void setUp() {
    manager = new CxfClientManager();
    manager.profileStore = mock(SecurityProfileStore.class);
    when(manager.profileStore.getProfile("plain"))
        .thenReturn(SecurityProfile.builder("plain").build());
    vertx = Vertx.vertx();
  }

  @AfterEach
  void tearDown() throws Exception {
    manager.close();
    vertx.close().toCompletionStage().toCompletableFuture().get(5, TimeUnit.SECONDS);
  }

  @Test
  void soap11UsesDeclaredActionDifferentFromOperation() throws Exception {
    startCapture(SOAP11);
    invoke("HelloPort", "sayHello");
    CapturedRequest request = assertSingleEnvelope(SOAP11);
    assertTrue(request.headers().get("Content-Type").startsWith("text/xml"));
    assertEquals("urn:mission361:hello-action", unquote(request.headers().get("SOAPAction")));
  }

  @Test
  void soap11MissingDeclaredActionUsesEmptyAction() throws Exception {
    startCapture(SOAP11);
    invoke("NoActionPort", "sayHello");
    CapturedRequest request = assertSingleEnvelope(SOAP11);
    assertEquals("", unquote(request.headers().get("SOAPAction")));
  }

  @Test
  void soap12UsesDeclaredBindingAction() throws Exception {
    startCapture(SOAP12);
    invoke("Hello12Port", "sayHello");
    CapturedRequest request = assertSingleEnvelope(SOAP12);
    String contentType = request.headers().get("Content-Type");
    assertTrue(contentType.startsWith("application/soap+xml"), contentType);
    String action =
        List.of(contentType.split(";")).stream()
            .map(String::trim)
            .filter(parameter -> parameter.startsWith("action="))
            .map(parameter -> unquote(parameter.substring("action=".length())))
            .findFirst()
            .orElseThrow(() -> new AssertionError("missing Content-Type action: " + contentType));
    assertEquals("urn:mission361:hello12-action", action);
    assertNull(request.headers().get("SOAPAction"), "SOAP 1.2 must not add a SOAPAction header");
  }

  @Test
  void unknownOperationDoesNotPretendEmptySoapAction() throws Exception {
    startCapture(SOAP11);
    assertTimeoutPreemptively(
        Duration.ofSeconds(5),
        () -> {
          IllegalArgumentException error =
              assertThrows(
                  IllegalArgumentException.class, () -> invoke("HelloPort", "noSuchOperation"));
          assertTrue(error.getMessage().contains("noSuchOperation"), error.getMessage());
          assertTrue(error.getMessage().contains("binding"), error.getMessage());
        });
    assertTrue(captured.isEmpty(), "unknown operation must fail before any wire transmission");
    assertEquals(0, manager.cacheSize(), "invalid operation must not publish a dispatch");
  }

  private void startCapture(String soapNamespace) throws Exception {
    server =
        vertx
            .createHttpServer()
            .requestHandler(
                request ->
                    request.bodyHandler(
                        body -> {
                          captured.add(
                              new CapturedRequest(
                                  MultiMap.caseInsensitiveMultiMap().addAll(request.headers()),
                                  body.toString()));
                          request
                              .response()
                              .putHeader(
                                  "Content-Type",
                                  soapNamespace.equals(SOAP12)
                                      ? "application/soap+xml; charset=UTF-8"
                                      : "text/xml; charset=UTF-8")
                              .end(
                                  "<s:Envelope xmlns:s=\""
                                      + soapNamespace
                                      + "\"><s:Body>"
                                      + "<sayHelloResponse xmlns=\""
                                      + NS
                                      + "\">Hello World</sayHelloResponse>"
                                      + "</s:Body></s:Envelope>");
                        }))
            .listen(0, "127.0.0.1")
            .toCompletionStage()
            .toCompletableFuture()
            .get(5, TimeUnit.SECONDS);
  }

  private void invoke(String port, String operation) throws Exception {
    String wsdl = Path.of(getClass().getResource("/soap-actions.wsdl").toURI()).toString();
    Dispatch<Source> dispatch =
        manager.getDispatch(
            wsdl,
            "http://127.0.0.1:" + server.actualPort() + "/service",
            "{" + NS + "}HelloService",
            "{" + NS + "}" + port,
            operation,
            "plain",
            2000L);
    Source response =
        dispatch.invoke(
            new StreamSource(
                new StringReader("<sayHelloRequest xmlns=\"" + NS + "\">World</sayHelloRequest>")));
    assertTrue(SoapEnvelopeHelper.sourceToString(response).contains("Hello World"));
  }

  private CapturedRequest assertSingleEnvelope(String soapNamespace) throws Exception {
    assertEquals(1, captured.size());
    CapturedRequest request = captured.getFirst();
    Document document = SoapEnvelopeHelper.parseResponse(request.body());
    assertEquals(soapNamespace, document.getDocumentElement().getNamespaceURI());
    assertEquals("Envelope", document.getDocumentElement().getLocalName());
    assertEquals(1, document.getElementsByTagNameNS("*", "Envelope").getLength());
    assertEquals(1, document.getElementsByTagNameNS(soapNamespace, "Body").getLength());
    var body = SoapEnvelopeHelper.directBodyElement(document);
    assertNotNull(body);
    var payload = document.getElementsByTagNameNS(NS, "sayHelloRequest");
    assertEquals(1, payload.getLength());
    assertSame(body, payload.item(0).getParentNode(), "PAYLOAD must remain a direct Body child");
    return request;
  }

  private static String unquote(String value) {
    assertNotNull(value, "expected action header/parameter on the wire");
    return value.startsWith("\"") && value.endsWith("\"")
        ? value.substring(1, value.length() - 1)
        : value;
  }
}
