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
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.mockito.ArgumentCaptor;
import org.mockito.Mock;
import org.mockito.MockitoAnnotations;

class SoapEndpointPublisherTest {

  @Mock BridgeConfig bridgeConfig;
  @Mock CxfServerManager cxfServerManager;
  @Mock WssSecurityProcessor wssProcessor;
  @Mock io.vertx.core.Vertx vertx;
  @Mock HttpServer httpServer;
  @Mock HttpServerRequest httpRequest;
  @Mock HttpServerResponse httpResponse;
  @Mock MultiMap headers;
  @Mock Buffer bodyBuffer;

  SoapEndpointPublisher publisher;

  @TempDir Path tempDir;

  @BeforeEach
  void setUp() throws Exception {
    MockitoAnnotations.openMocks(this);

    publisher = new SoapEndpointPublisher();
    publisher.bridgeConfig = bridgeConfig;
    publisher.cxfServerManager = cxfServerManager;
    publisher.wssProcessor = wssProcessor;
    publisher.vertx = vertx;

    Path wsdlFile = tempDir.resolve("test.wsdl");
    Files.writeString(wsdlFile, "<definitions/>");

    when(bridgeConfig.wsdlPath()).thenReturn(wsdlFile.toString());
    when(bridgeConfig.address()).thenReturn("http://0.0.0.0:9000/cxf");
    when(bridgeConfig.connectionTimeoutMs()).thenReturn(5000);
    when(bridgeConfig.consumerTimeoutMs()).thenReturn(5000);

    when(vertx.createHttpServer(any())).thenReturn(httpServer);
    when(httpServer.requestHandler(any())).thenReturn(httpServer);
    when(httpRequest.path()).thenReturn("/cxf");
    when(httpRequest.method()).thenReturn(HttpMethod.POST);
    when(httpRequest.headers()).thenReturn(headers);
    when(httpRequest.response()).thenReturn(httpResponse);
    when(httpResponse.setStatusCode(anyInt())).thenReturn(httpResponse);
    when(httpResponse.putHeader(anyString(), anyString())).thenReturn(httpResponse);
  }

  /**
   * Triggers the request handling flow by publishing the endpoint, simulating an HTTP POST request,
   * and executing the captured executeBlocking callable. Returns the response XML.
   */
  private String triggerRequestFlow(
      String requestXml, boolean wssEnabled, ConsumerResponse response) throws Exception {
    when(wssProcessor.canVerifyInbound()).thenReturn(wssEnabled);
    when(wssProcessor.canSignOutbound()).thenReturn(wssEnabled);
    if (wssEnabled) {
      when(wssProcessor.processInbound(anyString())).thenAnswer(i -> i.getArgument(0));
      when(wssProcessor.processOutbound(anyString())).thenAnswer(i -> i.getArgument(0));
    }

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
  void wssProcessor_notCalled_whenDisabled() throws Exception {
    String requestXml =
        "<soapenv:Envelope xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<soapenv:Body><test/></soapenv:Body></soapenv:Envelope>";

    ConsumerResponse response =
        ConsumerResponse.newBuilder()
            .setRequestId("test-1")
            .setPayload(ByteString.copyFromUtf8("<result>ok</result>"))
            .build();

    triggerRequestFlow(requestXml, false, response);

    verify(wssProcessor, never()).processInbound(anyString());
    verify(wssProcessor, never()).processOutbound(anyString());
  }

  @Test
  void processInbound_calledWithRequestXml_whenWssEnabled() throws Exception {
    String requestXml =
        "<soapenv:Envelope xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<soapenv:Header><wsse:Security>...</wsse:Security></soapenv:Header>"
            + "<soapenv:Body><test/></soapenv:Body></soapenv:Envelope>";

    ConsumerResponse response =
        ConsumerResponse.newBuilder()
            .setRequestId("test-2")
            .setPayload(ByteString.copyFromUtf8("<result>ok</result>"))
            .build();

    triggerRequestFlow(requestXml, true, response);

    ArgumentCaptor<String> inboundCaptor = ArgumentCaptor.forClass(String.class);
    verify(wssProcessor).processInbound(inboundCaptor.capture());
    assertEquals(requestXml, inboundCaptor.getValue());
  }

  @Test
  void processOutbound_calledWithResponseEnvelope_whenWssEnabled() throws Exception {
    String requestXml =
        "<soapenv:Envelope xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<soapenv:Body><test/></soapenv:Body></soapenv:Envelope>";

    ConsumerResponse response =
        ConsumerResponse.newBuilder()
            .setRequestId("test-3")
            .setPayload(ByteString.copyFromUtf8("<result>ok</result>"))
            .build();

    triggerRequestFlow(requestXml, true, response);

    ArgumentCaptor<String> outboundCaptor = ArgumentCaptor.forClass(String.class);
    verify(wssProcessor).processOutbound(outboundCaptor.capture());
    String outboundXml = outboundCaptor.getValue();
    assertTrue(outboundXml.contains("<result>ok</result>"));
    assertTrue(outboundXml.contains("soapenv:Envelope"));
    assertTrue(outboundXml.contains("soapenv:Body"));
  }

  @Test
  void processInbound_and_processOutbound_bothCalled_whenWssEnabled() throws Exception {
    String requestXml =
        "<soapenv:Envelope xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<soapenv:Body><test/></soapenv:Body></soapenv:Envelope>";

    ConsumerResponse response =
        ConsumerResponse.newBuilder()
            .setRequestId("test-4")
            .setPayload(ByteString.copyFromUtf8("<result>ok</result>"))
            .build();

    triggerRequestFlow(requestXml, true, response);

    verify(wssProcessor, times(1)).processInbound(anyString());
    verify(wssProcessor, times(1)).processOutbound(anyString());
  }

  @Test
  void processInbound_exception_propagates() throws Exception {
    when(wssProcessor.canVerifyInbound()).thenReturn(true);
    when(wssProcessor.processInbound(anyString()))
        .thenThrow(new RuntimeException("signature verification failed"));

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
    assertThrows(RuntimeException.class, callable::call);
  }
}
