package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import io.vertx.core.AsyncResult;
import io.vertx.core.Future;
import io.vertx.core.Handler;
import io.vertx.core.MultiMap;
import io.vertx.core.Vertx;
import io.vertx.core.buffer.Buffer;
import io.vertx.core.http.HttpConnection;
import io.vertx.core.http.HttpMethod;
import io.vertx.core.http.HttpServer;
import io.vertx.core.http.HttpServerRequest;
import io.vertx.core.http.HttpServerResponse;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Callable;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.mockito.ArgumentCaptor;
import org.mockito.Mock;
import org.mockito.MockitoAnnotations;

/**
 * Listener request-body cap ({@code CXF_MAX_BODY_BYTES}).
 *
 * <p>Gates are exercised with a pinned cap of 1024 bytes; when {@code CXF_MAX_BODY_BYTES} is
 * exported (container harness: {@code =1024}), the env-to-cap wiring is asserted as well.
 */
class SoapEndpointPublisherBodyCapTest {

  private static final String TEST_PROFILE_NAME = "test_profile";
  private static final String CAP_ENV_VAR = "CXF_MAX_BODY_BYTES";
  private static final long TEST_CAP_BYTES = 1024;

  @Mock BridgeConfig bridgeConfig;
  @Mock CxfServerManager cxfServerManager;
  @Mock SecurityProfileStore profileStore;
  @Mock Vertx vertx;
  @Mock HttpServer httpServer;
  @Mock HttpServerResponse httpResponse;
  @Mock MultiMap headers;

  SoapEndpointPublisher publisher;

  private AutoCloseable mocks;

  private HttpServerRequest httpRequest;
  private HttpConnection httpConnection;

  /** Outcome markers populated through the response stubs. */
  private final AtomicInteger statusCode = new AtomicInteger(-1);

  private final AtomicReference<String> endedBody = new AtomicReference<>();
  private final AtomicBoolean downstreamInvoked = new AtomicBoolean(false);

  @BeforeEach
  void setUp() throws Exception {
    mocks = MockitoAnnotations.openMocks(this);

    publisher = new SoapEndpointPublisher();
    publisher.bridgeConfig = bridgeConfig;
    publisher.cxfServerManager = cxfServerManager;
    publisher.profileStore = profileStore;
    publisher.vertx = vertx;

    // Deterministic cap for gate assertions; env wiring itself is proven below when the
    // variable is exported (container harness runs with CXF_MAX_BODY_BYTES=1024).
    publisher.pinnedMaxBodyBytes = TEST_CAP_BYTES;
    String rawCapEnv = System.getenv(CAP_ENV_VAR);
    org.junit.jupiter.api.Assumptions.assumeTrue(
        rawCapEnv == null || String.valueOf(TEST_CAP_BYTES).equals(rawCapEnv.trim()),
        "unexpected " + CAP_ENV_VAR + " value: expected 1024 when exported, got " + rawCapEnv);
    if (rawCapEnv != null) {
      assertEquals(
          TEST_CAP_BYTES,
          SoapEndpointPublisher.maxBodyBytes(),
          "env " + CAP_ENV_VAR + " must resolve into the cap");
    }

    when(bridgeConfig.address()).thenReturn("http://0.0.0.0:9000/cxf");
    when(bridgeConfig.connectionTimeoutMs()).thenReturn(5000);
    when(bridgeConfig.consumerTimeoutMs()).thenReturn(5000);

    SecurityProfile testProfile = SecurityProfile.builder(TEST_PROFILE_NAME).build();
    when(profileStore.getProfile(TEST_PROFILE_NAME)).thenReturn(testProfile);

    when(vertx.createHttpServer(any())).thenReturn(httpServer);
    when(httpServer.requestHandler(any())).thenReturn(httpServer);
    doAnswer(
            invocation -> {
              Handler<AsyncResult<HttpServer>> handler = invocation.getArgument(2);
              handler.handle(Future.succeededFuture(httpServer));
              return null;
            })
        .when(httpServer)
        .listen(anyInt(), anyString(), any());

    when(headers.entries()).thenReturn(List.of(Map.entry("content-type", "text/xml")));

    when(cxfServerManager.handleSoapRequest(any()))
        .thenAnswer(
            invocation -> {
              downstreamInvoked.set(true);
              return CompletableFuture.completedFuture(
                  cxf_bridge.ConsumerResponse.newBuilder()
                      .setRequestId("req")
                      .setPayload(
                          com.google.protobuf.ByteString.copyFromUtf8("<result>ok</result>"))
                      .setSecurityProfile(TEST_PROFILE_NAME)
                      .build());
            });

    when(httpResponse.setStatusCode(anyInt()))
        .thenAnswer(
            invocation -> {
              statusCode.set(invocation.getArgument(0));
              return httpResponse;
            });
    when(httpResponse.putHeader(anyString(), anyString())).thenReturn(httpResponse);
    when(httpResponse.end(anyString()))
        .thenAnswer(
            invocation -> {
              endedBody.compareAndSet(null, invocation.getArgument(0));
              return Future.succeededFuture();
            });
  }

