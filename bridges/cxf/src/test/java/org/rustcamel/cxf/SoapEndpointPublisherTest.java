package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import com.google.protobuf.ByteString;
import cxf_bridge.ConsumerResponse;
import io.vertx.core.AsyncResult;
import io.vertx.core.Future;
import io.vertx.core.Handler;
import io.vertx.core.MultiMap;
import io.vertx.core.buffer.Buffer;
import io.vertx.core.http.HttpMethod;
import io.vertx.core.http.HttpServer;
import io.vertx.core.http.HttpServerRequest;
import io.vertx.core.http.HttpServerResponse;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Callable;
import java.util.concurrent.CompletableFuture;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.mockito.ArgumentCaptor;
import org.mockito.Mock;
import org.mockito.MockitoAnnotations;

class SoapEndpointPublisherTest {

  @Mock BridgeConfig bridgeConfig;
  @Mock CxfServerManager cxfServerManager;
  @Mock SecurityProfileStore profileStore;
  @Mock io.vertx.core.Vertx vertx;
  @Mock HttpServer httpServer;
  @Mock HttpServerRequest httpRequest;
  @Mock HttpServerResponse httpResponse;
  @Mock MultiMap headers;
  @Mock Buffer bodyBuffer;

  SoapEndpointPublisher publisher;

  private static final String TEST_PROFILE_NAME = "test_profile";

  private static Path keystorePath;

  @BeforeAll
  static void setUpKeystore() throws Exception {
    keystorePath = TestKeystoreHelper.createTestKeystore();
  }

  @AfterAll
  static void tearDownKeystore() throws Exception {
    if (keystorePath != null) {
      Files.deleteIfExists(keystorePath);
    }
  }

  @BeforeEach
  void setUp() throws Exception {
    MockitoAnnotations.openMocks(this);

    publisher = new SoapEndpointPublisher();
    publisher.bridgeConfig = bridgeConfig;
    publisher.cxfServerManager = cxfServerManager;
    publisher.profileStore = profileStore;
    publisher.vertx = vertx;

    when(bridgeConfig.address()).thenReturn("http://0.0.0.0:9000/cxf");
    when(bridgeConfig.connectionTimeoutMs()).thenReturn(5000);
    when(bridgeConfig.consumerTimeoutMs()).thenReturn(5000);

    // Default profile — no security
    SecurityProfile testProfile = SecurityProfile.builder(TEST_PROFILE_NAME).build();
    when(profileStore.getProfile(TEST_PROFILE_NAME)).thenReturn(testProfile);

    when(vertx.createHttpServer(any())).thenReturn(httpServer);
    when(httpServer.requestHandler(any())).thenReturn(httpServer);
    when(httpRequest.path()).thenReturn("/cxf/" + TEST_PROFILE_NAME);
    when(httpRequest.method()).thenReturn(HttpMethod.POST);
    when(httpRequest.headers()).thenReturn(headers);
    when(httpRequest.response()).thenReturn(httpResponse);
    when(httpResponse.setStatusCode(anyInt())).thenReturn(httpResponse);
    when(httpResponse.putHeader(anyString(), anyString())).thenReturn(httpResponse);
  }

