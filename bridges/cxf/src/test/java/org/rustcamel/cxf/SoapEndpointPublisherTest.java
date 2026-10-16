package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import com.google.protobuf.ByteString;
import cxf_bridge.ConsumerRequest;
import cxf_bridge.ConsumerResponse;
import cxf_bridge.SignerIdentity;
import io.vertx.core.AsyncResult;
import io.vertx.core.Future;
import io.vertx.core.Handler;
import io.vertx.core.MultiMap;
import io.vertx.core.Vertx;
import io.vertx.core.buffer.Buffer;
import io.vertx.core.http.ClientAuth;
import io.vertx.core.http.HttpConnection;
import io.vertx.core.http.HttpMethod;
import io.vertx.core.http.HttpServer;
import io.vertx.core.http.HttpServerOptions;
import io.vertx.core.http.HttpServerRequest;
import io.vertx.core.http.HttpServerResponse;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyStore;
import java.security.MessageDigest;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Callable;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;
import javax.security.auth.x500.X500Principal;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.ValueSource;
import org.mockito.AdditionalAnswers;
import org.mockito.ArgumentCaptor;
import org.mockito.Mock;
import org.mockito.MockitoAnnotations;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.NodeList;

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
  private static final String SOAP11_NS = "http://schemas.xmlsoap.org/soap/envelope/";
  private static final String SOAP12_NS = "http://www.w3.org/2003/05/soap-envelope";
  private static final String XML_NS = "http://www.w3.org/XML/1998/namespace";

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
    // Default: a live consumer stream, so the availability gate (Task 2.7) passes and the
    // payload/fault matrix still runs. The pre-stream 503 tests override this to false.
    when(cxfServerManager.hasConsumerObserver()).thenReturn(true);

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
    ArgumentCaptor<Handler<AsyncResult<SoapEndpointPublisher.SoapHttpResult>>> resultHandlerCaptor =
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
    Callable<SoapEndpointPublisher.SoapHttpResult> callable = callableCaptor.getValue();
    SoapEndpointPublisher.SoapHttpResult result = callable.call();

    // Trigger the result handler to complete the response
    resultHandlerCaptor.getValue().handle(Future.succeededFuture(result));

    return result.xml();
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
  void prefixedBodyExtractedByLocalName() throws Exception {
    String requestXml =
        "<soapenv:Envelope xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<soapenv:Body><p:order xmlns:p=\"urn:t\">X</p:order></soapenv:Body></soapenv:Envelope>";

    ConsumerResponse response =
        ConsumerResponse.newBuilder()
            .setRequestId("test-pin-prefixed")
            .setSecurityProfile(TEST_PROFILE_NAME)
            .build();

    SecurityProfile profile = SecurityProfile.builder(TEST_PROFILE_NAME).build();
    triggerRequestFlow(requestXml, profile, response);

    ArgumentCaptor<ConsumerRequest> requestCaptor = ArgumentCaptor.forClass(ConsumerRequest.class);
    verify(cxfServerManager).handleSoapRequest(requestCaptor.capture());
    assertEquals(
        "<p:order xmlns:p=\"urn:t\">X</p:order>",
        requestCaptor.getValue().getPayload().toStringUtf8());
  }

  @Test
  void decoyElementNameNotMisExtracted() throws Exception {
    String requestXml =
        "<Envelope xmlns=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<Body><xsd:bodyData xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\">keep</xsd:bodyData>"
            + "</Body></Envelope>";

    ConsumerResponse response =
        ConsumerResponse.newBuilder()
            .setRequestId("test-decoy")
            .setSecurityProfile(TEST_PROFILE_NAME)
            .build();

    SecurityProfile profile = SecurityProfile.builder(TEST_PROFILE_NAME).build();
    triggerRequestFlow(requestXml, profile, response);

    ArgumentCaptor<ConsumerRequest> requestCaptor = ArgumentCaptor.forClass(ConsumerRequest.class);
    verify(cxfServerManager).handleSoapRequest(requestCaptor.capture());
    assertEquals(
        "<xsd:bodyData xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\">keep</xsd:bodyData>",
        requestCaptor.getValue().getPayload().toStringUtf8());
  }

  @Test
  void emittedConsumerRequestCarriesSigner() throws Exception {
    SecurityProfile signing =
        SecurityProfile.builder(TEST_PROFILE_NAME)
            .keystore(keystorePath.toString(), "changeit")
            .truststore(keystorePath.toString(), "changeit")
            .sigUser(TestKeystoreHelper.KEY_ALIAS, "changeit")
            .encUser(TestKeystoreHelper.KEY_ALIAS)
            .actionsOut("Signature")
            .actionsIn("Signature")
            .build();
    String plainEnvelope =
        "<soapenv:Envelope xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<soapenv:Body><test:Hello xmlns:test=\"http://test.example.com\">World</test:Hello>"
            + "</soapenv:Body></soapenv:Envelope>";
    String signed = new WssSecurityProcessor(signing).processOutbound(plainEnvelope);

    driveRequestOutcome(signed, "text/xml", signing, completedOk());

    ArgumentCaptor<ConsumerRequest> requestCaptor = ArgumentCaptor.forClass(ConsumerRequest.class);
    verify(cxfServerManager).handleSoapRequest(requestCaptor.capture());
    ConsumerRequest signedRequest = requestCaptor.getValue();
    assertTrue(
        signedRequest.hasSigner(), "A Body-covered signed request must carry the signer field");
    SignerIdentity signer = signedRequest.getSigner();
    X509Certificate cert = aliceCertificate();
    assertEquals(
        cert.getSubjectX500Principal().getName(X500Principal.RFC2253),
        signer.getSubject(),
        "signer subject must be the RFC 2253 DN");
    assertEquals(
        cert.getIssuerX500Principal().getName(X500Principal.RFC2253),
        signer.getIssuer(),
        "signer issuer must be the RFC 2253 DN");
    assertEquals(
        cert.getSerialNumber().toString(16), signer.getSerial(), "signer serial must be hex");
    assertEquals(
        sha256Hex(cert.getEncoded()),
        signer.getFingerprintSha256(),
        "signer fingerprint must be the SHA-256 of the DER certificate");

    // Unsigned request: the signer field stays absent (unknown).
    resetPublisher();
    SecurityProfile plain = SecurityProfile.builder(TEST_PROFILE_NAME).build();
    driveRequestOutcome(envelope11("<test/>"), "text/xml", plain, completedOk());
    ArgumentCaptor<ConsumerRequest> unsignedCaptor = ArgumentCaptor.forClass(ConsumerRequest.class);
    verify(cxfServerManager, times(2)).handleSoapRequest(unsignedCaptor.capture());
    assertFalse(
        unsignedCaptor.getAllValues().get(1).hasSigner(),
        "An unsigned request must carry no signer");

    // Spoofed signer headers must never mint a signer.
    resetPublisher();
    driveRequestOutcome(
        envelope11("<test/>"),
        "text/xml",
        plain,
        completedOk(),
        Map.of(
            "camel.cxf.signer.subject", "CN=attacker",
            "camel.cxf.signer.serial", "1",
            "camel.cxf.signer.issuer", "CN=attacker",
            "camel.cxf.signer.fingerprint_sha256", "00"));
    ArgumentCaptor<ConsumerRequest> spoofedCaptor = ArgumentCaptor.forClass(ConsumerRequest.class);
    verify(cxfServerManager, times(3)).handleSoapRequest(spoofedCaptor.capture());
    assertFalse(
        spoofedCaptor.getAllValues().get(2).hasSigner(),
        "Client-supplied signer headers must not export a signer");
  }

  @Test
  void missingBodyIsSenderFault() throws Exception {
    // The guard runs before WSS: a non-Envelope root, a wrong root namespace, a
    // Body in a mismatched namespace, and a Body nested under Header are all
    // rejected as Sender faults. An override profile proves WSS fault_status cannot
    // rewrite them.
    SecurityProfile defaults = SecurityProfile.builder(TEST_PROFILE_NAME).build();
    SecurityProfile overridden =
        SecurityProfile.builder(TEST_PROFILE_NAME).faultStatus(403).build();

    String noBody =
        "<soapenv:Envelope xmlns:soapenv=\"http://www.w3.org/2003/05/soap-envelope\">"
            + "<soapenv:Header/></soapenv:Envelope>";
    String wrongRoot = "<foo xmlns=\"urn:not-soap\"><Body/></foo>";
    String wrongRootNs = "<Envelope xmlns=\"urn:not-soap\"><Body/></Envelope>";
    String mismatchedBodyNs =
        "<soapenv:Envelope xmlns:soapenv=\"http://www.w3.org/2003/05/soap-envelope\""
            + " xmlns:s11=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<s11:Body/></soapenv:Envelope>";
    String nestedBodyInHeader =
        "<soapenv:Envelope xmlns:soapenv=\"http://www.w3.org/2003/05/soap-envelope\">"
            + "<soapenv:Header><soapenv:Body/></soapenv:Header></soapenv:Envelope>";

    for (String requestXml :
        new String[] {noBody, wrongRoot, wrongRootNs, mismatchedBodyNs, nestedBodyInHeader}) {
      HttpOutcome outcome =
          driveRequestOutcome(requestXml, "application/soap+xml", defaults, completedOk());
      assertEquals(
          400,
          outcome.status(),
          "envelope guard must yield Sender 400 for: " + requestXml + " -> " + outcome.body());
      assertTrue(
          outcome.body.contains("Sender") || outcome.body.contains("soap:Client"), outcome.body);
      verify(cxfServerManager, never()).handleSoapRequest(any());
    }

    // The WSS-only fault_status override must not rewrite a pre-WSS guard rejection.
    HttpOutcome overriddenOutcome =
        driveRequestOutcome(wrongRoot, "application/soap+xml", overridden, completedOk());
    assertEquals(
        400,
        overriddenOutcome.status,
        "fault_status override must not apply to the envelope guard: " + overriddenOutcome.body());
    verify(cxfServerManager, never()).handleSoapRequest(any());
  }

  @Test
  void directBodyOverridesNestedHeaderBody() throws Exception {
    SecurityProfile profile = SecurityProfile.builder(TEST_PROFILE_NAME).build();
    // A valid direct SOAP 1.2 Body plus a misleading SOAP 1.1 Body nested inside the
    // Header. The SOAP 1.1 Body is global-search-visible, so a recursive selector
    // would extract it instead of the direct payload.
    String requestXml =
        "<soapenv:Envelope xmlns:soapenv=\"http://www.w3.org/2003/05/soap-envelope\""
            + " xmlns:s11=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<soapenv:Header><s11:Body>"
            + "<evil:operation xmlns:evil=\"urn:evil\">header-body</evil:operation>"
            + "</s11:Body></soapenv:Header>"
            + "<soapenv:Body>"
            + "<wanted:operation xmlns:wanted=\"urn:good\">direct-body</wanted:operation>"
            + "</soapenv:Body></soapenv:Envelope>";

    ConsumerResponse response =
        ConsumerResponse.newBuilder()
            .setRequestId("test-direct-body")
            .setSecurityProfile(TEST_PROFILE_NAME)
            .build();

    triggerRequestFlow(requestXml, profile, response);

    ArgumentCaptor<ConsumerRequest> requestCaptor = ArgumentCaptor.forClass(ConsumerRequest.class);
    verify(cxfServerManager).handleSoapRequest(requestCaptor.capture());
    String payload = requestCaptor.getValue().getPayload().toStringUtf8();
    assertTrue(
        payload.contains("wanted:operation"),
        "the direct SOAP 1.2 Body operation must be forwarded: " + payload);
    assertFalse(
        payload.contains("evil:operation"),
        "the nested Header SOAP 1.1 Body must be ignored: " + payload);
  }

  @Test
  void malformedEnvelopeFails400() throws Exception {
    SecurityProfile profile = SecurityProfile.builder(TEST_PROFILE_NAME).build();
    when(profileStore.getProfile(TEST_PROFILE_NAME)).thenReturn(profile);

    when(cxfServerManager.handleSoapRequest(any()))
        .thenReturn(
            CompletableFuture.completedFuture(
                ConsumerResponse.newBuilder()
                    .setRequestId("test-malformed")
                    .setSecurityProfile(TEST_PROFILE_NAME)
                    .build()));

    when(headers.entries())
        .thenReturn(
            List.of(
                Map.entry("content-type", "application/soap+xml"),
                Map.entry("soapaction", "\"urn:test:Operation\"")));

    ArgumentCaptor<Callable> callableCaptor = ArgumentCaptor.forClass(Callable.class);
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<AsyncResult<SoapEndpointPublisher.SoapHttpResult>>> resultHandlerCaptor =
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

    requestHandlerCaptor.getValue().handle(httpRequest);
    chunkCaptor.getValue().handle(Buffer.buffer("<not-xml".getBytes(StandardCharsets.UTF_8)));
    endCaptor.getValue().handle((Void) null);

    @SuppressWarnings("unchecked")
    Callable<SoapEndpointPublisher.SoapHttpResult> callable = callableCaptor.getValue();
    Exception thrown = assertThrows(Exception.class, callable::call);

    resultHandlerCaptor.getValue().handle(Future.failedFuture(thrown));

    verify(httpResponse).setStatusCode(400);
    String faultBody = responseCaptor.getValue();
    assertTrue(faultBody.contains("malformed"), faultBody);
    verify(cxfServerManager, never()).handleSoapRequest(any());
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
    ArgumentCaptor<Handler<AsyncResult<SoapEndpointPublisher.SoapHttpResult>>> resultHandlerCaptor =
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
    Callable<SoapEndpointPublisher.SoapHttpResult> callable = callableCaptor.getValue();
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
    ArgumentCaptor<Handler<AsyncResult<SoapEndpointPublisher.SoapHttpResult>>> resultHandlerCaptor =
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
    Callable<SoapEndpointPublisher.SoapHttpResult> firstCallable =
        callableCaptor.getAllValues().get(0);
    SoapEndpointPublisher.SoapHttpResult firstResult = firstCallable.call();

    resultHandlerCaptor.getAllValues().get(0).handle(Future.succeededFuture(firstResult));
    verify(httpResponse).setStatusCode(200);

    // --- Second POST: identical bytes must be rejected as a WSS replay ---
    requestHandlerCaptor.getValue().handle(httpRequest);
    chunkCaptor.getAllValues().get(1).handle(signedBuffer);
    endCaptor.getAllValues().get(1).handle((Void) null);

    @SuppressWarnings("unchecked")
    Callable<SoapEndpointPublisher.SoapHttpResult> secondCallable =
        callableCaptor.getAllValues().get(1);
    Exception replayError = assertThrows(Exception.class, secondCallable::call);

    resultHandlerCaptor.getAllValues().get(1).handle(Future.failedFuture(replayError));

    // SOAP 1.1: any fault maps to 500 (the WSS replay is a Sender fault).
    verify(httpResponse).setStatusCode(500);
    String endedBody = responseCaptor.getAllValues().get(1);
    assertTrue(
        endedBody.contains("soap:Client"),
        "Failure branch must emit a soap:Client fault for WSS replay rejection");
  }

  @Test
  void httpsAddressFailsStartup() {
    when(bridgeConfig.address()).thenReturn("https://0.0.0.0:9000/soap");

    IllegalStateException ex = assertThrows(IllegalStateException.class, () -> publisher.publish());

    assertTrue(ex.getMessage().contains("https"), ex.getMessage());
    assertTrue(ex.getMessage().contains("soap_tls"), ex.getMessage());
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

  /**
   * Listener-TLS shape case table (Task 2.6): the same fail-closed shape rules the Rust config load
   * enforces, checked before any socket binds. Exactly one key-material form (PEM cert+key XOR
   * PKCS12 path+password), {@code soap_tls_client_ca_path} always required, TLS material requires
   * an {@code https} bind, {@code https} requires material, and TLS material without a configured
   * bind is not applicable. Incomplete pairs are rejected in both directions.
   */
  @Test
  void listenerTlsShapeCaseTable() {
    record Shape(
        String name,
        String address,
        String certPath,
        String keyPath,
        String clientCaPath,
        String pkcs12Path,
        String pkcs12Password,
        boolean accepted) {}

    Shape[] cases = {
      new Shape(
          "cert/key form accepted",
          "https://0.0.0.0:9443/soap",
          "/etc/cxf/server.pem",
          "/etc/cxf/server.key",
          "/etc/cxf/ca.pem",
          null,
          null,
          true),
      new Shape(
          "pkcs12 form accepted",
          "https://0.0.0.0:9443/soap",
          null,
          null,
          "/etc/cxf/ca.pem",
          "/etc/cxf/server.p12",
          "s3cret",
          true),
      new Shape(
          "both forms rejected",
          "https://0.0.0.0:9443/soap",
          "/etc/cxf/server.pem",
          "/etc/cxf/server.key",
          "/etc/cxf/ca.pem",
          "/etc/cxf/server.p12",
          "s3cret",
          false),
      new Shape(
          "neither form rejected for https",
          "https://0.0.0.0:9443/soap",
          null,
          null,
          null,
          null,
          null,
          false),
      new Shape(
          "missing client ca rejected (pem)",
          "https://0.0.0.0:9443/soap",
          "/etc/cxf/server.pem",
          "/etc/cxf/server.key",
          null,
          null,
          null,
          false),
      new Shape(
          "missing client ca rejected (pkcs12)",
          "https://0.0.0.0:9443/soap",
          null,
          null,
          null,
          "/etc/cxf/server.p12",
          "s3cret",
          false),
      new Shape(
          "http bind rejected",
          "http://0.0.0.0:9443/soap",
          "/etc/cxf/server.pem",
          "/etc/cxf/server.key",
          "/etc/cxf/ca.pem",
          null,
          null,
          false),
      new Shape(
          "cert without key rejected",
          "https://0.0.0.0:9443/soap",
          "/etc/cxf/server.pem",
          null,
          "/etc/cxf/ca.pem",
          null,
          null,
          false),
      new Shape(
          "key without cert rejected",
          "https://0.0.0.0:9443/soap",
          null,
          "/etc/cxf/server.key",
          "/etc/cxf/ca.pem",
          null,
          null,
          false),
      new Shape(
          "pkcs12 without password rejected",
          "https://0.0.0.0:9443/soap",
          null,
          null,
          "/etc/cxf/ca.pem",
          "/etc/cxf/server.p12",
          null,
          false),
      new Shape(
          "password without pkcs12 rejected",
          "https://0.0.0.0:9443/soap",
          null,
          null,
          "/etc/cxf/ca.pem",
          null,
          "s3cret",
          false),
      new Shape(
          "tls material without bind rejected",
          null,
          "/etc/cxf/server.pem",
          "/etc/cxf/server.key",
          "/etc/cxf/ca.pem",
          null,
          null,
          false),
    };

    for (Shape c : cases) {
      if (c.accepted()) {
        assertDoesNotThrow(
            () ->
                SoapEndpointPublisher.validateListenerTls(
                    c.address(),
                    c.certPath(),
                    c.keyPath(),
                    c.clientCaPath(),
                    c.pkcs12Path(),
                    c.pkcs12Password()),
            c.name());
      } else {
        IllegalStateException ex =
            assertThrows(
                IllegalStateException.class,
                () ->
                    SoapEndpointPublisher.validateListenerTls(
                        c.address(),
                        c.certPath(),
                        c.keyPath(),
                        c.clientCaPath(),
                        c.pkcs12Path(),
                        c.pkcs12Password()),
                c.name());
        assertNotNull(ex.getMessage(), c.name());
      }
    }
  }

  /**
   * Blank listener-TLS values both layers reject (N4 fail-closed parity): empty, ASCII whitespace,
   * and Unicode White_Space members where Java's {@code String.isBlank} and Rust's {@code
   * char::is_whitespace} disagree (NBSP U+00A0, NEL U+0085, mixed ASCII + U+202F).
   */
  private static final String[] BLANK_TLS_VALUES = {
    "", " ", "\t", " \t\r\n", "\u00A0", "\u0085", " \u202F",
  };

  private static void assertListenerTlsBlankRejected(
      String address,
      String certPath,
      String keyPath,
      String clientCaPath,
      String pkcs12Path,
      String pkcs12Password,
      String poolKey) {
    IllegalStateException ex =
        assertThrows(
            IllegalStateException.class,
            () ->
                SoapEndpointPublisher.validateListenerTls(
                    address, certPath, keyPath, clientCaPath, pkcs12Path, pkcs12Password));
    assertTrue(ex.getMessage().contains(poolKey), ex.getMessage());
    assertTrue(ex.getMessage().contains("whitespace"), ex.getMessage());
  }

  @Test
  void listenerTlsRejectsBlankMaterialValues() {
    // Each blank material key inside an otherwise-valid PEM listener shape.
    for (String blank : BLANK_TLS_VALUES) {
      assertListenerTlsBlankRejected(
          "https://0.0.0.0:9443/soap",
          blank,
          "/etc/cxf/server.key",
          "/etc/cxf/ca.pem",
          null,
          null,
          "soap_tls_cert_path");
      assertListenerTlsBlankRejected(
          "https://0.0.0.0:9443/soap",
          "/etc/cxf/server.pem",
          blank,
          "/etc/cxf/ca.pem",
          null,
          null,
          "soap_tls_key_path");
      assertListenerTlsBlankRejected(
          "https://0.0.0.0:9443/soap",
          "/etc/cxf/server.pem",
          "/etc/cxf/server.key",
          blank,
          null,
          null,
          "soap_tls_client_ca_path");
    }

    // Each blank material key inside an otherwise-valid PKCS#12 listener shape.
    for (String blank : BLANK_TLS_VALUES) {
      assertListenerTlsBlankRejected(
          "https://0.0.0.0:9443/soap",
          null,
          null,
          "/etc/cxf/ca.pem",
          blank,
          "s3cret",
          "soap_tls_pkcs12_path");
      IllegalStateException ex =
          assertThrows(
              IllegalStateException.class,
              () ->
                  SoapEndpointPublisher.validateListenerTls(
                      "https://0.0.0.0:9443/soap",
                      null,
                      null,
                      "/etc/cxf/ca.pem",
                      "/etc/cxf/server.p12",
                      blank));
      assertTrue(ex.getMessage().contains("soap_tls_pkcs12_password"), ex.getMessage());
      assertTrue(ex.getMessage().contains("whitespace"), ex.getMessage());
      // Non-echo is only meaningful for distinctive (non-ASCII) blanks; plain
      // ASCII whitespace occurs in any message text.
      if (blank.codePoints().anyMatch(cp -> cp > 127)) {
        assertFalse(ex.getMessage().contains(blank), "error must not echo the value");
      }
    }

    // Explicitly blank-only TLS material with no bind is rejected as a blank
    // value, not as a confusing "none is configured" shape error.
    for (String blank : BLANK_TLS_VALUES) {
      assertListenerTlsBlankRejected(
          null, null, null, blank, null, null, "soap_tls_client_ca_path");
    }

    // A blank-complete PEM form over an https bind previously looked like "no
    // material" here (Java) while Rust config load accepted it as material.
    assertListenerTlsBlankRejected(
        "https://0.0.0.0:9443/soap", "", "", "", null, null, "soap_tls_cert_path");

    // Genuine unset producer-only path stays valid.
    assertDoesNotThrow(
        () -> SoapEndpointPublisher.validateListenerTls(null, null, null, null, null, null));
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

  // ── Task 2.7: mutual-TLS listener + startup publication + availability gate ──

  /** A real {@link Vert.x} that still records the {@link HttpServerOptions} it was created with. */
  private static Vertx recordingVertx(Vertx real) {
    return org.mockito.Mockito.mock(Vertx.class, AdditionalAnswers.delegatesTo(real));
  }

  /** A fresh publisher wired to the shared mocks, with {@code v} as its Vert.x instance. */
  private SoapEndpointPublisher publisherWiredWith(Vertx v) {
    SoapEndpointPublisher pub = new SoapEndpointPublisher();
    pub.bridgeConfig = bridgeConfig;
    pub.cxfServerManager = cxfServerManager;
    pub.profileStore = profileStore;
    pub.vertx = v;
    return pub;
  }

  private static int postStatus(int port, javax.net.ssl.SSLContext ctx, String body)
      throws Exception {
    java.net.http.HttpClient client = java.net.http.HttpClient.newBuilder().sslContext(ctx).build();
    java.net.http.HttpResponse<String> response =
        client.send(
            java.net.http.HttpRequest.newBuilder()
                .uri(java.net.URI.create("https://127.0.0.1:" + port + "/cxf/" + TEST_PROFILE_NAME))
                .header("content-type", "text/xml; charset=utf-8")
                .POST(java.net.http.HttpRequest.BodyPublishers.ofString(body))
                .build(),
            java.net.http.HttpResponse.BodyHandlers.ofString());
    return response.statusCode();
  }

  private static java.net.http.HttpResponse<String> postPlain(int port, String body)
      throws Exception {
    java.net.http.HttpClient client = java.net.http.HttpClient.newHttpClient();
    return client.send(
        java.net.http.HttpRequest.newBuilder()
            .uri(java.net.URI.create("http://127.0.0.1:" + port + "/cxf/" + TEST_PROFILE_NAME))
            .header("content-type", "text/xml; charset=utf-8")
            .POST(java.net.http.HttpRequest.BodyPublishers.ofString(body))
            .build(),
        java.net.http.HttpResponse.BodyHandlers.ofString());
  }

  /**
   * PEM cert/key/client-CA + https: the listener binds with {@code ClientAuth.REQUIRED} on an
   * SSL-enabled server — a TLS client without a certificate cannot complete the handshake, a client
   * presenting the CA-signed client certificate can. One identity serves all profiles.
   */
  @Test
  void certKeyFormBindsWithClientAuth() throws Exception {
    Path dir = Files.createTempDirectory("cxf-listener-pem");
    TestKeystoreHelper.ListenerTlsMaterial material =
        TestKeystoreHelper.createListenerTlsMaterial(dir);
    Vertx real = Vertx.vertx();
    Vertx recording = recordingVertx(real);
    SoapEndpointPublisher pub = publisherWiredWith(recording);
    try {
      when(bridgeConfig.address()).thenReturn("https://127.0.0.1:0/cxf");
      when(bridgeConfig.soapTlsCertPath()).thenReturn(material.certPem().toString());
      when(bridgeConfig.soapTlsKeyPath()).thenReturn(material.keyPem().toString());
      when(bridgeConfig.soapTlsClientCaPath()).thenReturn(material.clientCaPem().toString());

      pub.publish();
      assertTrue(pub.soapPort().isPresent(), "publish must record the actual bound port");
      int port = pub.soapPort().getAsInt();
      assertTrue(port > 0, "bound port must be positive: " + port);

      ArgumentCaptor<HttpServerOptions> optionsCaptor =
          ArgumentCaptor.forClass(HttpServerOptions.class);
      verify(recording).createHttpServer(optionsCaptor.capture());
      HttpServerOptions options = optionsCaptor.getValue();
      assertTrue(options.isSsl(), "https listener must enable SSL");
      assertEquals(ClientAuth.REQUIRED, options.getClientAuth(), "client auth must be REQUIRED");
      assertNotNull(options.getPemKeyCertOptions(), "PEM identity must be configured");
      assertEquals(material.certPem().toString(), options.getPemKeyCertOptions().getCertPath());
      assertEquals(material.keyPem().toString(), options.getPemKeyCertOptions().getKeyPath());
      assertNotNull(options.getPemTrustOptions(), "client CA trust must be configured");
      // Client authentication is REQUIRED: no client certificate, no completed exchange.
      // Under TLS 1.3 the client side of the handshake finishes before the server's
      // client-certificate verdict, so the rejection surfaces on the first I/O — both
      // phases are covered here.
      javax.net.ssl.SSLContext trustOnly =
          TestKeystoreHelper.trustOnlySslContext(material.clientCaPem());
      try (javax.net.ssl.SSLSocket socket =
          (javax.net.ssl.SSLSocket) trustOnly.getSocketFactory().createSocket("127.0.0.1", port)) {
        socket.setSoTimeout(5_000);
        assertThrows(
            javax.net.ssl.SSLException.class,
            () -> {
              socket.startHandshake();
              socket
                  .getOutputStream()
                  .write("GET / HTTP/1.0\r\n\r\n".getBytes(StandardCharsets.US_ASCII));
              socket.getOutputStream().flush();
              int n = socket.getInputStream().read();
              fail("client without a certificate must be rejected, read returned " + n);
            });
      }

      // The same single identity accepts a CA-signed client certificate (mutual TLS).
      javax.net.ssl.SSLContext withClient =
          TestKeystoreHelper.clientSslContext(
              material.clientCaPem(), material.clientJksPath(), material.password());
      try (javax.net.ssl.SSLSocket socket =
          (javax.net.ssl.SSLSocket) withClient.getSocketFactory().createSocket("127.0.0.1", port)) {
        socket.setSoTimeout(5_000);
        socket.startHandshake();
      }
    } finally {
      pub.stop();
      real.close().toCompletionStage().toCompletableFuture().get(5, TimeUnit.SECONDS);
    }
  }

  /** PKCS#12 store + client CA: the listener binds the configured PFX identity with mutual TLS. */
  @Test
  void pkcs12FormBinds() throws Exception {
    Path dir = Files.createTempDirectory("cxf-listener-p12");
    TestKeystoreHelper.ListenerTlsMaterial material =
        TestKeystoreHelper.createListenerTlsMaterial(dir);
    Vertx real = Vertx.vertx();
    Vertx recording = recordingVertx(real);
    SoapEndpointPublisher pub = publisherWiredWith(recording);
    try {
      when(bridgeConfig.address()).thenReturn("https://127.0.0.1:0/cxf");
      when(bridgeConfig.soapTlsPkcs12Path()).thenReturn(material.pkcs12Path().toString());
      when(bridgeConfig.soapTlsPkcs12Password()).thenReturn(material.password());
      when(bridgeConfig.soapTlsClientCaPath()).thenReturn(material.clientCaPem().toString());

      pub.publish();
      assertTrue(pub.soapPort().isPresent(), "listener must bind and report its actual port");
      int port = pub.soapPort().getAsInt();

      ArgumentCaptor<HttpServerOptions> optionsCaptor =
          ArgumentCaptor.forClass(HttpServerOptions.class);
      verify(recording).createHttpServer(optionsCaptor.capture());
      HttpServerOptions options = optionsCaptor.getValue();
      assertTrue(options.isSsl());
      assertEquals(ClientAuth.REQUIRED, options.getClientAuth());
      assertNotNull(options.getPfxKeyCertOptions(), "PKCS#12 identity must be configured");
      assertEquals(material.pkcs12Path().toString(), options.getPfxKeyCertOptions().getPath());
      assertNotNull(options.getPemTrustOptions(), "client CA trust must be configured");

      // The PFX identity serves the handshake: a client with a valid client cert connects.
      javax.net.ssl.SSLContext withClient =
          TestKeystoreHelper.clientSslContext(
              material.clientCaPem(), material.clientJksPath(), material.password());
      try (javax.net.ssl.SSLSocket socket =
          (javax.net.ssl.SSLSocket) withClient.getSocketFactory().createSocket("127.0.0.1", port)) {
        socket.setSoTimeout(5_000);
        socket.startHandshake();
      }
    } finally {
      pub.stop();
      real.close().toCompletionStage().toCompletableFuture().get(5, TimeUnit.SECONDS);
    }
  }

  /**
   * Listener TLS is independent of the WSS message-layer stores: a WSS-only profile with no pool
   * TLS fails closed on an https bind (no WSS-store fallback), and when pool TLS is configured the
   * listener identity comes ONLY from the {@code soap_tls_*} keys — never from the WSS keystore.
   */
  @Test
  void listenerTlsIndependentOfWssStores() throws Exception {
    SecurityProfile wssOnly =
        SecurityProfile.builder(TEST_PROFILE_NAME)
            .keystore(keystorePath.toString(), "changeit")
            .truststore(keystorePath.toString(), "changeit")
            .build();
    when(profileStore.getProfile(TEST_PROFILE_NAME)).thenReturn(wssOnly);

    // Fail closed: WSS-only profile + https + no pool TLS must NOT fall back to the WSS stores.
    when(bridgeConfig.address()).thenReturn("https://127.0.0.1:9443/cxf");
    IllegalStateException ex = assertThrows(IllegalStateException.class, () -> publisher.publish());
    assertTrue(ex.getMessage().contains("soap_tls"), ex.getMessage());
    verify(vertx, never()).createHttpServer(any());

    // Positive half: with pool TLS configured, the listener identity is exactly the pool material.
    Path dir = Files.createTempDirectory("cxf-listener-iso");
    TestKeystoreHelper.ListenerTlsMaterial material =
        TestKeystoreHelper.createListenerTlsMaterial(dir);
    Vertx real = Vertx.vertx();
    Vertx recording = recordingVertx(real);
    SoapEndpointPublisher pub = publisherWiredWith(recording);
    try {
      when(bridgeConfig.address()).thenReturn("https://127.0.0.1:0/cxf");
      when(bridgeConfig.soapTlsCertPath()).thenReturn(material.certPem().toString());
      when(bridgeConfig.soapTlsKeyPath()).thenReturn(material.keyPem().toString());
      when(bridgeConfig.soapTlsClientCaPath()).thenReturn(material.clientCaPem().toString());
      pub.publish();

      ArgumentCaptor<HttpServerOptions> optionsCaptor =
          ArgumentCaptor.forClass(HttpServerOptions.class);
      verify(recording).createHttpServer(optionsCaptor.capture());
      HttpServerOptions options = optionsCaptor.getValue();
      assertEquals(material.certPem().toString(), options.getPemKeyCertOptions().getCertPath());
      assertNotEquals(
          keystorePath.toString(),
          options.getPemKeyCertOptions().getCertPath(),
          "the WSS keystore must never serve as the listener identity");
      assertNull(options.getPfxKeyCertOptions(), "no PKCS#12 fallback from WSS stores");
    } finally {
      pub.stop();
      real.close().toCompletionStage().toCompletableFuture().get(5, TimeUnit.SECONDS);
    }
  }

  /**
   * Startup publication with no consumer stream: firing the Quarkus {@code StartupEvent} binds the
   * configured https listener and records the real bound port; {@code handleSoapRequest} before the
   * stream fails with the typed unavailability (no hang) and an actual POST takes the non-fault
   * {@code 503} transport-availability response.
   */
  @Test
  void publishesAtStartupWithoutConsumerStream() throws Exception {
    Path dir = Files.createTempDirectory("cxf-listener-startup");
    TestKeystoreHelper.ListenerTlsMaterial material =
        TestKeystoreHelper.createListenerTlsMaterial(dir);
    Vertx real = Vertx.vertx();
    SoapEndpointPublisher pub = publisherWiredWith(real);
    CxfServerManager manager = new CxfServerManager();
    try {
      when(bridgeConfig.address()).thenReturn("https://127.0.0.1:0/cxf");
      when(bridgeConfig.soapTlsCertPath()).thenReturn(material.certPem().toString());
      when(bridgeConfig.soapTlsKeyPath()).thenReturn(material.keyPem().toString());
      when(bridgeConfig.soapTlsClientCaPath()).thenReturn(material.clientCaPem().toString());
      when(bridgeConfig.maxConcurrentRequests()).thenReturn(100);
      manager.bridgeConfig = bridgeConfig;
      manager.endpointPublisher = pub;
      pub.cxfServerManager = manager;

      pub.onStart(mock(io.quarkus.runtime.StartupEvent.class));

      assertTrue(pub.soapPort().isPresent(), "startup publish must bind the listener");
      int port = pub.soapPort().getAsInt();
      assertTrue(port > 0, "soapPort() must be the real bound port: " + port);

      // handleSoapRequest before any consumer stream: typed unavailability, not a hang.
      CompletableFuture<cxf_bridge.ConsumerResponse> pending =
          manager.handleSoapRequest(
              cxf_bridge.ConsumerRequest.newBuilder().setRequestId("pre-stream").build());
      java.util.concurrent.ExecutionException failure =
          assertThrows(
              java.util.concurrent.ExecutionException.class,
              () -> pending.get(2, TimeUnit.SECONDS),
              "pre-stream handleSoapRequest must fail fast, not hang");
      assertInstanceOf(
          CxfServerManager.StreamUnavailableException.class,
          failure.getCause(),
          "pre-stream failure must be the typed stream unavailability: " + failure.getCause());

      // An actual POST to the announced port takes the transport-availability 503 (no fault).
      javax.net.ssl.SSLContext withClient =
          TestKeystoreHelper.clientSslContext(
              material.clientCaPem(), material.clientJksPath(), material.password());
      assertEquals(
          503,
          postStatus(port, withClient, envelope11("<test/>")),
          "pre-stream POST must take the non-fault 503 transport-availability response");
    } finally {
      manager.cleanup();
      real.close().toCompletionStage().toCompletableFuture().get(5, TimeUnit.SECONDS);
    }
  }

  // ── F3: listener TLS is validated on every Java startup, bind or no bind ──

  /**
   * An incomplete listener TLS shape (a PKCS#12 keystore location with the required password
   * missing) must be rejected by the actual startup observer even when no SOAP bind is configured.
   * The defect was that validation lived only on the publication path, so a producer-only startup
   * skipped it; startup must fail before readiness/publication and must not bind anything.
   */
  @Test
  void startupRejectsIncompleteListenerTlsWithoutBind() {
    when(bridgeConfig.address()).thenReturn(null);
    when(bridgeConfig.soapTlsPkcs12Path()).thenReturn("/etc/cxf/server.p12");

    IllegalStateException ex =
        assertThrows(
            IllegalStateException.class,
            () -> publisher.onStart(mock(io.quarkus.runtime.StartupEvent.class)));

    assertTrue(ex.getMessage().contains("soap_tls_pkcs12_password"), ex.getMessage());
    assertTrue(publisher.soapPort().isEmpty(), "a rejected startup must not report a bound port");
    verify(vertx, never()).createHttpServer(any());
  }

  /**
   * No bind and no listener TLS is a valid producer-only startup: it must succeed, report no
   * listener port, and never create an implicit default server bind.
   */
  @Test
  void startupWithoutBindAndWithoutTlsDoesNotPublish() {
    when(bridgeConfig.address()).thenReturn(null);

    assertDoesNotThrow(() -> publisher.onStart(mock(io.quarkus.runtime.StartupEvent.class)));

    assertTrue(
        publisher.soapPort().isEmpty(), "producer-only startup must not report a listener port");
    verify(vertx, never()).createHttpServer(any());
  }

  /**
   * Client authentication requires the client CA trust material: a startup with key material but no
   * {@code soap_tls_client_ca_path}, and no bind configured, must take the documented
   * invalid-configuration refusal before readiness rather than starting.
   */
  @Test
  void startupRejectsTlsClientAuthWithoutBind() {
    when(bridgeConfig.address()).thenReturn(null);
    when(bridgeConfig.soapTlsCertPath()).thenReturn("/etc/cxf/server.pem");
    when(bridgeConfig.soapTlsKeyPath()).thenReturn("/etc/cxf/server.key");

    IllegalStateException ex =
        assertThrows(
            IllegalStateException.class,
            () -> publisher.onStart(mock(io.quarkus.runtime.StartupEvent.class)));

    assertTrue(ex.getMessage().contains("soap_tls_client_ca_path"), ex.getMessage());
    assertTrue(publisher.soapPort().isEmpty(), "a rejected startup must not report a bound port");
    verify(vertx, never()).createHttpServer(any());
  }

  /**
   * The pre-stream response is transport availability, not a SOAP fault: HTTP 503 with {@code
   * Retry-After} and a plaintext body containing no Envelope/Fault, never a generic 500, and no
   * consumer RPC is issued.
   */
  @Test
  void preStream503IsTransportAvailabilityNotFault() throws Exception {
    Vertx real = Vertx.vertx();
    SoapEndpointPublisher pub = publisherWiredWith(real);
    try {
      when(bridgeConfig.address()).thenReturn("http://127.0.0.1:0/soap");
      when(cxfServerManager.hasConsumerObserver()).thenReturn(false);

      pub.publish();
      assertTrue(pub.soapPort().isPresent());
      java.net.http.HttpResponse<String> response =
          postPlain(pub.soapPort().getAsInt(), envelope11("<test/>"));

      assertEquals(503, response.statusCode(), "pre-stream status must be 503: " + response.body());
      assertNotNull(
          response.headers().firstValue("retry-after").orElse(null),
          "the transport-availability response must carry Retry-After");
      assertTrue(
          response.headers().firstValue("content-type").orElse("").contains("text/plain"),
          "body must be plaintext: " + response.headers().firstValue("content-type"));
      String body = response.body();
      assertFalse(body.contains("Envelope"), "no SOAP envelope: " + body);
      assertFalse(body.contains("Fault"), "no SOAP fault: " + body);
      assertFalse(body.contains("<"), "plaintext body only: " + body);
      verify(cxfServerManager, never()).handleSoapRequest(any());
    } finally {
      pub.stop();
      real.close().toCompletionStage().toCompletableFuture().get(5, TimeUnit.SECONDS);
    }
  }

  /**
   * Availability is checked BEFORE WS-Security and before any XML/version classification: a
   * Signature-required profile whose signature is missing takes the pre-stream 503 (never a 400/500
   * security fault), the WSS processor is never invoked, and the transport body limits still apply
   * first (oversize is 413, not a parse-500).
   */
  @Test
  void availabilityCheckedBeforeWssForSignatureProfile() throws Exception {
    SecurityProfile signatureRequired =
        SecurityProfile.builder(TEST_PROFILE_NAME)
            .keystore(keystorePath.toString(), "changeit")
            .truststore(keystorePath.toString(), "changeit")
            .actionsIn("Signature")
            .build();
    WssSecurityProcessor recording = mock(WssSecurityProcessor.class);
    installWssProcessor(recording);
    when(cxfServerManager.hasConsumerObserver()).thenReturn(false);

    // Missing signature on a Signature-required profile: still the pre-stream 503.
    HttpOutcome outcome =
        driveRequestOutcome(envelope11("<test/>"), "text/xml", signatureRequired, completedOk());
    assertEquals(503, outcome.status(), "pre-stream gate must win over WSS: " + outcome.body());
    verify(httpResponse).putHeader("Retry-After", "1");
    assertTrue(outcome.body().contains("routes unavailable"), outcome.body());
    assertFalse(outcome.body().contains("Envelope"), outcome.body());
    assertFalse(outcome.body().contains("Fault"), outcome.body());
    verifyNoInteractions(recording);

    // Transport body limits apply before the gate and before parsing: oversize is 413.
    resetPublisher();
    WssSecurityProcessor recording2 = mock(WssSecurityProcessor.class);
    installWssProcessor(recording2);
    publisher.pinnedMaxBodyBytes = 64;
    when(httpRequest.connection()).thenReturn(mock(HttpConnection.class));
    HttpOutcome oversized =
        driveRequestOutcome(envelope11("<test/>"), "text/xml", signatureRequired, completedOk());
    assertEquals(413, oversized.status(), "oversize body must be 413: " + oversized.body());
    assertTrue(oversized.body().contains("CXF_MAX_BODY_BYTES"), oversized.body());
    verifyNoInteractions(recording2);
  }

  /**
   * Late stream loss after the availability gate, with an ACTUAL pending consumer future and
   * replacement state in effect, yields the SAME 503 transport-availability response (typed {@code
   * StreamUnavailableException}) — never a generic Receiver 500 and never a fault envelope.
   *
   * <p>Sequence: the gate passes on live stream A; the request is registered on A and dispatched;
   * at dispatch B replaces A and A's stream closes while its request is in flight (A no longer owns
   * the liveness flag, yet its own pending future must still fail typed — see {@code
   * CxfServerManagerTest.replacedStreamCloseFailsOnlyItsPendingRequests}); the typed failure maps
   * to the transport-availability 503. B stays live throughout.
   */
  @Test
  void lateStreamLossYieldsSame503() throws Exception {
    CxfServerManager manager = new CxfServerManager();
    when(bridgeConfig.maxConcurrentRequests()).thenReturn(100);
    // Bounded, but the close must unblock the dispatch — never the timeout — on the fixed path.
    when(bridgeConfig.consumerTimeoutMs()).thenReturn(5_000);
    manager.bridgeConfig = bridgeConfig;
    manager.endpointPublisher = publisher;
    publisher.cxfServerManager = manager;

    @SuppressWarnings("unchecked")
    io.grpc.stub.StreamObserver<cxf_bridge.ConsumerRequest> observerA =
        mock(io.grpc.stub.StreamObserver.class);
    @SuppressWarnings("unchecked")
    io.grpc.stub.StreamObserver<cxf_bridge.ConsumerRequest> observerB =
        mock(io.grpc.stub.StreamObserver.class);

    // The stream dies late, at dispatch: B replaces A, then A's stream closes while A's
    // just-registered request is still in flight.
    doAnswer(
            invocation -> {
              manager.setConsumerRequestObserver(observerB);
              manager.onConsumerStreamClosed(observerA);
              return null;
            })
        .when(observerA)
        .onNext(any());

    manager.setConsumerRequestObserver(observerA);

    when(profileStore.getProfile(TEST_PROFILE_NAME))
        .thenReturn(SecurityProfile.builder(TEST_PROFILE_NAME).build());
    when(headers.entries())
        .thenReturn(
            List.of(
                Map.entry("content-type", "text/xml"),
                Map.entry("soapaction", "\"urn:test:Operation\"")));
    when(bodyBuffer.toString(StandardCharsets.UTF_8)).thenReturn(envelope11("<test/>"));

    ArgumentCaptor<Callable> callableCaptor = ArgumentCaptor.forClass(Callable.class);
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<AsyncResult<SoapEndpointPublisher.SoapHttpResult>>> resultHandlerCaptor =
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

    requestHandlerCaptor.getValue().handle(httpRequest);
    chunkCaptor
        .getValue()
        .handle(Buffer.buffer(envelope11("<test/>").getBytes(StandardCharsets.UTF_8)));
    endCaptor.getValue().handle((Void) null);
    // The availability gate above saw a live stream (A) and passed.

    // Dispatch registers the REAL pending future on A; A's stream dies at dispatch (replaced
    // by B, then closed). The typed unavailability surfaces out of the blocking pipeline.
    ArgumentCaptor<Integer> statusCaptor = ArgumentCaptor.forClass(Integer.class);
    @SuppressWarnings("unchecked")
    Callable<SoapEndpointPublisher.SoapHttpResult> callable = callableCaptor.getValue();
    try {
      SoapEndpointPublisher.SoapHttpResult result = callable.call();
      resultHandlerCaptor.getValue().handle(Future.succeededFuture(result));
    } catch (Exception e) {
      resultHandlerCaptor.getValue().handle(Future.failedFuture(e));
    }

    verify(httpResponse, atLeastOnce()).setStatusCode(statusCaptor.capture());
    assertEquals(503, statusCaptor.getValue(), "late stream loss must be the same 503");
    verify(httpResponse).putHeader("Retry-After", "1");
    String body = responseCaptor.getValue();
    assertFalse(body.contains("Envelope"), body);
    assertFalse(body.contains("Fault"), body);
    assertTrue(manager.hasConsumerObserver(), "replacement stream B must stay live");
    manager.cleanup();
  }

  @Test
  void soap11FaultPrefixIsDeclared() throws Exception {
    SecurityProfile profile = SecurityProfile.builder(TEST_PROFILE_NAME).build();
    String fault = driveRequest(envelope11("<test/>"), "text/xml", profile, failedConsumer());

    Element faultcode = findFirst(SoapEnvelopeHelper.parseResponse(fault), "faultcode");
    assertNotNull(faultcode, "SOAP 1.1 fault must carry a faultcode: " + fault);
    String codeValue = faultcode.getTextContent().trim();
    int colon = codeValue.indexOf(':');
    assertTrue(colon > 0, "faultcode must be a prefixed QName: " + codeValue);
    String prefix = codeValue.substring(0, colon);
    assertEquals(
        SOAP11_NS,
        faultcode.lookupNamespaceURI(prefix),
        "faultcode prefix '" + prefix + "' must resolve to the SOAP 1.1 namespace");
  }

  @Test
  void soap12FaultUses12Structure() throws Exception {
    SecurityProfile profile = SecurityProfile.builder(TEST_PROFILE_NAME).build();
    // Driven default: a consumer failure renders the legacy soap:Server code as Receiver.
    String fault =
        driveRequest(envelope12("<test/>"), "application/soap+xml", profile, failedConsumer());
    Document doc = SoapEnvelopeHelper.parseResponse(fault);
    assertEquals(
        SOAP12_NS,
        doc.getDocumentElement().getNamespaceURI(),
        "SOAP 1.2 root must render a 1.2 envelope: " + fault);
    assertNull(findFirst(doc, "faultcode"), "SOAP 1.2 fault must not use faultcode: " + fault);
    Element code = findFirst(doc, "Code");
    assertNotNull(code, "SOAP 1.2 fault must carry Code: " + fault);
    Element value = findChild(code, "Value");
    assertNotNull(value, "SOAP 1.2 Code must carry Value: " + fault);
    assertSoap12Value(value, "Receiver", fault);
    Element reason = findFirst(doc, "Reason");
    assertNotNull(reason, "SOAP 1.2 fault must carry Reason: " + fault);
    Element text = findChild(reason, "Text");
    assertNotNull(text, "SOAP 1.2 Reason must carry Text: " + fault);
    assertEquals(
        "en",
        text.getAttributeNS(XML_NS, "lang"),
        "SOAP 1.2 Text must carry an xml:lang attribute: " + fault);

    // Legacy Client/Server map to Sender/Receiver; the five valid SOAP 1.2 top-level codes pass
    // through; any unknown/unbound custom code falls back to Receiver. Every value is a QName
    // whose prefix resolves to the SOAP 1.2 namespace.
    String[][] codes = {
      {"soap:Client", "Sender"},
      {"soap:Server", "Receiver"},
      {"Sender", "Sender"},
      {"Receiver", "Receiver"},
      {"VersionMismatch", "VersionMismatch"},
      {"MustUnderstand", "MustUnderstand"},
      {"DataEncodingUnknown", "DataEncodingUnknown"},
      {"custom:Foo", "Receiver"},
    };
    for (String[] entry : codes) {
      String rendered = SoapEndpointPublisher.buildSoapFault(entry[0], "boom", "1.2");
      assertNull(findFirst(SoapEnvelopeHelper.parseResponse(rendered), "faultcode"), rendered);
      Element renderedValue = findFirst(SoapEnvelopeHelper.parseResponse(rendered), "Value");
      assertNotNull(renderedValue, "SOAP 1.2 fault must carry Code/Value: " + rendered);
      assertSoap12Value(renderedValue, entry[1], rendered);
    }
  }

  private static void assertSoap12Value(Element value, String expectedLocal, String fault) {
    String text = value.getTextContent().trim();
    int colon = text.indexOf(':');
    assertTrue(colon > 0, "SOAP 1.2 Code/Value must be a prefixed QName: " + text);
    String prefix = text.substring(0, colon);
    assertEquals(expectedLocal, text.substring(colon + 1), "unexpected SOAP 1.2 code: " + text);
    assertEquals(
        SOAP12_NS,
        value.lookupNamespaceURI(prefix),
        "SOAP 1.2 code prefix '" + prefix + "' must resolve to the SOAP 1.2 namespace: " + fault);
  }

  @Test
  void contentTypeFallbackSelectsVersion() throws Exception {
    SecurityProfile profile = SecurityProfile.builder(TEST_PROFILE_NAME).build();

    String fault12 = driveRequest("<not-xml", "application/soap+xml", profile, completedOk());
    Document doc12 = SoapEnvelopeHelper.parseResponse(fault12);
    assertEquals(
        SOAP12_NS,
        doc12.getDocumentElement().getNamespaceURI(),
        "application/soap+xml on an unreadable root must render SOAP 1.2: " + fault12);
    assertNotNull(findFirst(doc12, "Code"), "SOAP 1.2 fault must carry Code: " + fault12);

    String fault11 = driveRequest("<not-xml", "text/xml", profile, completedOk());
    Document doc11 = SoapEnvelopeHelper.parseResponse(fault11);
    assertEquals(
        SOAP11_NS,
        doc11.getDocumentElement().getNamespaceURI(),
        "text/xml on an unreadable root must render SOAP 1.1: " + fault11);
    assertNotNull(findFirst(doc11, "faultcode"), "SOAP 1.1 fault must carry faultcode: " + fault11);
  }

  @Test
  void versionDetectedBeforeWss() throws Exception {
    SecurityProfile secured =
        SecurityProfile.builder(TEST_PROFILE_NAME)
            .keystore(keystorePath.toString(), "changeit")
            .build();
    // Controlled failure at the version-aware overload: no keystore/store load is exercised.
    WssSecurityProcessor recording = spy(new WssSecurityProcessor(secured));
    doThrow(new WSSecurityException(WSSecurityException.ErrorCode.FAILED_CHECK, "forced"))
        .when(recording)
        .processInbound(anyString(), anyString());

    publisher =
        new SoapEndpointPublisher() {
          @Override
          WssSecurityProcessor wssProcessorFor(String profileName, SecurityProfile profile) {
            return recording;
          }
        };
    publisher.bridgeConfig = bridgeConfig;
    publisher.cxfServerManager = cxfServerManager;
    publisher.profileStore = profileStore;
    publisher.vertx = vertx;

    // SOAP 1.2 envelope root with a conflicting text/xml header: the root must win.
    String fault = driveRequest(envelope12("<test/>"), "text/xml", secured, completedOk());

    ArgumentCaptor<String> versionCaptor = ArgumentCaptor.forClass(String.class);
    verify(recording).processInbound(anyString(), versionCaptor.capture());
    assertEquals(
        "1.2",
        versionCaptor.getValue(),
        "WSS must receive the root-detected version, not the content-type fallback");

    Document doc = SoapEnvelopeHelper.parseResponse(fault);
    assertEquals(
        SOAP12_NS,
        doc.getDocumentElement().getNamespaceURI(),
        "the security-failure fault must retain the detected 1.2 envelope: " + fault);
    Element value = findFirst(doc, "Value");
    assertNotNull(value, "SOAP 1.2 fault must carry Code/Value: " + fault);
    String qname = value.getTextContent().trim();
    int colon = qname.indexOf(':');
    assertTrue(colon > 0, "SOAP 1.2 Code/Value must be a prefixed QName: " + qname);
    assertEquals("Sender", qname.substring(colon + 1), "forced security failure is a Sender fault");
    assertEquals(
        SOAP12_NS,
        value.lookupNamespaceURI(qname.substring(0, colon)),
        "SOAP 1.2 code prefix must resolve to the SOAP 1.2 namespace");
  }

  @Test
  void senderFaultIs400AndReceiverFaultIs500() throws Exception {
    SecurityProfile defaults = SecurityProfile.builder(TEST_PROFILE_NAME).build();
    SecurityProfile overridden =
        SecurityProfile.builder(TEST_PROFILE_NAME).faultStatus(403).build();

    installWssProcessor(faultingWssProcessor());

    // Baseline default Sender: SOAP 1.2 WSS failure with no override → 400.
    HttpOutcome baseline =
        driveRequestOutcome(envelope12("<test/>"), "application/soap+xml", defaults, completedOk());
    assertEquals(
        400,
        baseline.status(),
        "SOAP 1.2 default WSS Sender fault must be 400: " + baseline.body());
    assertSoap12FaultLocalName(baseline.body(), "Sender");

    // Sender with a WSS-only override: the same WSS failure → 403, not 400.
    HttpOutcome overriddenSender =
        driveRequestOutcome(
            envelope12("<test/>"), "application/soap+xml", overridden, completedOk());
    assertEquals(
        403,
        overriddenSender.status(),
        "WSS fault_status override must apply to Sender faults: " + overriddenSender.body());
    assertSoap12FaultLocalName(overriddenSender.body(), "Sender");

    // Receiver with the same override: a Rust route fault (soap:Server) → 500 (override
    // must not leak to non-WSS Receiver faults), never 200.
    resetPublisher();
    HttpOutcome route =
        driveRequestOutcome(
            envelope12("<test/>"), "application/soap+xml", overridden, completedServerFault());
    assertEquals(
        500,
        route.status(),
        "SOAP 1.2 Receiver fault must stay 500 despite a WSS-only override: " + route.body());
  }

  @Test
  void soap11AnyFaultIs500() throws Exception {
    SecurityProfile profile = SecurityProfile.builder(TEST_PROFILE_NAME).build();

    // Sender: a controlled WSS security failure on SOAP 1.1 → 500 (not 400).
    installWssProcessor(faultingWssProcessor());
    HttpOutcome sender =
        driveRequestOutcome(envelope11("<test/>"), "text/xml", profile, completedOk());
    assertEquals(500, sender.status(), "SOAP 1.1 Sender fault must be 500: " + sender.body());

    // Receiver: a Rust route fault on SOAP 1.1 → 500.
    resetPublisher();
    HttpOutcome receiver =
        driveRequestOutcome(envelope11("<test/>"), "text/xml", profile, completedServerFault());
    assertEquals(500, receiver.status(), "SOAP 1.1 Receiver fault must be 500: " + receiver.body());
  }

  @ParameterizedTest
  @ValueSource(strings = {"1.1", "1.2"})
  void outboundMalformedRouteResponseIsReceiver(String version) throws Exception {
    assertOutboundResponse(version, "<route-secret>", false, false);
  }

  @ParameterizedTest
  @ValueSource(strings = {"1.1", "1.2"})
  void outboundSigningFailureIsReceiver(String version) throws Exception {
    assertOutboundResponse(version, "<result>route-secret</result>", true, false);
  }

  @ParameterizedTest
  @ValueSource(strings = {"1.1", "1.2"})
  void signedRouteFaultRemainsReceiver(String version) throws Exception {
    assertOutboundResponse(version, "", false, true);
  }

  /** Real HTTP, real inbound signature verification, and the existing outbound processor seam. */
  private void assertOutboundResponse(
      String version, String routePayload, boolean failSigning, boolean routeFault)
      throws Exception {
    SecurityProfile signing =
        SecurityProfile.builder(TEST_PROFILE_NAME)
            .keystore(keystorePath.toString(), "changeit")
            .truststore(keystorePath.toString(), "changeit")
            .sigUser(TestKeystoreHelper.KEY_ALIAS, "changeit")
            .actionsIn("Signature")
            .actionsOut("Signature")
            .faultStatus(418)
            .build();
    String requestXml = "1.2".equals(version) ? envelope12("<test/>") : envelope11("<test/>");
    String signedRequest = new WssSecurityProcessor(signing).processOutbound(requestXml);
    WssSecurityProcessor processor = spy(new WssSecurityProcessor(signing));
    if (failSigning) {
      doThrow(
              new WSSecurityException(
                  WSSecurityException.ErrorCode.FAILURE,
                  new IllegalStateException("signing-secret route-secret")))
          .when(processor)
          .processOutbound(anyString());
    }
    installWssProcessor(processor);
    Vertx real = Vertx.vertx();
    publisher.vertx = real;
    when(bridgeConfig.address()).thenReturn("http://127.0.0.1:0/cxf");
    when(profileStore.getProfile(TEST_PROFILE_NAME)).thenReturn(signing);
    when(cxfServerManager.handleSoapRequest(any()))
        .thenReturn(
            routeFault
                ? completedServerFault()
                : CompletableFuture.completedFuture(
                    ConsumerResponse.newBuilder()
                        .setSecurityProfile(TEST_PROFILE_NAME)
                        .setPayload(ByteString.copyFromUtf8(routePayload))
                        .build()));
    try {
      publisher.publish();
      var response = postPlain(publisher.soapPort().orElseThrow(), signedRequest);
      ArgumentCaptor<ConsumerRequest> requestCaptor =
          ArgumentCaptor.forClass(ConsumerRequest.class);
      verify(cxfServerManager).handleSoapRequest(requestCaptor.capture());
      assertTrue(requestCaptor.getValue().hasSigner(), "inbound verification must succeed");
      verify(processor).processInbound(anyString(), eq(version));
      verify(processor, times(1)).processOutbound(anyString());
      String fault = response.body();
      Document doc = SoapEnvelopeHelper.parseResponse(fault);
      String namespace = "1.2".equals(version) ? SOAP12_NS : SOAP11_NS;
      assertEquals(namespace, doc.getDocumentElement().getNamespaceURI());
      Element code = findFirst(doc, "1.2".equals(version) ? "Value" : "faultcode");
      assertNotNull(code, fault);
      String qname = code.getTextContent().trim();
      int colon = qname.indexOf(':');
      assertTrue(colon > 0, fault);
      assertAll(
          () -> assertEquals(500, response.statusCode(), "security override must not apply"),
          () ->
              assertEquals(
                  "1.2".equals(version) ? "Receiver" : "Server", qname.substring(colon + 1)),
          () -> assertEquals(namespace, code.lookupNamespaceURI(qname.substring(0, colon))),
          () -> assertFalse(fault.contains("signing-secret"), fault),
          () -> assertFalse(fault.contains("route-secret"), fault));
      if (routeFault) {
        assertNotNull(findFirst(doc, "Signature"), "route faults must still be signed");
        new WssSecurityProcessor(signing).processInbound(fault, version);
      }
    } finally {
      publisher.stop();
      real.close().toCompletionStage().toCompletableFuture().get(5, TimeUnit.SECONDS);
    }
  }

  /** A processor whose inbound verification always fails with a WSS security fault. */
  private WssSecurityProcessor faultingWssProcessor() throws Exception {
    SecurityProfile secured =
        SecurityProfile.builder(TEST_PROFILE_NAME)
            .keystore(keystorePath.toString(), "changeit")
            .build();
    WssSecurityProcessor recording = spy(new WssSecurityProcessor(secured));
    doThrow(new WSSecurityException(WSSecurityException.ErrorCode.FAILED_CHECK, "forced"))
        .when(recording)
        .processInbound(anyString(), anyString());
    return recording;
  }

  private static void assertSoap12FaultLocalName(String fault, String expectedLocal)
      throws Exception {
    Document doc = SoapEnvelopeHelper.parseResponse(fault);
    Element value = findFirst(doc, "Value");
    assertNotNull(value, "SOAP 1.2 fault must carry Code/Value: " + fault);
    String qname = value.getTextContent().trim();
    int colon = qname.indexOf(':');
    assertTrue(colon > 0, "SOAP 1.2 Code/Value must be a prefixed QName: " + qname);
    assertEquals(expectedLocal, qname.substring(colon + 1), "unexpected fault code: " + fault);
  }

  /** HTTP outcome captured from one driven request. */
  private record HttpOutcome(int status, String body) {}

  /** Re-creates the plain publisher wired to the shared mocks (no WSS override). */
  private void resetPublisher() {
    publisher = new SoapEndpointPublisher();
    publisher.bridgeConfig = bridgeConfig;
    publisher.cxfServerManager = cxfServerManager;
    publisher.profileStore = profileStore;
    publisher.vertx = vertx;
  }

  /** Installs a publisher whose WSS processor is the supplied (recorded/faulting) instance. */
  private void installWssProcessor(WssSecurityProcessor recording) {
    publisher =
        new SoapEndpointPublisher() {
          @Override
          WssSecurityProcessor wssProcessorFor(String profileName, SecurityProfile profile) {
            return recording;
          }
        };
    publisher.bridgeConfig = bridgeConfig;
    publisher.cxfServerManager = cxfServerManager;
    publisher.profileStore = profileStore;
    publisher.vertx = vertx;
  }

  /** Drives one request through publish → chunk → end and returns the ended response body. */
  private String driveRequest(
      String requestXml,
      String contentType,
      SecurityProfile profile,
      CompletableFuture<ConsumerResponse> consumerResponse)
      throws Exception {
    return driveRequestOutcome(requestXml, contentType, profile, consumerResponse).body();
  }

  /**
   * Drives one request and returns both the captured HTTP status and the ended response body. Uses
   * last-captured-value semantics so a single test can drive the same publisher mock more than
   * once.
   */
  private HttpOutcome driveRequestOutcome(
      String requestXml,
      String contentType,
      SecurityProfile profile,
      CompletableFuture<ConsumerResponse> consumerResponse)
      throws Exception {
    return driveRequestOutcome(requestXml, contentType, profile, consumerResponse, Map.of());
  }

  /**
   * Drives one request and returns both the captured HTTP status and the ended response body. Uses
   * last-captured-value semantics so a single test can drive the same publisher mock more than
   * once. {@code extraHeaders} are appended to the default content-type/soapaction entries.
   */
  private HttpOutcome driveRequestOutcome(
      String requestXml,
      String contentType,
      SecurityProfile profile,
      CompletableFuture<ConsumerResponse> consumerResponse,
      Map<String, String> extraHeaders)
      throws Exception {
    when(profileStore.getProfile(TEST_PROFILE_NAME)).thenReturn(profile);
    when(cxfServerManager.handleSoapRequest(any())).thenReturn(consumerResponse);
    List<Map.Entry<String, String>> entries = new ArrayList<>();
    entries.add(Map.entry("content-type", contentType));
    entries.add(Map.entry("soapaction", "\"urn:test:Operation\""));
    extraHeaders.forEach((key, value) -> entries.add(Map.entry(key, value)));
    when(headers.entries()).thenReturn(entries);

    ArgumentCaptor<Callable> callableCaptor = ArgumentCaptor.forClass(Callable.class);
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<AsyncResult<SoapEndpointPublisher.SoapHttpResult>>> resultHandlerCaptor =
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
    verify(httpServer, atLeastOnce()).requestHandler(requestHandlerCaptor.capture());

    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<Buffer>> chunkCaptor = ArgumentCaptor.forClass(Handler.class);
    @SuppressWarnings("unchecked")
    ArgumentCaptor<Handler<Void>> endCaptor = ArgumentCaptor.forClass(Handler.class);
    when(httpRequest.handler(chunkCaptor.capture())).thenReturn(httpRequest);
    when(httpRequest.endHandler(endCaptor.capture())).thenReturn(httpRequest);

    requestHandlerCaptor.getAllValues().get(lastIndex(requestHandlerCaptor)).handle(httpRequest);
    chunkCaptor
        .getAllValues()
        .get(lastIndex(chunkCaptor))
        .handle(Buffer.buffer(requestXml.getBytes(StandardCharsets.UTF_8)));
    endCaptor.getAllValues().get(lastIndex(endCaptor)).handle((Void) null);

    // No callable = the request never entered the blocking pipeline (transport-availability
    // gate or body-cap rejection answered it on the event loop).
    List<Callable> capturedCallables = callableCaptor.getAllValues();
    if (!capturedCallables.isEmpty()) {
      @SuppressWarnings("unchecked")
      Callable<SoapEndpointPublisher.SoapHttpResult> callable =
          capturedCallables.get(capturedCallables.size() - 1);
      try {
        SoapEndpointPublisher.SoapHttpResult result = callable.call();
        resultHandlerCaptor
            .getAllValues()
            .get(lastIndex(resultHandlerCaptor))
            .handle(Future.succeededFuture(result));
      } catch (Exception e) {
        resultHandlerCaptor
            .getAllValues()
            .get(lastIndex(resultHandlerCaptor))
            .handle(Future.failedFuture(e));
      }
    }

    ArgumentCaptor<Integer> statusCaptor = ArgumentCaptor.forClass(Integer.class);
    verify(httpResponse, atLeastOnce()).setStatusCode(statusCaptor.capture());
    return new HttpOutcome(
        statusCaptor.getAllValues().get(lastIndex(statusCaptor)),
        responseCaptor.getAllValues().get(lastIndex(responseCaptor)));
  }

  private static <T> int lastIndex(ArgumentCaptor<T> captor) {
    return captor.getAllValues().size() - 1;
  }

  private static String envelope11(String body) {
    return "<soapenv:Envelope xmlns:soapenv=\""
        + SOAP11_NS
        + "\"><soapenv:Body>"
        + body
        + "</soapenv:Body></soapenv:Envelope>";
  }

  private static String envelope12(String body) {
    return "<soapenv:Envelope xmlns:soapenv=\""
        + SOAP12_NS
        + "\"><soapenv:Body>"
        + body
        + "</soapenv:Body></soapenv:Envelope>";
  }

  private static CompletableFuture<ConsumerResponse> failedConsumer() {
    return CompletableFuture.failedFuture(new RuntimeException("boom"));
  }

  private static CompletableFuture<ConsumerResponse> completedOk() {
    return CompletableFuture.completedFuture(
        ConsumerResponse.newBuilder()
            .setRequestId("test-ok")
            .setSecurityProfile(TEST_PROFILE_NAME)
            .build());
  }

  /** A Rust consumer response carrying a route fault ({@code soap:Server} → Receiver). */
  private static CompletableFuture<ConsumerResponse> completedServerFault() {
    return CompletableFuture.completedFuture(
        ConsumerResponse.newBuilder()
            .setRequestId("test-server-fault")
            .setSecurityProfile(TEST_PROFILE_NAME)
            .setFault(true)
            .setFaultCode("soap:Server")
            .setFaultString("route failed")
            .build());
  }

  /** The real self-signed certificate stored in the test keystore. */
  private static X509Certificate aliceCertificate() throws Exception {
    KeyStore keyStore = KeyStore.getInstance("JKS");
    try (var in = Files.newInputStream(keystorePath)) {
      keyStore.load(in, TestKeystoreHelper.KEYSTORE_PASSWORD.toCharArray());
    }
    return (X509Certificate) keyStore.getCertificate(TestKeystoreHelper.KEY_ALIAS);
  }

  private static String sha256Hex(byte[] der) throws Exception {
    byte[] digest = MessageDigest.getInstance("SHA-256").digest(der);
    StringBuilder hex = new StringBuilder(digest.length * 2);
    for (byte b : digest) {
      hex.append(Character.forDigit((b >> 4) & 0xf, 16)).append(Character.forDigit(b & 0xf, 16));
    }
    return hex.toString();
  }

  private static Element findFirst(Document doc, String localName) {
    NodeList matches = doc.getElementsByTagNameNS("*", localName);
    return matches.getLength() > 0 ? (Element) matches.item(0) : null;
  }

  private static Element findChild(Element parent, String localName) {
    NodeList children = parent.getChildNodes();
    for (int i = 0; i < children.getLength(); i++) {
      if (children.item(i) instanceof Element element) {
        String ln = element.getLocalName();
        if (ln == null) {
          ln = element.getTagName();
        }
        if (localName.equals(ln)) {
          return element;
        }
      }
    }
    return null;
  }
}