  @AfterEach
  void tearDown() throws Exception {
    mocks.close();
  }

  /**
   * Publishes the endpoint, captures the request handler, and drives one POST against a fresh
   * mocked request declaring {@code declaredContentLength} (may be null) while streaming {@code
   * streamedBodyBytes} payload bytes. Tolerates implementations that aggregate via the legacy
   * single-shot callback instead of bounded chunk handlers.
   */
  private void drivePost(String declaredContentLength, int streamedBodyBytes) throws Exception {
    ArgumentCaptor<Callable> callableCaptor = ArgumentCaptor.forClass(Callable.class);
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<AsyncResult<String>>> resultHandlerCaptor =
        ArgumentCaptor.forClass(Handler.class);
    doAnswer(invocation -> null)
        .when(vertx)
        .executeBlocking(callableCaptor.capture(), resultHandlerCaptor.capture());

    publisher.publish();

    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<HttpServerRequest>> requestHandlerCaptor =
        ArgumentCaptor.forClass(Handler.class);
    verify(httpServer).requestHandler(requestHandlerCaptor.capture());
    Handler<HttpServerRequest> requestHandler = requestHandlerCaptor.getValue();

    httpRequest = mock(HttpServerRequest.class);
    httpConnection = mock(HttpConnection.class);
    when(httpRequest.path()).thenReturn("/cxf/" + TEST_PROFILE_NAME);
    when(httpRequest.method()).thenReturn(HttpMethod.POST);
    when(httpRequest.headers()).thenReturn(headers);
    when(httpRequest.response()).thenReturn(httpResponse);
    when(httpRequest.connection()).thenReturn(httpConnection);
    when(httpRequest.getHeader("Content-Length")).thenReturn(declaredContentLength);

    // Registration APIs: bounded-chunk vs legacy aggregated capture.
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<Buffer>> chunkCaptor = ArgumentCaptor.forClass(Handler.class);
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<Void>> endCaptor = ArgumentCaptor.forClass(Handler.class);
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<Buffer>> legacyBodyCaptor = ArgumentCaptor.forClass(Handler.class);

    when(httpRequest.handler(chunkCaptor.capture())).thenReturn(httpRequest);
    when(httpRequest.endHandler(endCaptor.capture())).thenReturn(httpRequest);
    when(httpRequest.bodyHandler(legacyBodyCaptor.capture())).thenReturn(httpRequest);

    requestHandler.handle(httpRequest);

    if (chunkCaptor.getAllValues().size() == 1) {
      int bigChunk = streamedBodyBytes / 2;
      byte[] half = new byte[bigChunk];
      java.util.Arrays.fill(half, (byte) 'a');
      Buffer halfBuffer = Buffer.buffer(half);
      chunkCaptor.getValue().handle(halfBuffer);
      chunkCaptor.getValue().handle(halfBuffer);
      if (endCaptor.getAllValues().size() == 1) {
        endCaptor.getValue().handle((Void) null);
      }
    } else if (legacyBodyCaptor.getAllValues().size() == 1) {
      byte[] whole = new byte[streamedBodyBytes];
      java.util.Arrays.fill(whole, (byte) 'a');
      legacyBodyCaptor.getValue().handle(Buffer.buffer(whole));
    } else {
      // Neither API registered: the gate already rejected the request upfront.
      return;
    }

    // Drain the blocked pipeline when the downstream handler was reached.
    List<Callable> callables = callableCaptor.getAllValues();
    if (!callables.isEmpty()) {
      @SuppressWarnings("unchecked")
      Callable<String> callable = callables.get(callables.size() - 1);
      String xml = callable.call();
      List<Handler<AsyncResult<String>>> handlers = resultHandlerCaptor.getAllValues();
      @SuppressWarnings("unchecked")
      Handler<AsyncResult<String>> resultHandler = handlers.get(handlers.size() - 1);
      resultHandler.handle(Future.succeededFuture(xml));
    }
  }

  private void assertNoDownstreamProcessing() {
    assertFalse(downstreamInvoked.get(), "downstream consumer must not be reached");
    try {
      verify(vertx, never()).executeBlocking(any(Callable.class), any());
    } catch (AssertionError e) {
      throw new AssertionError("blocked processing pipeline must not start", e);
    }
  }

  @Test
  void declaredOversizedContentLengthRejectedUpfront() throws Exception {
    drivePost("4096", 4096);

    assertEquals(413, statusCode.get(), "declared oversized Content-Length must yield HTTP 413");
    assertNotNull(endedBody.get(), "a short rejection body must be sent");
    assertTrue(
        endedBody.get().contains(CAP_ENV_VAR),
        "rejection body must name the cap: " + endedBody.get());
    assertNoDownstreamProcessing();
  }

  @Test
  void lyingContentLengthRejectedMidStream() throws Exception {
    drivePost("10", 4096);

    assertEquals(413, statusCode.get(), "liar Content-Length must be caught mid-stream");
    assertFalse(downstreamInvoked.get(), "downstream consumer must not be reached");
    verify(httpConnection, atLeastOnce()).close();
    assertNoDownstreamProcessing();
  }