  private String triggerRequestFlow(
      String requestXml, SecurityProfile profile, ConsumerResponse response) throws Exception {
    when(profileStore.getProfile(TEST_PROFILE_NAME)).thenReturn(profile);

    CompletableFuture<ConsumerResponse> future = CompletableFuture.completedFuture(response);
    when(cxfServerManager.handleSoapRequest(any())).thenReturn(future);

    when(headers.entries())
        .thenReturn(
            List.of(
                Map.entry("content-type", "text/xml"),
                Map.entry("soapaction", "\"urn:test:Operation\"")));

    when(bodyBuffer.toString(StandardCharsets.UTF_8)).thenReturn(requestXml);

    // Capture executeBlocking callable before triggering body handler
    ArgumentCaptor<Callable> callableCaptor = ArgumentCaptor.forClass(Callable.class);
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<AsyncResult<String>>> resultHandlerCaptor =
        ArgumentCaptor.forClass(Handler.class);
    doAnswer(invocation -> null)
        .when(vertx)
        .executeBlocking(callableCaptor.capture(), resultHandlerCaptor.capture());

    // Simulate successful listen
    doAnswer(
            invocation -> {
              @SuppressWarnings("unchecked")
              Handler<AsyncResult<HttpServer>> handler = invocation.getArgument(2);
              handler.handle(Future.succeededFuture(httpServer));
              return null;
            })
        .when(httpServer)
        .listen(anyInt(), anyString(), any());

    // Capture response end
    ArgumentCaptor<String> responseCaptor = ArgumentCaptor.forClass(String.class);
    when(httpResponse.end(responseCaptor.capture())).thenAnswer(i -> Future.succeededFuture());

    // Publish endpoint
    publisher.publish();

    // Capture request handler
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<HttpServerRequest>> requestHandlerCaptor =
        ArgumentCaptor.forClass(Handler.class);
    verify(httpServer).requestHandler(requestHandlerCaptor.capture());

    // Capture bounded stream handlers before triggering request handler
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<Buffer>> chunkCaptor = ArgumentCaptor.forClass(Handler.class);
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<Void>> endCaptor = ArgumentCaptor.forClass(Handler.class);
    when(httpRequest.handler(chunkCaptor.capture())).thenReturn(httpRequest);
    when(httpRequest.endHandler(endCaptor.capture())).thenReturn(httpRequest);

    // Trigger request handler → registers bounded accumulation handlers
    requestHandlerCaptor.getValue().handle(httpRequest);

    // Feed one chunk + end-of-stream → executeBlocking captured
    chunkCaptor.getValue().handle(Buffer.buffer(requestXml.getBytes(StandardCharsets.UTF_8)));
    endCaptor.getValue().handle((Void) null);

    // Execute the callable directly
    @SuppressWarnings("unchecked")
    Callable<String> callable = callableCaptor.getValue();
    String responseXml = callable.call();

    // Trigger the result handler to complete the response
    @SuppressWarnings("unchecked")
    Handler<AsyncResult<String>> resultHandler = resultHandlerCaptor.getValue();
    resultHandler.handle(Future.succeededFuture(responseXml));

    return responseXml;
  }

  @Test
  void noWssProcessing_whenProfileHasNoSecurity() throws Exception {
    String requestXml =
        "<soapenv:Envelope xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<soapenv:Body><test/></soapenv:Body></soapenv:Envelope>";

    ConsumerResponse response =
        ConsumerResponse.newBuilder()
            .setRequestId("test-1")
            .setPayload(ByteString.copyFromUtf8("<result>ok</result>"))
            .setSecurityProfile(TEST_PROFILE_NAME)
            .build();

    SecurityProfile profile = SecurityProfile.builder(TEST_PROFILE_NAME).build();
    String responseXml = triggerRequestFlow(requestXml, profile, response);
    assertTrue(responseXml.contains("<result>ok</result>"));
  }

  @Test
  void extractProfileName_valid() {
    assertEquals("baleares", SoapEndpointPublisher.extractProfileName("/cxf/baleares"));
    assertEquals("baleares", SoapEndpointPublisher.extractProfileName("/cxf/baleares/"));
    assertEquals("baleares", SoapEndpointPublisher.extractProfileName("/cxf/baleares/service"));
  }

  @Test
  void extractProfileName_noProfile_returnsNull() {
    assertNull(SoapEndpointPublisher.extractProfileName("/cxf"));
    assertNull(SoapEndpointPublisher.extractProfileName("/cxf/"));
    assertNull(SoapEndpointPublisher.extractProfileName("/other/path"));
    assertNull(SoapEndpointPublisher.extractProfileName(null));
  }

  @Test
  void extractProfileName_unknownProfile_returnsName() {
    // extractProfileName doesn't validate — returns the segment
    assertEquals("unknown", SoapEndpointPublisher.extractProfileName("/cxf/unknown"));
  }

