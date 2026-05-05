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
import java.util.List;
import java.util.Map;
import java.util.concurrent.Callable;
import java.util.concurrent.CompletableFuture;
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

    // Capture body handler before triggering request handler
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<Buffer>> bodyHandlerCaptor = ArgumentCaptor.forClass(Handler.class);
    when(httpRequest.bodyHandler(bodyHandlerCaptor.capture())).thenReturn(httpRequest);

    // Trigger request handler → sets body handler
    requestHandlerCaptor.getValue().handle(httpRequest);

    // Trigger body handler → calls executeBlocking → captures callable
    bodyHandlerCaptor.getValue().handle(bodyBuffer);

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
    ArgumentCaptor<Handler<Buffer>> bodyHandlerCaptor = ArgumentCaptor.forClass(Handler.class);
    when(httpRequest.bodyHandler(bodyHandlerCaptor.capture())).thenReturn(httpRequest);

    requestHandlerCaptor.getValue().handle(httpRequest);
    bodyHandlerCaptor.getValue().handle(bodyBuffer);

    @SuppressWarnings("unchecked")
    Callable<String> callable = callableCaptor.getValue();
    assertThrows(Exception.class, callable::call);
  }
}