  @Test
  void underCapBodyPasses() throws Exception {
    String envelope =
        "<soapenv:Envelope xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<soapenv:Body><test/></soapenv:Body></soapenv:Envelope>";

    assertTrue(envelope.length() <= TEST_CAP_BYTES, "fixture must fit under the cap");

    ArgumentCaptor<Callable> callableCaptor = ArgumentCaptor.forClass(Callable.class);
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<AsyncResult<String>>> resultHandlerCaptor =
        ArgumentCaptor.forClass(Handler.class);
    doAnswer(invocation -> null)
        .when(vertx)
        .executeBlocking(callableCaptor.capture(), resultHandlerCaptor.capture());

    publisher.publish();

    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<HttpServerRequest>> requestHandlerCaptor =
        ArgumentCaptor.forClass(Handler.class);
    verify(httpServer).requestHandler(requestHandlerCaptor.capture());

    HttpServerRequest request = mock(HttpServerRequest.class);
    HttpConnection httpConnection = mock(HttpConnection.class);
    when(request.path()).thenReturn("/cxf/" + TEST_PROFILE_NAME);
    when(request.method()).thenReturn(HttpMethod.POST);
    when(request.headers()).thenReturn(headers);
    when(request.response()).thenReturn(httpResponse);
    when(request.connection()).thenReturn(httpConnection);
    when(request.getHeader("Content-Length")).thenReturn(String.valueOf(envelope.length()));

    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<Buffer>> chunkCaptor = ArgumentCaptor.forClass(Handler.class);
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<Void>> endCaptor = ArgumentCaptor.forClass(Handler.class);
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<Buffer>> legacyBodyCaptor = ArgumentCaptor.forClass(Handler.class);
    when(request.handler(chunkCaptor.capture())).thenReturn(request);
    when(request.endHandler(endCaptor.capture())).thenReturn(request);
    when(request.bodyHandler(legacyBodyCaptor.capture())).thenReturn(request);

    requestHandlerCaptor.getValue().handle(request);

    byte[] raw = envelope.getBytes(StandardCharsets.UTF_8);
    if (chunkCaptor.getAllValues().size() == 1) {
      int split = raw.length / 2;
      chunkCaptor.getValue().handle(Buffer.buffer(java.util.Arrays.copyOfRange(raw, 0, split)));
      chunkCaptor
          .getValue()
          .handle(Buffer.buffer(java.util.Arrays.copyOfRange(raw, split, raw.length)));
      endCaptor.getValue().handle((Void) null);
    } else {
      legacyBodyCaptor.getValue().handle(Buffer.buffer(raw));
    }

    List<Callable> callables = callableCaptor.getAllValues();
    assertFalse(callables.isEmpty(), "blocking pipeline must run once");
    @SuppressWarnings("unchecked")
    Callable<String> callable = callables.get(callables.size() - 1);
    String xml = callable.call();
    assertTrue(downstreamInvoked.get(), "normal path must reach the downstream consumer");
    List<Handler<AsyncResult<String>>> handlers = resultHandlerCaptor.getAllValues();
    @SuppressWarnings("unchecked")
    Handler<AsyncResult<String>> resultHandler = handlers.get(handlers.size() - 1);
    resultHandler.handle(Future.succeededFuture(xml));

    assertEquals(200, statusCode.get(), "under-cap body follows the normal 200 path");
    assertNotNull(endedBody.get());
    assertTrue(
        endedBody.get().contains("soapenv:Envelope"),
        "response must carry the wrapped SOAP envelope: " + endedBody.get());
  }

  @Test
  void parseCapMalformedFailsLoud() {
    IllegalStateException ex =
        assertThrows(IllegalStateException.class, () -> SoapEndpointPublisher.parseCap("abc"));
    assertTrue(
        ex.getMessage().contains(CAP_ENV_VAR),
        "failure must name " + CAP_ENV_VAR + ": " + ex.getMessage());
    assertTrue(ex.getMessage().contains("abc"), "failure must echo the raw value");
  }

  @Test
  void parseCapValidValue() {
    assertEquals(2048L, SoapEndpointPublisher.parseCap("2048"));
  }

  @Test
  void parseCapAboveCeilingFailsLoud() {
    // 18 MiB would invert the decode-limit ordering (cap <= 17 MiB ceiling < 18 MiB Rust
    // decode limit).
    IllegalStateException ex =
        assertThrows(
            IllegalStateException.class,
            () -> SoapEndpointPublisher.parseCap(String.valueOf(18L * 1024 * 1024)));
    assertTrue(
        ex.getMessage().contains(CAP_ENV_VAR),
        "failure must name " + CAP_ENV_VAR + ": " + ex.getMessage());
    assertTrue(
        ex.getMessage().contains("17") && ex.getMessage().contains("decode"),
        "failure must explain the 17 MiB decode-ordering ceiling: " + ex.getMessage());
  }
}