  @Test
  void processInbound_exception_propagates() throws Exception {
    SecurityProfile profile =
        SecurityProfile.builder(TEST_PROFILE_NAME).keystore("/nonexistent.jks", "pass").build();
    when(profileStore.getProfile(TEST_PROFILE_NAME)).thenReturn(profile);

    CompletableFuture<ConsumerResponse> future =
        CompletableFuture.completedFuture(
            ConsumerResponse.newBuilder()
                .setRequestId("test-5")
                .setPayload(ByteString.copyFromUtf8("<result>ok</result>"))
                .build());
    when(cxfServerManager.handleSoapRequest(any())).thenReturn(future);

    when(headers.entries())
        .thenReturn(
            List.of(
                Map.entry("content-type", "text/xml"),
                Map.entry("soapaction", "\"urn:test:Operation\"")));

    String requestXml =
        "<soapenv:Envelope xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<soapenv:Body><test/></soapenv:Body></soapenv:Envelope>";
    when(bodyBuffer.toString(StandardCharsets.UTF_8)).thenReturn(requestXml);

    ArgumentCaptor<Callable> callableCaptor = ArgumentCaptor.forClass(Callable.class);
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<AsyncResult<String>>> resultHandlerCaptor =
        ArgumentCaptor.forClass(Handler.class);
    doAnswer(invocation -> null)
        .when(vertx)
        .executeBlocking(callableCaptor.capture(), resultHandlerCaptor.capture());

    doAnswer(
            invocation -> {
              @SuppressWarnings("unchecked")
              Handler<AsyncResult<HttpServer>> handler = invocation.getArgument(2);
              handler.handle(Future.succeededFuture(httpServer));
              return null;
            })
        .when(httpServer)
        .listen(anyInt(), anyString(), any());

    publisher.publish();

    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<HttpServerRequest>> requestHandlerCaptor =
        ArgumentCaptor.forClass(Handler.class);
    verify(httpServer).requestHandler(requestHandlerCaptor.capture());

    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<Buffer>> chunkCaptor = ArgumentCaptor.forClass(Handler.class);
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<Void>> endCaptor = ArgumentCaptor.forClass(Handler.class);
    when(httpRequest.handler(chunkCaptor.capture())).thenReturn(httpRequest);
    when(httpRequest.endHandler(endCaptor.capture())).thenReturn(httpRequest);

    requestHandlerCaptor.getValue().handle(httpRequest);
    chunkCaptor.getValue().handle(Buffer.buffer(requestXml.getBytes(StandardCharsets.UTF_8)));
    endCaptor.getValue().handle((Void) null);

    @SuppressWarnings("unchecked")
    Callable<String> callable = callableCaptor.getValue();
    assertThrows(Exception.class, callable::call);
  }

  @Test
  void publisherReusesProcessorPerProfile() {
    SecurityProfile profile = SecurityProfile.builder(TEST_PROFILE_NAME).build();

    WssSecurityProcessor first = publisher.wssProcessorFor(TEST_PROFILE_NAME, profile);
    WssSecurityProcessor second = publisher.wssProcessorFor(TEST_PROFILE_NAME, profile);
    WssSecurityProcessor other = publisher.wssProcessorFor("other_profile", profile);

    assertSame(first, second, "Same profile name must reuse the cached processor");
    assertNotSame(first, other, "Different profile name must get its own processor");
  }

