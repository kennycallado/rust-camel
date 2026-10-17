package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import cxf_bridge.SoapRequest;
import cxf_bridge.SoapResponse;
import io.grpc.Status;
import io.grpc.stub.StreamObserver;
import io.vertx.core.MultiMap;
import io.vertx.core.Vertx;
import io.vertx.core.http.HttpServer;
import jakarta.xml.ws.Dispatch;
import java.io.StringReader;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import javax.xml.transform.Source;
import javax.xml.transform.stream.StreamSource;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.CsvSource;
import org.w3c.dom.Document;

class CxfClientManagerWireTest {
  private static final String NS = "http://example.com/hello";
  private static final String SOAP11 = "http://schemas.xmlsoap.org/soap/envelope/";
  private static final String SOAP12 = "http://www.w3.org/2003/05/soap-envelope";
  private static final String ONEWAY_NS = "urn:oneway:363";
  private final List<CapturedRequest> captured = new CopyOnWriteArrayList<>();
  private CountDownLatch requestLatch;
  private CxfClientManager manager;
  private SecurityProfileStore profileStore;
  private CxfBridgeService service;
  private Vertx vertx;
  private HttpServer server;

  private record CapturedRequest(MultiMap headers, String body) {}

  @BeforeEach
  void setUp() {
    manager = new CxfClientManager();
    profileStore = mock(SecurityProfileStore.class);
    when(profileStore.getProfile("plain")).thenReturn(SecurityProfile.builder("plain").build());
    manager.profileStore = profileStore;

    BridgeConfig bridgeConfig = mock(BridgeConfig.class);
    when(bridgeConfig.connectionTimeoutMs()).thenReturn(5000);
    service = new CxfBridgeService();
    service.bridgeConfig = bridgeConfig;
    service.clientManager = manager;
    service.profileStore = profileStore;

    vertx = Vertx.vertx();
    requestLatch = new CountDownLatch(1);
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

  @ParameterizedTest
  @CsvSource({
    "receiveMessage, urn:receive, 200",
    "receiveMessage, urn:receive, 202",
    "echoRequest, urn:echo, 200",
    "echoRequest, urn:echo, 202"
  })
  void emptyAcknowledgmentsFollowWsdlShapeAndSendOnce(String element, String action, int status)
      throws Exception {
    startEmptyCapture(status);
    String wsdl = Path.of(getClass().getResource("/soap-oneway.wsdl").toURI()).toString();
    String payload = "<ns:" + element + " xmlns:ns=\"" + ONEWAY_NS + "\">hi</ns:" + element + ">";
    SoapRequest request =
        SoapRequest.newBuilder()
            .setSecurityProfile("plain")
            .setWsdlPath(wsdl)
            .setServiceName("{urn:oneway:363}OneWayService")
            .setPortName("{urn:oneway:363}MainPort")
            .setAddress("http://127.0.0.1:" + server.actualPort() + "/service")
            .setOperation("")
            .setPayload(com.google.protobuf.ByteString.copyFromUtf8(payload))
            .setTimeoutMs(2000L)
            .build();
    RecordingObserver observer = new RecordingObserver();

    service.invoke(request, observer);

    if ("receiveMessage".equals(element)) {
      assertNull(observer.error.get(), "one-way receive must succeed: " + observer.error.get());
      assertEquals(1, observer.responses.size(), "exactly one empty acknowledgment");
      SoapResponse ack = observer.responses.get(0);
      assertFalse(ack.getFault(), "the one-way acknowledgment must not be a fault");
      assertEquals(0, ack.getPayload().size(), "one-way acknowledgment carries empty bytes");
      assertTrue(observer.completed.get(), "the one-way call must complete");
    } else {
      // An empty request-reply is never a non-fault success. A 200 with no body surfaces through
      // CXF's structured fault path; a 202/empty Source is rejected as gRPC INTERNAL by the new
      // empty-content guards.
      Throwable error = observer.error.get();
      if (error != null) {
        assertNotEquals(Status.Code.OK, Status.fromThrowable(error).getCode());
        assertTrue(observer.responses.isEmpty(), "no response may be fabricated");
      } else {
        assertEquals(1, observer.responses.size(), "echo must not be an empty success");
        assertTrue(observer.responses.get(0).getFault(), "echo must surface a fault, not an ACK");
      }
    }

    assertTrue(requestLatch.await(5, TimeUnit.SECONDS), "the server must record the POST");
    assertEquals(1, captured.size(), "each send must record exactly one POST");
    CapturedRequest wire = captured.getFirst();
    assertEquals(action, unquote(wire.headers().get("SOAPAction")));
    Document document = SoapEnvelopeHelper.parseResponse(wire.body());
    assertEquals(SOAP11, document.getDocumentElement().getNamespaceURI());
    assertEquals("Envelope", document.getDocumentElement().getLocalName());
    assertEquals(1, document.getElementsByTagNameNS("*", "Envelope").getLength());
    assertEquals(1, document.getElementsByTagNameNS(SOAP11, "Body").getLength());
    var body = SoapEnvelopeHelper.directBodyElement(document);
    assertNotNull(body);
    var payloadElement = document.getElementsByTagNameNS(ONEWAY_NS, element);
    assertEquals(1, payloadElement.getLength());
    assertSame(
        body, payloadElement.item(0).getParentNode(), "PAYLOAD must remain a direct Body child");
  }

  private void startEmptyCapture(int status) throws Exception {
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
                          requestLatch.countDown();
                          request
                              .response()
                              .setStatusCode(status)
                              .putHeader("Content-Type", "text/xml; charset=UTF-8")
                              .end();
                        }))
            .listen(0, "127.0.0.1")
            .toCompletionStage()
            .toCompletableFuture()
            .get(5, TimeUnit.SECONDS);
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

  private static final class RecordingObserver implements StreamObserver<SoapResponse> {
    final List<SoapResponse> responses = new ArrayList<>();
    final AtomicReference<Throwable> error = new AtomicReference<>();
    final AtomicBoolean completed = new AtomicBoolean(false);

    @Override
    public void onNext(SoapResponse value) {
      responses.add(value);
    }

    @Override
    public void onError(Throwable t) {
      error.set(t);
    }

    @Override
    public void onCompleted() {
      completed.set(true);
    }
  }
}