  @Test
  void replayedRequestRejectedThroughPublishedEndpoint() throws Exception {
    SecurityProfile realProfile =
        SecurityProfile.builder(TEST_PROFILE_NAME)
            .keystore(keystorePath.toString(), "changeit")
            .truststore(keystorePath.toString(), "changeit")
            .sigUser("alice", "changeit")
            .encUser("alice")
            .actionsOut("Timestamp Signature")
            .actionsIn("Timestamp Signature")
            .build();
    when(profileStore.getProfile(TEST_PROFILE_NAME)).thenReturn(realProfile);

    String plainEnvelope =
        "<soapenv:Envelope xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<soapenv:Body><test:Hello xmlns:test=\"http://test.example.com\">World</test:Hello>"
            + "</soapenv:Body></soapenv:Envelope>";
    String signedBody = new WssSecurityProcessor(realProfile).processOutbound(plainEnvelope);
    when(bodyBuffer.toString(StandardCharsets.UTF_8)).thenReturn(signedBody);

    ConsumerResponse okResponse =
        ConsumerResponse.newBuilder()
            .setRequestId("test-replay-1")
            .setPayload(ByteString.copyFromUtf8("<result>ok</result>"))
            .setSecurityProfile(TEST_PROFILE_NAME)
            .build();
    when(cxfServerManager.handleSoapRequest(any()))
        .thenReturn(CompletableFuture.completedFuture(okResponse));

    when(headers.entries())
        .thenReturn(
            List.of(
                Map.entry("content-type", "text/xml"),
                Map.entry("soapaction", "\"urn:test:Operation\"")));

    ArgumentCaptor<Callable> callableCaptor = ArgumentCaptor.forClass(Callable.class);
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<AsyncResult<String>>> resultHandlerCaptor =
        ArgumentCaptor.forClass(Handler.class);
    doAnswer(invocation -> null)
        .when(vertx)
        .executeBlocking(callableCaptor.capture(), resultHandlerCaptor.capture());

    doAnswer(
            invocation -> {
              @SuppressWarnings("unchecked")
              Handler<AsyncResult<HttpServer>> handler = invocation.getArgument(2);
              handler.handle(Future.succeededFuture(httpServer));
              return null;
            })
        .when(httpServer)
        .listen(anyInt(), anyString(), any());

    ArgumentCaptor<String> responseCaptor = ArgumentCaptor.forClass(String.class);
    when(httpResponse.end(responseCaptor.capture())).thenAnswer(i -> Future.succeededFuture());

    publisher.publish();

    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<HttpServerRequest>> requestHandlerCaptor =
        ArgumentCaptor.forClass(Handler.class);
    verify(httpServer).requestHandler(requestHandlerCaptor.capture());

    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<Buffer>> chunkCaptor = ArgumentCaptor.forClass(Handler.class);
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<Void>> endCaptor = ArgumentCaptor.forClass(Handler.class);
    when(httpRequest.handler(chunkCaptor.capture())).thenReturn(httpRequest);
    when(httpRequest.endHandler(endCaptor.capture())).thenReturn(httpRequest);

    // --- First POST: signed body accepted ---
    io.vertx.core.buffer.Buffer signedBuffer =
        Buffer.buffer(signedBody.getBytes(StandardCharsets.UTF_8));
    requestHandlerCaptor.getValue().handle(httpRequest);
    chunkCaptor.getAllValues().get(0).handle(signedBuffer);
    endCaptor.getAllValues().get(0).handle((Void) null);

    @SuppressWarnings("unchecked")
    Callable<String> firstCallable = callableCaptor.getAllValues().get(0);
    String firstResponseXml = firstCallable.call();

    @SuppressWarnings("unchecked")
    Handler<AsyncResult<String>> firstHandler = resultHandlerCaptor.getAllValues().get(0);
    firstHandler.handle(Future.succeededFuture(firstResponseXml));
    verify(httpResponse).setStatusCode(200);

    // --- Second POST: identical bytes must be rejected as a WSS replay ---
    requestHandlerCaptor.getValue().handle(httpRequest);
    chunkCaptor.getAllValues().get(1).handle(signedBuffer);
    endCaptor.getAllValues().get(1).handle((Void) null);

    @SuppressWarnings("unchecked")
    Callable<String> secondCallable = callableCaptor.getAllValues().get(1);
    Exception replayError = assertThrows(Exception.class, secondCallable::call);

    @SuppressWarnings("unchecked")
    Handler<AsyncResult<String>> secondHandler = resultHandlerCaptor.getAllValues().get(1);
    secondHandler.handle(Future.failedFuture(replayError));

    verify(httpResponse).setStatusCode(400);
    String endedBody = responseCaptor.getAllValues().get(1);
    assertTrue(
        endedBody.contains("soap:Client"),
        "Failure branch must emit a soap:Client fault for WSS replay rejection");
  }

  @Test
  void httpsAddressFailsStartup() {
    when(bridgeConfig.address()).thenReturn("https://0.0.0.0:9000/soap");

    IllegalStateException ex = assertThrows(IllegalStateException.class, () -> publisher.publish());

    assertTrue(ex.getMessage().contains("scheme not supported"), ex.getMessage());
    assertTrue(ex.getMessage().contains("https"), ex.getMessage());
    verify(vertx, never()).createHttpServer(any());
  }

  @Test
  void addressWithoutSchemeFailsStartup() {
    when(bridgeConfig.address()).thenReturn("//0.0.0.0:9000/soap");

    IllegalStateException ex = assertThrows(IllegalStateException.class, () -> publisher.publish());

    assertTrue(ex.getMessage().contains("scheme not supported"), ex.getMessage());
    assertTrue(ex.getMessage().contains("(missing)"), ex.getMessage());
    verify(vertx, never()).createHttpServer(any());
  }

  @Test
  void httpAddressStillBinds() {
    when(bridgeConfig.address()).thenReturn("http://127.0.0.1:0/soap");

    doAnswer(
            invocation -> {
              @SuppressWarnings("unchecked")
              Handler<AsyncResult<HttpServer>> handler = invocation.getArgument(2);
              handler.handle(Future.succeededFuture(httpServer));
              return null;
            })
        .when(httpServer)
        .listen(anyInt(), anyString(), any());
    doAnswer(
            invocation -> {
              @SuppressWarnings("unchecked")
              Handler<AsyncResult<Void>> handler = invocation.getArgument(0);
              handler.handle(Future.succeededFuture());
              return null;
            })
        .when(httpServer)
        .close(any());

    assertDoesNotThrow(() -> publisher.publish());
    verify(httpServer).listen(eq(0), eq("127.0.0.1"), any());
    publisher.stop();
  }
}
