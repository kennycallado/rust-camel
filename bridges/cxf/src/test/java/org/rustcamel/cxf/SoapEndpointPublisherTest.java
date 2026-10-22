package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import com.google.protobuf.ByteString;
import cxf_bridge.ConsumerRequest;
import cxf_bridge.ConsumerResponse;
import cxf_bridge.SignerIdentity;
import cxf_bridge.SoapHeader;
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
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyStore;
import java.security.MessageDigest;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.Callable;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.stream.Collectors;
import javax.security.auth.x500.X500Principal;
import javax.xml.namespace.QName;
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
  private static final String FIXTURE_SERVICE = "{urn:oneway:363}OneWayService";
  private static final String FIXTURE_PORT = "{urn:oneway:363}MainPort";
  private static final String SOAP11_NS = "http://schemas.xmlsoap.org/soap/envelope/";
  private static final String SOAP12_NS = "http://www.w3.org/2003/05/soap-envelope";
  private static final String XML_NS = "http://www.w3.org/XML/1998/namespace";

  /** The fixture's input-only operation body and declared action ({@code urn:receive}). */
  private static final String RECEIVE_ELEMENT =
      "<r:receiveMessage xmlns:r=\"urn:oneway:363\">hello</r:receiveMessage>";

  /** The fixture's request-reply operation body and declared action ({@code urn:echo}). */
  private static final String ECHO_ELEMENT =
      "<e:echoRequest xmlns:e=\"urn:oneway:363\">hello</e:echoRequest>";

  private static final String APP_HEADER_NS = "http://tempuri.org/commonSchema";
  private static final String APP_HEADER_QNAME = "{" + APP_HEADER_NS + "}requestHeaderType";
  private static final String HEADER_XML =
      "<rh:requestHeaderType xmlns:rh=\"http://tempuri.org/commonSchema\">"
          + "<rh:organisation>28071</rh:organisation></rh:requestHeaderType>";
  private static final String PARTNER_TWO_HEADER_NS = "http://tempuri.org/resources";
  private static final String OTHER_HEADER_QNAME =
      "{" + PARTNER_TWO_HEADER_NS + "}RequestHeaderElement";

  private static final String EMPTY_ACTION_SERVICE = "{urn:oneway:363:empty}EmptyActionService";
  private static final String EMPTY_ACTION_PORT = "{urn:oneway:363:empty}EmptyActionPort";
  private static final String EMPTY_ACTION_ELEMENT =
      "<m:emptyActionMessage xmlns:m=\"urn:oneway:363:empty\">x</m:emptyActionMessage>";

  private static Path keystorePath;
  private static Path expiredKeystorePath;
  private static Path expiredTruststorePath;

  @BeforeAll
  static void setUpKeystore() throws Exception {
    keystorePath = TestKeystoreHelper.createTestKeystore();
    expiredKeystorePath = TestKeystoreHelper.createExpiredTestKeystore();
    expiredTruststorePath =
        TestKeystoreHelper.createTruststoreWithCertificate(
            expiredKeystorePath, TestKeystoreHelper.KEY_ALIAS);
  }

  @AfterAll
  static void tearDownKeystore() throws Exception {
    for (Path store : new Path[] {keystorePath, expiredKeystorePath, expiredTruststorePath}) {
      if (store != null) {
        Files.deleteIfExists(store);
      }
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
    when(cxfServerManager.hasConsumerObserver(anyString())).thenReturn(true);

    // Default profile — no security
    SecurityProfile testProfile = SecurityProfile.builder(TEST_PROFILE_NAME).build();
    when(profileStore.getProfile(TEST_PROFILE_NAME)).thenReturn(testProfile);

    // Every listener bind resolves a catalog for each configured profile (Task 2.1), so the mocked
    // store exposes one profile with valid fixture WSDL metadata by default. Failure-path tests
    // override profiles() with incomplete or broken metadata; producer-only tests never publish().
    when(profileStore.profiles()).thenReturn(Map.of(TEST_PROFILE_NAME, listenerFixtureProfile()));

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
                Map.entry("content-type", "text/xml"), Map.entry("soapaction", "\"urn:echo\"")));

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
    String requestXml = envelope11(ECHO_ELEMENT);

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
    String requestXml = envelope11("<p:echoRequest xmlns:p=\"urn:oneway:363\">X</p:echoRequest>");

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
        "<p:echoRequest xmlns:p=\"urn:oneway:363\">X</p:echoRequest>",
        requestCaptor.getValue().getPayload().toStringUtf8());
  }

  @Test
  void decoyElementNameNotMisExtracted() throws Exception {
    String requestXml =
        "<Envelope xmlns=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<Body><xsd:echoRequest xmlns:xsd=\"urn:oneway:363\">keep</xsd:echoRequest>"
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
        "<xsd:echoRequest xmlns:xsd=\"urn:oneway:363\">keep</xsd:echoRequest>",
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
            .signatureAlgorithm("http://www.w3.org/2001/04/xmldsig-more#rsa-sha256")
            .signatureDigestAlgorithm("http://www.w3.org/2001/04/xmlenc#sha256")
            .build();
    String plainEnvelope = envelope11(ECHO_ELEMENT);
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
    driveRequestOutcome(echoEnvelope11(), "text/xml", plain, completedOk());
    ArgumentCaptor<ConsumerRequest> unsignedCaptor = ArgumentCaptor.forClass(ConsumerRequest.class);
    verify(cxfServerManager, times(2)).handleSoapRequest(unsignedCaptor.capture());
    assertFalse(
        unsignedCaptor.getAllValues().get(1).hasSigner(),
        "An unsigned request must carry no signer");

    // Spoofed signer headers must never mint a signer.
    resetPublisher();
    driveRequestOutcome(
        echoEnvelope11(),
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

  // --- Application SOAP header export (T1, T3, T4, T5, T5b, T5c, T7, T10) ---

  @Test
  void allow_listed_header_is_exported() throws Exception {
    SecurityProfile signing = signingHeaderProfile(APP_HEADER_QNAME);
    String plain = envelope11WithHeaders(appHeader("requestHeaderType", "hello"));
    String signed = new WssSecurityProcessor(signing).processOutbound(plain);

    driveRequestOutcome(signed, "text/xml", signing, completedOk());

    ConsumerRequest dispatched = captureDispatchedRequest(1);
    assertEquals(1, dispatched.getSoapHeadersCount(), "one allow-listed header is exported");
    SoapHeader header = dispatched.getSoapHeaders(0);
    assertEquals(APP_HEADER_NS, header.getNs());
    assertEquals("requestHeaderType", header.getLocalName());
    Element root = SoapEnvelopeHelper.parseResponse(header.getXml()).getDocumentElement();
    assertEquals(APP_HEADER_NS, root.getNamespaceURI());
    assertEquals("requestHeaderType", root.getLocalName());
    assertEquals("hello", root.getTextContent());
  }

  @Test
  void signed_flag_true_for_covered_header() throws Exception {
    SecurityProfile profile = signingHeaderProfile(APP_HEADER_QNAME);
    String signed =
        SignedSoapHeaderEnvelopeFactory.signSoap11(
            envelope11(ECHO_ELEMENT),
            keystorePath,
            TestKeystoreHelper.KEY_ALIAS,
            "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256",
            "http://www.w3.org/2001/04/xmlenc#sha256",
            HEADER_XML,
            true);

    driveRequestOutcome(signed, "text/xml", profile, completedOk());

    ConsumerRequest dispatched = captureDispatchedRequest(1);
    assertEquals(1, dispatched.getSoapHeadersCount(), "one allow-listed header is exported");
    SoapHeader header = dispatched.getSoapHeaders(0);
    assertEquals(APP_HEADER_NS, header.getNs());
    assertEquals("requestHeaderType", header.getLocalName());
    assertTrue(
        header.getSigned(), "a header covered by the verified signature must be marked signed");
  }

  @Test
  void signedHeaderCoverage_bodyOnlySignatureLeavesHeaderUnsigned() throws Exception {
    SecurityProfile profile = signingHeaderProfile(APP_HEADER_QNAME);
    String signed =
        SignedSoapHeaderEnvelopeFactory.signSoap11(
            envelope11(ECHO_ELEMENT),
            keystorePath,
            TestKeystoreHelper.KEY_ALIAS,
            "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256",
            "http://www.w3.org/2001/04/xmlenc#sha256",
            HEADER_XML,
            false);

    driveRequestOutcome(signed, "text/xml", profile, completedOk());

    ConsumerRequest dispatched = captureDispatchedRequest(1);
    assertEquals(1, dispatched.getSoapHeadersCount(), "one allow-listed header is exported");
    SoapHeader header = dispatched.getSoapHeaders(0);
    assertEquals(APP_HEADER_NS, header.getNs());
    assertEquals("requestHeaderType", header.getLocalName());
    assertFalse(header.getSigned(), "a Body-only signature leaves the application header unsigned");
  }

  @Test
  void unlisted_header_is_ignored() throws Exception {
    SecurityProfile profile = headerAllowListProfile(APP_HEADER_QNAME);
    String request =
        envelope11WithHeaders(
            appHeader("requestHeaderType", "allowed"), appHeader("otherHeaderType", "ignored"));

    driveRequestOutcome(request, "text/xml", profile, completedOk());

    ConsumerRequest dispatched = captureDispatchedRequest(1);
    assertEquals(1, dispatched.getSoapHeadersCount(), "only the allow-listed header is exported");
    assertEquals("requestHeaderType", dispatched.getSoapHeaders(0).getLocalName());
  }

  @Test
  void duplicate_qname_faults() throws Exception {
    SecurityProfile profile = headerAllowListProfile(APP_HEADER_QNAME);
    String request =
        envelope11WithHeaders(
            appHeader("requestHeaderType", "TOPSECRET-DUP-1"),
            appHeader("requestHeaderType", "TOPSECRET-DUP-2"));

    HttpOutcome outcome = driveRequestOutcome(request, "text/xml", profile, completedOk());

    assertEquals(500, outcome.status(), outcome.body());
    assertSenderFault(outcome.body(), "1.1");
    assertTrue(
        outcome.body().contains("invalid SOAP header export"),
        "the fault must name the export failure: " + outcome.body());
    assertFalse(
        outcome.body().contains("TOPSECRET-DUP"), "the fault must not carry header content");
    verify(cxfServerManager, never()).handleSoapRequest(any());
  }

  @Test
  void size_caps_fault() throws Exception {
    String singleMarker = "TOPSECRET-SINGLE";
    SecurityProfile singleProfile = headerAllowListProfile(APP_HEADER_QNAME);
    HttpOutcome single =
        driveRequestOutcome(
            envelope11WithHeaders(appHeader("requestHeaderType", singleMarker + "A".repeat(20000))),
            "text/xml",
            singleProfile,
            completedOk());
    assertEquals(500, single.status(), single.body());
    assertSenderFault(single.body(), "1.1");
    assertTrue(single.body().contains("invalid SOAP header export"), single.body());
    assertFalse(single.body().contains(singleMarker), "the fault must not carry header content");
    verify(cxfServerManager, never()).handleSoapRequest(any());

    String aggregateMarker = "TOPSECRET-AGG";
    List<String> localNames = List.of("h1", "h2", "h3", "h4", "h5");
    StringBuilder headers = new StringBuilder();
    for (String localName : localNames) {
      headers.append(appHeader(localName, aggregateMarker + "A".repeat(13900)));
    }
    SecurityProfile aggregateProfile =
        headerAllowListProfile(
            localNames.stream()
                .map(localName -> "{" + APP_HEADER_NS + "}" + localName)
                .toArray(String[]::new));
    HttpOutcome aggregate =
        driveRequestOutcome(
            envelope11WithHeaders(headers.toString()), "text/xml", aggregateProfile, completedOk());
    assertEquals(500, aggregate.status(), aggregate.body());
    assertSenderFault(aggregate.body(), "1.1");
    assertTrue(aggregate.body().contains("invalid SOAP header export"), aggregate.body());
    assertFalse(
        aggregate.body().contains(aggregateMarker), "the fault must not carry header content");
    verify(cxfServerManager, never()).handleSoapRequest(any());
  }

  @Test
  void cap_fault_ignores_fault_status_override() throws Exception {
    SecurityProfile profile =
        SecurityProfile.builder(TEST_PROFILE_NAME)
            .exposedSoapHeaders(List.of(APP_HEADER_QNAME))
            .faultStatus(403)
            .build();

    HttpOutcome outcome =
        driveRequestOutcome(
            envelope11WithHeaders(appHeader("requestHeaderType", "A".repeat(20000))),
            "text/xml",
            profile,
            completedOk());

    assertEquals(500, outcome.status(), "a header-export fault is never a WSS fault");
    assertNotEquals(403, outcome.status());
    assertSenderFault(outcome.body(), "1.1");
    verify(cxfServerManager, never()).handleSoapRequest(any());
  }

  @Test
  void cap_fault_from_verified_wss_ignores_fault_status_override() throws Exception {
    SecurityProfile profile =
        SecurityProfile.builder(TEST_PROFILE_NAME)
            .keystore(keystorePath.toString(), "changeit")
            .truststore(keystorePath.toString(), "changeit")
            .sigUser(TestKeystoreHelper.KEY_ALIAS, "changeit")
            .encUser(TestKeystoreHelper.KEY_ALIAS)
            .actionsOut("Signature")
            .actionsIn("Signature")
            .signatureAlgorithm("http://www.w3.org/2001/04/xmldsig-more#rsa-sha256")
            .signatureDigestAlgorithm("http://www.w3.org/2001/04/xmlenc#sha256")
            .exposedSoapHeaders(List.of(APP_HEADER_QNAME))
            .faultStatus(403)
            .build();

    String marker = "TOPSECRET-WSS";
    String plain =
        envelope11WithHeaders(appHeader("requestHeaderType", marker + "A".repeat(20000)));
    String signed = new WssSecurityProcessor(profile).processOutbound(plain);

    HttpOutcome outcome = driveRequestOutcome(signed, "text/xml", profile, completedOk());

    assertEquals(500, outcome.status(), "a header-export fault from the WSS path is Sender/500");
    assertNotEquals(403, outcome.status());
    assertSenderFault(outcome.body(), "1.1");
    assertTrue(outcome.body().contains("invalid SOAP header export"), outcome.body());
    assertFalse(outcome.body().contains(marker), "the fault must not carry header content");
    verify(cxfServerManager, never()).handleSoapRequest(any());
  }

  @Test
  void exact_limits_reach_the_route() throws Exception {
    String exactContent =
        contentExactlyFilling("requestHeaderType", SoapHeaderExporter.MAX_HEADER_BYTES);
    SecurityProfile exactProfile = headerAllowListProfile(APP_HEADER_QNAME);
    HttpOutcome exact =
        driveRequestOutcome(
            envelope11WithHeaders(appHeader("requestHeaderType", exactContent)),
            "text/xml",
            exactProfile,
            completedOk());
    assertEquals(200, exact.status(), exact.body());
    assertFalse(exact.body().contains("Fault"), exact.body());

    List<String> localNames = List.of("h1", "h2", "h3", "h4", "h5", "h6", "h7", "h8");
    int perHeader = SoapHeaderExporter.MAX_TOTAL_BYTES / localNames.size();
    StringBuilder headers = new StringBuilder();
    for (String localName : localNames) {
      headers.append(appHeader(localName, contentExactlyFilling(localName, perHeader)));
    }
    SecurityProfile totalProfile =
        headerAllowListProfile(
            localNames.stream()
                .map(localName -> "{" + APP_HEADER_NS + "}" + localName)
                .toArray(String[]::new));
    resetPublisher();
    HttpOutcome total =
        driveRequestOutcome(
            envelope11WithHeaders(headers.toString()), "text/xml", totalProfile, completedOk());
    assertEquals(200, total.status(), total.body());
    assertFalse(total.body().contains("Fault"), total.body());

    ArgumentCaptor<ConsumerRequest> requestCaptor = ArgumentCaptor.forClass(ConsumerRequest.class);
    verify(cxfServerManager, times(2)).handleSoapRequest(requestCaptor.capture());
    List<ConsumerRequest> requests = requestCaptor.getAllValues();

    assertEquals(1, requests.get(0).getSoapHeadersCount());
    assertEquals(
        SoapHeaderExporter.MAX_HEADER_BYTES,
        requests.get(0).getSoapHeaders(0).getXml().getBytes(StandardCharsets.UTF_8).length);

    assertEquals(localNames.size(), requests.get(1).getSoapHeadersCount());
    int totalBytes = 0;
    for (SoapHeader header : requests.get(1).getSoapHeadersList()) {
      int bytes = header.getXml().getBytes(StandardCharsets.UTF_8).length;
      assertTrue(
          bytes < SoapHeaderExporter.MAX_HEADER_BYTES,
          "each aggregate header must stay below the single-header cap: " + bytes);
      totalBytes += bytes;
    }
    assertEquals(SoapHeaderExporter.MAX_TOTAL_BYTES, totalBytes);
  }

  @Test
  void signed_flag_false_without_wss() throws Exception {
    SecurityProfile profile = headerAllowListProfile(APP_HEADER_QNAME);

    driveRequestOutcome(
        envelope11WithHeaders(appHeader("requestHeaderType", "hello")),
        "text/xml",
        profile,
        completedOk());

    ConsumerRequest dispatched = captureDispatchedRequest(1);
    assertEquals(1, dispatched.getSoapHeadersCount());
    assertFalse(
        dispatched.getSoapHeaders(0).getSigned(), "the no-WSS path never marks a header as signed");
  }

  @Test
  void several_qnames_per_profile() throws Exception {
    String request =
        envelope11WithHeaders(
            appHeader("requestHeaderType", "one"),
            headerElement(PARTNER_TWO_HEADER_NS, "rh2", "RequestHeaderElement", "two"));

    SecurityProfile partnerTwo =
        listenerHeaderProfile("partner_two", APP_HEADER_QNAME, OTHER_HEADER_QNAME);
    SecurityProfile partnerOne = listenerHeaderProfile("partner_one", APP_HEADER_QNAME);
    when(profileStore.profiles())
        .thenReturn(Map.of("partner_two", partnerTwo, "partner_one", partnerOne));

    // Both requests run through the SAME publisher instance so any per-profile exporter state is
    // shared; only the addressed profile (path + store lookup) changes.
    when(httpRequest.path()).thenReturn("/cxf/partner_two");
    when(profileStore.getProfile("partner_two")).thenReturn(partnerTwo);
    driveRequestOutcome(request, "text/xml", partnerTwo, completedOk());

    when(httpRequest.path()).thenReturn("/cxf/partner_one");
    when(profileStore.getProfile("partner_one")).thenReturn(partnerOne);
    driveRequestOutcome(request, "text/xml", partnerOne, completedOk());

    ArgumentCaptor<ConsumerRequest> requestCaptor = ArgumentCaptor.forClass(ConsumerRequest.class);
    verify(cxfServerManager, times(2)).handleSoapRequest(requestCaptor.capture());
    List<ConsumerRequest> requests = requestCaptor.getAllValues();

    assertEquals("partner_two", requests.get(0).getSecurityProfile());
    assertEquals(
        Set.of(APP_HEADER_QNAME, OTHER_HEADER_QNAME),
        exportedQNames(requests.get(0)),
        "the first profile exports exactly both allow-listed QNames");
    assertEquals("partner_one", requests.get(1).getSecurityProfile());
    assertEquals(
        Set.of(APP_HEADER_QNAME),
        exportedQNames(requests.get(1)),
        "the same request under a narrower profile exports only its one allowed QName");
  }

  @Test
  void expired_pinned_cert_rejected_by_default() throws Exception {
    SecurityProfile profile = expiredPinnedProfile(false);

    HttpOutcome outcome =
        driveRequestOutcome(signedExpiredEnvelope(), "text/xml", profile, completedOk());

    assertEquals(500, outcome.status(), outcome.body());
    assertSenderFault(outcome.body(), "1.1");
    assertTrue(
        outcome.body().contains("The provided certificate is invalid"),
        "the rendered fault must carry the invalidCert message: " + outcome.body());
    verify(cxfServerManager, never()).handleSoapRequest(any());
  }

  @Test
  void expired_pinned_cert_e2e_reaches_route_with_signer() throws Exception {
    SecurityProfile profile = expiredPinnedProfile(true);

    driveRequestOutcome(signedExpiredEnvelope(), "text/xml", profile, completedOk());

    ArgumentCaptor<ConsumerRequest> requestCaptor = ArgumentCaptor.forClass(ConsumerRequest.class);
    verify(cxfServerManager, times(1)).handleSoapRequest(requestCaptor.capture());
    ConsumerRequest dispatched = requestCaptor.getValue();
    assertTrue(dispatched.hasSigner(), "the accepted expired pin must export a signer");
    X509Certificate expired =
        TestKeystoreHelper.certificateFrom(expiredKeystorePath, TestKeystoreHelper.KEY_ALIAS);
    assertEquals(
        sha256Hex(expired.getEncoded()),
        dispatched.getSigner().getFingerprintSha256(),
        "the dispatched signer must be the exact pinned expired certificate");
    assertFalse(
        dispatched.getSigner().getFingerprintSha256().isEmpty(),
        "the dispatched signer fingerprint must be non-empty");
  }

  private static SecurityProfile expiredPinnedProfile(boolean optIn) {
    SecurityProfile.Builder builder =
        SecurityProfile.builder(TEST_PROFILE_NAME)
            .keystore(expiredKeystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .truststore(expiredTruststorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .actionsIn("Signature");
    if (optIn) {
      builder.allowExpiredPinnedCertificate(true);
    }
    return builder.build();
  }

  private static String signedExpiredEnvelope() throws Exception {
    return ExpiredSignedEnvelopeFactory.signSoap11(
        envelope11(ECHO_ELEMENT),
        expiredKeystorePath,
        TestKeystoreHelper.KEY_ALIAS,
        "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256",
        "http://www.w3.org/2001/04/xmlenc#sha256",
        false,
        false);
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
            + "<wanted:echoRequest xmlns:wanted=\"urn:oneway:363\">direct-body</wanted:echoRequest>"
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
        payload.contains("wanted:echoRequest"),
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
            .signatureAlgorithm("http://www.w3.org/2001/04/xmldsig-more#rsa-sha256")
            .signatureDigestAlgorithm("http://www.w3.org/2001/04/xmlenc#sha256")
            .build();
    when(profileStore.getProfile(TEST_PROFILE_NAME)).thenReturn(realProfile);

    String plainEnvelope = envelope11(ECHO_ELEMENT);
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
                Map.entry("content-type", "text/xml"), Map.entry("soapaction", "\"urn:echo\"")));

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

  /** The fixture WSDL path on the test classpath, as the configured filesystem metadata. */
  private static String fixtureWsdlPath() throws Exception {
    URL wsdl = SoapEndpointPublisherTest.class.getResource("/soap-oneway.wsdl");
    assertNotNull(wsdl, "fixture WSDL must be on the test classpath");
    return Path.of(wsdl.toURI()).toString();
  }

  /** A profile carrying the valid fixture WSDL metadata a listener bind requires (Task 2.1). */
  private static SecurityProfile listenerFixtureProfile() throws Exception {
    return listenerProfile(fixtureWsdlPath(), FIXTURE_SERVICE, FIXTURE_PORT);
  }

  /** A listener profile with exactly the supplied WSDL metadata, complete or not. */
  private static SecurityProfile listenerProfile(
      String wsdlPath, String serviceName, String portName) {
    return SecurityProfile.builder(TEST_PROFILE_NAME)
        .wsdlPath(wsdlPath)
        .serviceName(serviceName)
        .portName(portName)
        .build();
  }

  /**
   * Task 2.1: a listener bind serves the whole profile store, so every profile must carry a
   * non-blank WSDL path, service name, and port name, and the configured WSDL must load. Any
   * missing/blank metadata (null, empty, whitespace-only), a missing WSDL file, or an absent
   * service/port must fail listener startup before any HTTP server is created, so no ready SOAP
   * port is ever announced.
   */
  @Test
  void listenerCatalogFailurePreventsBind() throws Exception {
    when(bridgeConfig.address()).thenReturn("http://127.0.0.1:0/soap");

    SecurityProfile[] brokenProfiles = {
      listenerProfile(null, FIXTURE_SERVICE, FIXTURE_PORT),
      listenerProfile("", FIXTURE_SERVICE, FIXTURE_PORT),
      listenerProfile("   ", FIXTURE_SERVICE, FIXTURE_PORT),
      listenerProfile("/nonexistent/cxf-missing.wsdl", FIXTURE_SERVICE, FIXTURE_PORT),
      listenerProfile(fixtureWsdlPath(), null, FIXTURE_PORT),
      listenerProfile(fixtureWsdlPath(), "   ", FIXTURE_PORT),
      listenerProfile(fixtureWsdlPath(), FIXTURE_SERVICE, null),
      listenerProfile(fixtureWsdlPath(), FIXTURE_SERVICE, "   "),
    };

    for (SecurityProfile broken : brokenProfiles) {
      resetPublisher();
      when(profileStore.profiles()).thenReturn(Map.of(TEST_PROFILE_NAME, broken));

      IllegalStateException ex =
          assertThrows(IllegalStateException.class, () -> publisher.publish());
      assertTrue(ex.getMessage().contains(TEST_PROFILE_NAME), ex.getMessage());
      assertTrue(
          publisher.soapPort().isEmpty(), "a failed catalog load must announce no listener port");
    }

    verify(vertx, never()).createHttpServer(any());
  }

  /**
   * Task 2.1: the catalog snapshot is built from the startup profile and never reloaded from later
   * profile metadata; the operation set stays bound to the originally selected port.
   */
  @Test
  void listenerCatalogSnapshotUsesStartupProfile() throws Exception {
    SecurityProfile mainProfile =
        SecurityProfile.builder(TEST_PROFILE_NAME)
            .wsdlPath(fixtureWsdlPath())
            .serviceName("{urn:oneway:363}OneWayService")
            .portName("{urn:oneway:363}MainPort")
            .build();
    when(profileStore.profiles()).thenReturn(Map.of(TEST_PROFILE_NAME, mainProfile));

    publisher.initializeOperationCatalogs();

    WsdlOperationCatalog snapshot = publisher.operationCatalogs.get(TEST_PROFILE_NAME);
    assertNotNull(snapshot, "the startup profile must yield a catalog");
    assertDoesNotThrow(
        () -> snapshot.resolveProducer("EchoOp", null),
        "the snapshot must resolve the original main-port binding");

    // Change the backing mocked profile metadata after initialization.
    SecurityProfile aliasProfile =
        SecurityProfile.builder(TEST_PROFILE_NAME)
            .wsdlPath(fixtureWsdlPath())
            .serviceName("{urn:oneway:363}AliasService")
            .portName("{urn:oneway:363}AliasPort")
            .build();
    when(profileStore.profiles()).thenReturn(Map.of(TEST_PROFILE_NAME, aliasProfile));
    clearInvocations(profileStore);

    assertDoesNotThrow(
        () -> snapshot.resolveProducer("EchoOp", null),
        "the initialized snapshot must ignore later profile metadata changes");
    assertThrows(
        WsdlOperationCatalog.InvalidOperationException.class,
        () -> snapshot.resolveProducer("AliasReceiveMessageOp", null),
        "the alias-only operation must stay unreachable in the initialized main snapshot");
    verify(profileStore, never()).profiles();
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
      when(cxfServerManager.hasConsumerObserver(TEST_PROFILE_NAME)).thenReturn(false);

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
   * The gate asks about the addressed profile only; another profile's live stream is irrelevant.
   */
  @Test
  void gateUsesAddressedProfileNotAnyProfile() throws Exception {
    when(cxfServerManager.hasConsumerObserver(TEST_PROFILE_NAME)).thenReturn(false);
    when(cxfServerManager.hasConsumerObserver("other_profile")).thenReturn(true);

    HttpOutcome outcome =
        driveRequestOutcome(
            envelope11("<test/>"),
            "text/xml",
            SecurityProfile.builder(TEST_PROFILE_NAME).build(),
            completedOk());

    assertEquals(503, outcome.status(), outcome.body());
    verify(httpResponse).putHeader("Retry-After", "1");
    assertTrue(outcome.body().contains("routes unavailable"), outcome.body());
    assertFalse(outcome.body().contains("Envelope"), outcome.body());
    assertFalse(outcome.body().contains("Fault"), outcome.body());
    verify(cxfServerManager).hasConsumerObserver(TEST_PROFILE_NAME);
    verify(cxfServerManager, never()).hasConsumerObserver("other_profile");
    verify(cxfServerManager, never()).handleSoapRequest(any());
  }

  /** With the real manager, a stream for another profile never serves the addressed profile. */
  @Test
  @SuppressWarnings("unchecked")
  void noCrossProfileFallbackWithRealManager() throws Exception {
    Vertx real = Vertx.vertx();
    SoapEndpointPublisher pub = publisherWiredWith(real);
    CxfServerManager manager = new CxfServerManager();
    io.grpc.stub.StreamObserver<ConsumerRequest> otherObserver =
        mock(io.grpc.stub.StreamObserver.class);
    try {
      manager.bridgeConfig = bridgeConfig;
      manager.endpointPublisher = pub;
      pub.cxfServerManager = manager;
      when(bridgeConfig.maxConcurrentRequests()).thenReturn(10);
      when(bridgeConfig.address()).thenReturn("http://127.0.0.1:0/soap");
      manager.setConsumerRequestObserver("other_profile", otherObserver);

      pub.publish();
      assertTrue(pub.soapPort().isPresent());
      java.net.http.HttpResponse<String> response =
          postPlain(pub.soapPort().getAsInt(), envelope11("<test/>"));

      assertEquals(503, response.statusCode(), response.body());
      assertNotNull(response.headers().firstValue("retry-after").orElse(null));
      assertFalse(response.body().contains("Envelope"), response.body());
      verify(otherObserver, never()).onNext(any());
    } finally {
      manager.cleanup();
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
    when(cxfServerManager.hasConsumerObserver(TEST_PROFILE_NAME)).thenReturn(false);

    // Missing signature on a Signature-required profile: still the pre-stream 503.
    HttpOutcome outcome =
        driveRequestOutcome(envelope11("<test/>"), "text/xml", signatureRequired, completedOk());
    assertEquals(503, outcome.status(), "pre-stream gate must win over WSS: " + outcome.body());
    verify(httpResponse).putHeader("Retry-After", "1");
    assertTrue(outcome.body().contains("routes unavailable"), outcome.body());
    assertFalse(outcome.body().contains("Envelope"), outcome.body());
    assertFalse(outcome.body().contains("Fault"), outcome.body());
    verify(cxfServerManager).hasConsumerObserver(TEST_PROFILE_NAME);
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
              manager.setConsumerRequestObserver(TEST_PROFILE_NAME, observerB);
              manager.onConsumerStreamClosed(TEST_PROFILE_NAME, observerA);
              return null;
            })
        .when(observerA)
        .onNext(any());

    manager.setConsumerRequestObserver(TEST_PROFILE_NAME, observerA);

    when(profileStore.getProfile(TEST_PROFILE_NAME))
        .thenReturn(SecurityProfile.builder(TEST_PROFILE_NAME).build());
    when(headers.entries())
        .thenReturn(
            List.of(
                Map.entry("content-type", "text/xml"), Map.entry("soapaction", "\"urn:echo\"")));
    when(bodyBuffer.toString(StandardCharsets.UTF_8)).thenReturn(echoEnvelope11());

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
    chunkCaptor.getValue().handle(Buffer.buffer(echoEnvelope11().getBytes(StandardCharsets.UTF_8)));
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
    assertTrue(
        manager.hasConsumerObserver(TEST_PROFILE_NAME), "replacement stream B must stay live");
    manager.cleanup();
  }

  // ── Task 2.2: operation-aware SOAP action extraction and correlated one-way ACK ──

  /** One header/content-type case for operation-aware SOAP action extraction. */
  private record ActionCase(
      String name,
      String version,
      String contentType,
      Map<String, String> headers,
      boolean permitted) {}

  @Test
  void soap12ContentTypeActionHonorsQuotedPairEscapes() {
    // Reviewer example: an escaped quote inside `note` must not expose its embedded `action=` text
    // as the parameter boundary; the genuine trailing action wins.
    assertEquals(
        "urn:echo",
        SoapEndpointPublisher.extractSoapAction(
            Map.of(
                "content-type",
                "application/soap+xml; note=\"x\\\";action=urn:receive;z=x\"; action=\"urn:echo\"")));

    // An escaped backslash (\\") before the closing quote keeps the parameter boundary intact.
    assertEquals(
        "urn:echo",
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; note=\"a\\\\\"; action=\"urn:echo\"")));

    // A quoted-pair inside the action value itself is part of the value, not a terminator.
    assertEquals(
        "urn:a\"b",
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; action=\"urn:a\\\"b\"")));
  }

  @Test
  void soap12ContentTypeSkipsWhitespaceBeforeQuotedValue() {
    // RFC 9110 permits whitespace after '='; `note= "x;action=urn:receive;z=x"` is a single quoted
    // value, so the embedded action look-alike must be ignored and the real action returned.
    assertEquals(
        "urn:echo",
        SoapEndpointPublisher.extractSoapAction(
            Map.of(
                "content-type",
                "application/soap+xml; note= \"x;action=urn:receive;z=x\"; action=\"urn:echo\"")));

    // Same whitespace shape where the embedded action matches but the genuine quoted action is
    // empty: present-empty semantics win, not the embedded `urn:receive`.
    assertEquals(
        "",
        SoapEndpointPublisher.extractSoapAction(
            Map.of(
                "content-type",
                "application/soap+xml; note= \"x;action=urn:receive;z=x\"; action=\"\"")));

    // Whitespace before an unquoted value stays trimmed, exactly as before.
    assertEquals(
        "urn:echo",
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; action= urn:echo")));
  }

  @Test
  void soap12ContentTypeTreatsApostropheAsDataAndRecoversMalformedParameters() {
    // Reviewer input: an apostrophe is ordinary token data, not a quote delimiter, so the embedded
    // `action=` inside `note='...` terminates at the genuine top-level `;` and the real action
    // wins.
    assertEquals(
        "urn:echo",
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; note='x; action=\"urn:echo\"")));

    // Apostrophe inside an unquoted action value is part of the value verbatim.
    assertEquals(
        "urn:'echo'",
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; action=urn:'echo'")));

    // Unterminated quoted-string: pass 1 ends still inQuote, so the whole trailing segment is
    // swallowed. Nothing after it is live and no action is injected from malformed data.
    assertNull(
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; broken=\"oops; action=urn:conflict")));

    // The same malformed tail is swallowed when it follows the genuine action segment.
    assertEquals(
        "urn:conflict",
        SoapEndpointPublisher.extractSoapAction(
            Map.of(
                "content-type", "application/soap+xml; action=\"urn:conflict\"; broken=\"oops")));

    // A quoted value in a non-action segment spans its semicolons; its embedded action never
    // surfaces and the genuine trailing action wins.
    assertEquals(
        "urn:echo",
        SoapEndpointPublisher.extractSoapAction(
            Map.of(
                "content-type",
                "application/soap+xml; note=\"x\" junk=\"a; action=urn:receive; z=b\";"
                    + " action=urn:echo")));

    // Parameters without '=' or with an empty name are malformed and must not hide the action.
    assertEquals(
        "urn:conflict",
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; noeq; action=\"urn:conflict\"")));

    // Characters after a genuine closing quote up to the next ';' are an ignored tail.
    assertEquals(
        "urn:a",
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; action=\"urn:a\" junk")));
  }

  @Test
  void soap12ContentTypeEdgeCaseMatrix() {
    // Quote at end of header (closed): value parsed.
    assertEquals(
        "urn:echo",
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; action=\"urn:echo\"")));

    // Backslash at end of header inside quotes: quoted-pair never closes, so the trailing segment
    // is unterminated and swallowed → null.
    assertNull(
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; action=\"urn:echo\\")));

    // ';' immediately after the opening quote is quoted data, not a separator.
    assertEquals(
        ";urn:echo",
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; action=\";urn:echo\"")));

    // Empty segments are ignored.
    assertEquals(
        "urn:echo",
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml;; ; action=urn:echo")));

    // Value exactly "" is present-empty, not absence.
    assertEquals(
        "",
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; action=\"\"")));

    // Tab after '=' is skipped like space.
    assertEquals(
        "urn:echo",
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; action=\turn:echo")));

    // '=' inside a quoted value is data.
    assertEquals(
        "urn:a=b",
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; action=\"urn:a=b\"")));

    // First action wins even when a later action segment is present.
    assertEquals(
        "urn:first",
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; action=urn:first; action=urn:second")));

    // Mixed-case name and notaction look-alike.
    assertEquals(
        "urn:echo",
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; AcTiOn=\"urn:echo\"")));
    assertNull(
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; notaction=\"urn:echo\"")));
  }

  @Test
  void soap12ContentTypeActionIgnoresQuotedTailText() {
    // Reviewer inputs: only the interior up to the FIRST unescaped closing quote is the value;
    // later quotes in the segment are ignored tail, never part of the action.
    assertEquals(
        "urn:a",
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; action=\"urn:a\" junk\" x\"")));

    assertEquals(
        "urn:a",
        SoapEndpointPublisher.extractSoapAction(
            Map.of(
                "content-type", "application/soap+xml; action=\"urn:a\" junk\"; charset=utf-8\"")));

    // An escaped interior quote does not close the value; unescaping yields the real action.
    assertEquals(
        "a\"b",
        SoapEndpointPublisher.extractSoapAction(
            Map.of("content-type", "application/soap+xml; action=\"a\\\"b\" tail\" x\"")));
  }

  @Test
  void oneWayActionPresenceSurvivesHttpExtraction() throws Exception {
    SecurityProfile profile = SecurityProfile.builder(TEST_PROFILE_NAME).build();
    List<ActionCase> cases =
        List.of(
            new ActionCase("soap11 absent header", "1.1", "text/xml", Map.of(), true),
            new ActionCase(
                "soap11 matching quoted header",
                "1.1",
                "text/xml",
                Map.of("soapaction", "\"urn:receive\""),
                true),
            new ActionCase(
                "soap11 present-empty header", "1.1", "text/xml", Map.of("soapaction", ""), false),
            new ActionCase(
                "soap11 quoted-empty header",
                "1.1",
                "text/xml",
                Map.of("soapaction", "\"\""),
                false),
            new ActionCase("soap12 absent action", "1.2", "application/soap+xml", Map.of(), true),
            new ActionCase(
                "soap12 matching quoted action",
                "1.2",
                "application/soap+xml; action=\"urn:receive\"",
                Map.of(),
                true),
            new ActionCase(
                "soap12 matching unquoted action",
                "1.2",
                "application/soap+xml; action=urn:receive",
                Map.of(),
                true),
            new ActionCase(
                "soap12 present-empty action",
                "1.2",
                "application/soap+xml; action=",
                Map.of(),
                false),
            new ActionCase(
                "soap12 quoted-empty action",
                "1.2",
                "application/soap+xml; action=\"\"",
                Map.of(),
                false),
            new ActionCase(
                "soap12 mixed-case parameter",
                "1.2",
                "application/soap+xml; AcTiOn=\"urn:receive\"",
                Map.of(),
                true),
            new ActionCase(
                "soap12 unrelated notaction parameter",
                "1.2",
                "application/soap+xml; notaction=\"urn:receive\"",
                Map.of(),
                true),
            new ActionCase(
                "soap12 empty header overrides matching action",
                "1.2",
                "application/soap+xml; action=\"urn:receive\"",
                Map.of("soapaction", ""),
                false));

    for (ActionCase c : cases) {
      resetPublisher();
      clearInvocations(cxfServerManager);
      clearInvocations(httpResponse);
      HttpOutcome outcome =
          driveRequestOutcome(
              receiveEnvelope(c.version()), c.contentType(), profile, completedOk(), c.headers());

      if (c.permitted()) {
        assertEquals(202, outcome.status(), c.name() + ": " + outcome.body());
        assertEquals("", outcome.body(), c.name() + " must be an empty ACK");
        ArgumentCaptor<ConsumerRequest> requestCaptor =
            ArgumentCaptor.forClass(ConsumerRequest.class);
        verify(cxfServerManager, times(1)).handleSoapRequest(requestCaptor.capture());
        assertEquals("ReceiveMessageOp", requestCaptor.getValue().getOperation(), c.name());
      } else {
        int expected = "1.2".equals(c.version()) ? 400 : 500;
        assertEquals(expected, outcome.status(), c.name() + ": " + outcome.body());
        assertTrue(
            outcome.body().contains("Sender") || outcome.body().contains("soap:Client"),
            c.name() + ": " + outcome.body());
        verify(cxfServerManager, never()).handleSoapRequest(any());
      }
    }

    // A declared-empty-action catalog: a present-empty header AGREES with the declaration.
    resetPublisher();
    clearInvocations(cxfServerManager);
    clearInvocations(httpResponse);
    SecurityProfile emptyProfile =
        SecurityProfile.builder(TEST_PROFILE_NAME)
            .wsdlPath(emptyActionWsdlPath())
            .serviceName(EMPTY_ACTION_SERVICE)
            .portName(EMPTY_ACTION_PORT)
            .build();
    when(profileStore.profiles()).thenReturn(Map.of(TEST_PROFILE_NAME, emptyProfile));
    HttpOutcome empty =
        driveRequestOutcome(
            envelope11(EMPTY_ACTION_ELEMENT),
            "text/xml",
            emptyProfile,
            completedOk(),
            Map.of("soapaction", ""));
    assertEquals(202, empty.status(), "declared-empty action must accept a present-empty header");
    ArgumentCaptor<ConsumerRequest> emptyCaptor = ArgumentCaptor.forClass(ConsumerRequest.class);
    verify(cxfServerManager, times(1)).handleSoapRequest(emptyCaptor.capture());
    assertEquals("EmptyActionOp", emptyCaptor.getValue().getOperation());
  }

  @Test
  void oneWayAckWaitsForCorrelatedNonFaultCompletion() throws Exception {
    SecurityProfile signing =
        SecurityProfile.builder(TEST_PROFILE_NAME)
            .keystore(keystorePath.toString(), "changeit")
            .build();
    WssSecurityProcessor processor = spy(new WssSecurityProcessor(signing));
    doReturn(false).when(processor).canVerifyInbound();
    doReturn(true).when(processor).canSignOutbound();
    installWssProcessor(processor);

    AsyncDrive drive = beginAsyncDrive(receiveEnvelope11(), "text/xml", signing, Map.of());

    drive.awaitDispatch();
    verify(httpResponse, never()).end(anyString());

    drive.completion.complete(
        ConsumerResponse.newBuilder()
            .setRequestId("test-oneway-ack")
            .setPayload(ByteString.copyFromUtf8("<serialized>ack</serialized>"))
            .setSecurityProfile(TEST_PROFILE_NAME)
            .build());
    drive.awaitFinished();

    ArgumentCaptor<ConsumerRequest> requestCaptor = ArgumentCaptor.forClass(ConsumerRequest.class);
    verify(cxfServerManager, times(1)).handleSoapRequest(requestCaptor.capture());
    assertEquals("ReceiveMessageOp", requestCaptor.getValue().getOperation());
    assertEquals(202, drive.status(), "one-way non-fault completion must be an empty 202");
    assertEquals("", drive.body(), "the ACK must carry no envelope or payload");
    verify(processor, never()).processOutbound(anyString());
    verify(httpResponse, times(1)).end("");
  }

  @Test
  void oneWayUnknownAmbiguousAndConflictingInputsNeverDispatch() throws Exception {
    SecurityProfile mainOverride = listenerProfileWithFault(418);
    SecurityProfile aliasOverride =
        SecurityProfile.builder(TEST_PROFILE_NAME)
            .wsdlPath(fixtureWsdlPath())
            .serviceName("{urn:oneway:363}AliasService")
            .portName("{urn:oneway:363}AliasPort")
            .faultStatus(418)
            .build();

    record BadCase(
        String name,
        String version,
        String contentType,
        String requestXml,
        Map<String, String> headers,
        SecurityProfile profile,
        boolean alias) {}
    List<BadCase> cases =
        List.of(
            new BadCase(
                "unknown QName 1.1",
                "1.1",
                "text/xml",
                envelope11("<x:unknown xmlns:x=\"urn:unknown\"/>"),
                Map.of(),
                mainOverride,
                false),
            new BadCase(
                "unknown QName 1.2",
                "1.2",
                "application/soap+xml",
                envelope12("<x:unknown xmlns:x=\"urn:unknown\"/>"),
                Map.of(),
                mainOverride,
                false),
            new BadCase(
                "alias-ambiguous input 1.1",
                "1.1",
                "text/xml",
                receiveEnvelope11(),
                Map.of(),
                aliasOverride,
                true),
            new BadCase(
                "alias-ambiguous input 1.2",
                "1.2",
                "application/soap+xml",
                receiveEnvelope12(),
                Map.of(),
                aliasOverride,
                true),
            new BadCase(
                "conflicting action 1.1",
                "1.1",
                "text/xml",
                receiveEnvelope11(),
                Map.of("soapaction", "urn:echo"),
                mainOverride,
                false),
            new BadCase(
                "conflicting action 1.2",
                "1.2",
                "application/soap+xml",
                receiveEnvelope12(),
                Map.of("soapaction", "urn:echo"),
                mainOverride,
                false),
            new BadCase(
                "present-empty action 1.2",
                "1.2",
                "application/soap+xml",
                receiveEnvelope12(),
                Map.of("soapaction", ""),
                mainOverride,
                false));

    for (BadCase c : cases) {
      resetPublisher();
      clearInvocations(cxfServerManager);
      clearInvocations(httpResponse);
      when(profileStore.profiles())
          .thenReturn(Map.of(TEST_PROFILE_NAME, c.alias() ? aliasOverride : mainOverride));
      HttpOutcome outcome =
          driveRequestOutcome(
              c.requestXml(), c.contentType(), c.profile(), completedOk(), c.headers());

      int expected = "1.2".equals(c.version()) ? 400 : 500;
      assertEquals(expected, outcome.status(), c.name() + ": " + outcome.body());
      assertNotEquals(418, outcome.status(), c.name() + " must not take the WSS-only override");
      assertNotEquals(202, outcome.status(), c.name());
      assertTrue(
          outcome.body().contains("Sender") || outcome.body().contains("soap:Client"),
          c.name() + ": " + outcome.body());
      verify(cxfServerManager, never()).handleSoapRequest(any());
    }
  }

  @Test
  void oneWayMissingCatalogFailsClosedWithoutDispatch() throws Exception {
    // Production publish() resolves a catalog for every served profile, so a served profile whose
    // catalog is absent is a broken startup invariant. It must fail closed exactly like an
    // unknown/ambiguous/conflicting selection: Sender fault per version, never the WSS override,
    // never an empty-operation dispatch to Rust.
    SecurityProfile servedButUncataloged = listenerProfileWithFault(418);
    for (String version : new String[] {"1.1", "1.2"}) {
      resetPublisher();
      clearInvocations(cxfServerManager);
      clearInvocations(httpResponse);
      // The profile is still served by getProfile() while the published catalog snapshot is empty.
      when(profileStore.profiles()).thenReturn(Map.of());

      HttpOutcome outcome =
          driveRequestOutcome(
              receiveEnvelope(version),
              soapContentType(version),
              servedButUncataloged,
              completedOk());

      int expected = "1.2".equals(version) ? 400 : 500;
      assertEquals(expected, outcome.status(), version + ": " + outcome.body());
      assertNotEquals(418, outcome.status(), version + " must not take the WSS-only override");
      assertNotEquals(202, outcome.status(), version);
      assertSenderFault(outcome.body(), version);
      verify(cxfServerManager, never()).handleSoapRequest(any());
    }
  }

  @Test
  void oneWayEscapedContentTypeActionConflictNeverDispatches() throws Exception {
    // The escaped quote inside `note` hides an embedded `action=urn:receive`; the real action is
    // the trailing `urn:echo`, which conflicts with the receive operation's declared action.
    String contentType =
        "application/soap+xml; note=\"x\\\";action=urn:receive;z=x\"; action=\"urn:echo\"";
    SecurityProfile served = listenerProfileWithFault(418);
    for (String version : new String[] {"1.1", "1.2"}) {
      resetPublisher();
      clearInvocations(cxfServerManager);
      clearInvocations(httpResponse);
      when(profileStore.profiles()).thenReturn(Map.of(TEST_PROFILE_NAME, served));

      HttpOutcome outcome =
          driveRequestOutcome(receiveEnvelope(version), contentType, served, completedOk());

      int expected = "1.2".equals(version) ? 400 : 500;
      assertEquals(expected, outcome.status(), version + ": " + outcome.body());
      assertNotEquals(418, outcome.status(), version + " must not take the WSS-only override");
      assertNotEquals(202, outcome.status(), version);
      assertSenderFault(outcome.body(), version);
      verify(cxfServerManager, never()).handleSoapRequest(any());
    }
  }

  @Test
  void oneWayWhitespaceBeforeQuotedContentTypeActionNeverDispatches() throws Exception {
    // Whitespace after '=' keeps `note` quoted, so its embedded `action=` is ignored. The real
    // action then conflicts (urn:echo) or is present-empty ("" vs declared urn:receive): both are
    // Sender faults per version and must never dispatch.
    SecurityProfile served = listenerProfileWithFault(418);
    String[] contentTypes = {
      "application/soap+xml; note= \"x;action=urn:receive;z=x\"; action=\"urn:echo\"",
      "application/soap+xml; note= \"x;action=urn:receive;z=x\"; action=\"\""
    };
    for (String contentType : contentTypes) {
      for (String version : new String[] {"1.1", "1.2"}) {
        resetPublisher();
        clearInvocations(cxfServerManager);
        clearInvocations(httpResponse);
        when(profileStore.profiles()).thenReturn(Map.of(TEST_PROFILE_NAME, served));

        HttpOutcome outcome =
            driveRequestOutcome(receiveEnvelope(version), contentType, served, completedOk());

        int expected = "1.2".equals(version) ? 400 : 500;
        String label = version + " " + contentType + ": " + outcome.body();
        assertEquals(expected, outcome.status(), label);
        assertNotEquals(418, outcome.status(), version + " must not take the WSS-only override");
        assertNotEquals(202, outcome.status(), label);
        assertSenderFault(outcome.body(), version);
        verify(cxfServerManager, never()).handleSoapRequest(any());
      }
    }
  }

  @Test
  void oneWayApostropheAndMalformedContentTypeActionNeverDispatch() throws Exception {
    // Reviewer shapes: the apostrophe is token data, so the trailing action is genuine; and in the
    // second, a non-action segment's quoted value spans its semicolons while the genuine action
    // conflicts. Both are Sender faults per version and must never dispatch.
    SecurityProfile served = listenerProfileWithFault(418);
    String[] contentTypes = {
      "application/soap+xml; note='x; action=\"urn:echo\"",
      "application/soap+xml; note=\"x\" junk=\"a; action=urn:receive; z=b\"; action=urn:echo",
      "application/soap+xml; action=\"urn:echo\" junk\" x\""
    };
    for (String contentType : contentTypes) {
      for (String version : new String[] {"1.1", "1.2"}) {
        resetPublisher();
        clearInvocations(cxfServerManager);
        clearInvocations(httpResponse);
        when(profileStore.profiles()).thenReturn(Map.of(TEST_PROFILE_NAME, served));

        HttpOutcome outcome =
            driveRequestOutcome(receiveEnvelope(version), contentType, served, completedOk());

        int expected = "1.2".equals(version) ? 400 : 500;
        String label = version + " " + contentType + ": " + outcome.body();
        assertEquals(expected, outcome.status(), label);
        assertNotEquals(418, outcome.status(), version + " must not take the WSS-only override");
        assertNotEquals(202, outcome.status(), label);
        assertSenderFault(outcome.body(), version);
        verify(cxfServerManager, never()).handleSoapRequest(any());
      }
    }
  }

  @Test
  void oneWayUnterminatedQuotedParameterYieldsAbsentActionAndOneWaySuccess() throws Exception {
    // Pass 1 swallows the entire trailing segment when the quote never closes, so no action is
    // injected. An absent action on the receive operation is a valid selection: the one-way
    // operation dispatches once and completes with the empty 202 ACK — never a fault.
    SecurityProfile profile = listenerProfileWithFault(418);
    String contentType = "application/soap+xml; broken=\"oops; action=urn:receive";
    for (String version : new String[] {"1.1", "1.2"}) {
      resetPublisher();
      clearInvocations(cxfServerManager);
      clearInvocations(httpResponse);
      when(profileStore.profiles()).thenReturn(Map.of(TEST_PROFILE_NAME, profile));

      HttpOutcome outcome =
          driveRequestOutcome(receiveEnvelope(version), contentType, profile, completedOk());

      String label = version + ": " + outcome.body();
      assertEquals(202, outcome.status(), label);
      assertNotEquals(418, outcome.status(), label);
      assertEquals("", outcome.body(), label);
      ArgumentCaptor<ConsumerRequest> requestCaptor =
          ArgumentCaptor.forClass(ConsumerRequest.class);
      verify(cxfServerManager, times(1)).handleSoapRequest(requestCaptor.capture());
      assertEquals("ReceiveMessageOp", requestCaptor.getValue().getOperation(), label);
    }
  }

  @Test
  void oneWayFailuresPreserveAvailabilitySecurityAndFaultMatrix() throws Exception {
    for (String version : new String[] {"1.1", "1.2"}) {
      String contentType = soapContentType(version);
      String request = receiveEnvelope(version);
      int senderStatus = "1.2".equals(version) ? 400 : 500;

      // WSS security failure: a Sender fault where the WSS-only override DOES apply.
      resetPublisher();
      installWssProcessor(faultingWssProcessor());
      clearInvocations(cxfServerManager);
      HttpOutcome wss =
          driveRequestOutcome(request, contentType, listenerProfileWithFault(418), completedOk());
      assertEquals(418, wss.status(), "WSS override must apply: " + wss.body());
      assertTrue(wss.body().contains("Fault"), wss.body());
      verify(cxfServerManager, never()).handleSoapRequest(any());

      // Route soap:Client fault: Sender, and the WSS-only override must NOT apply.
      resetPublisher();
      clearInvocations(cxfServerManager);
      HttpOutcome client =
          driveRequestOutcome(
              request, contentType, listenerProfileWithFault(418), completedClientFault());
      assertEquals(senderStatus, client.status(), "route Client fault: " + client.body());
      assertNotEquals(418, client.status());
      assertDispatchedAsReceive();
      assertSenderFault(client.body(), version);

      // Route soap:Server fault: Receiver 500.
      resetPublisher();
      clearInvocations(cxfServerManager);
      HttpOutcome server =
          driveRequestOutcome(
              request, contentType, listenerProfileWithFault(418), completedServerFault());
      assertEquals(500, server.status(), "route Server fault: " + server.body());
      assertNotEquals(202, server.status());
      assertDispatchedAsReceive();

      // Unknown fault code: Receiver 500.
      resetPublisher();
      clearInvocations(cxfServerManager);
      HttpOutcome unknown =
          driveRequestOutcome(
              request,
              contentType,
              listenerProfileWithFault(418),
              completedFault("custom:Foo", "unknown code"));
      assertEquals(500, unknown.status(), "unknown fault code must be Receiver 500");

      // Signer-descriptor Receiver fault: still Receiver 500.
      resetPublisher();
      clearInvocations(cxfServerManager);
      HttpOutcome descriptor =
          driveRequestOutcome(
              request,
              contentType,
              listenerProfileWithFault(418),
              completedFault("signer:Descriptor", "signer descriptor rejected"));
      assertEquals(500, descriptor.status(), "signer descriptor fault must be Receiver 500");
      assertNotEquals(202, descriptor.status());

      // Timeout: Receiver 500.
      resetPublisher();
      clearInvocations(cxfServerManager);
      when(bridgeConfig.consumerTimeoutMs()).thenReturn(20);
      try {
        HttpOutcome timeout =
            driveRequestOutcome(
                request, contentType, listenerProfileWithFault(418), new CompletableFuture<>());
        assertEquals(500, timeout.status(), "timeout must be Receiver 500");
        assertNotEquals(202, timeout.status());
        assertDispatchedAsReceive();
      } finally {
        when(bridgeConfig.consumerTimeoutMs()).thenReturn(5000);
      }

      // Early unavailable stream: 503 transport availability.
      resetPublisher();
      clearInvocations(cxfServerManager);
      clearInvocations(httpResponse);
      when(cxfServerManager.hasConsumerObserver(TEST_PROFILE_NAME)).thenReturn(false);
      try {
        assertTransportUnavailable(
            driveRequestOutcome(
                request, contentType, listenerProfileWithFault(418), completedOk()));
      } finally {
        when(cxfServerManager.hasConsumerObserver(TEST_PROFILE_NAME)).thenReturn(true);
      }

      // Late unavailable stream: the SAME 503.
      resetPublisher();
      clearInvocations(cxfServerManager);
      clearInvocations(httpResponse);
      assertTransportUnavailable(
          driveRequestOutcome(
              request,
              contentType,
              listenerProfileWithFault(418),
              CompletableFuture.failedFuture(
                  new CxfServerManager.StreamUnavailableException("late"))));
    }
  }

  @Test
  void requestReplySuccessStillWrapsAndSignsOnce() throws Exception {
    SecurityProfile signing =
        SecurityProfile.builder(TEST_PROFILE_NAME)
            .keystore(keystorePath.toString(), "changeit")
            .truststore(keystorePath.toString(), "changeit")
            .sigUser(TestKeystoreHelper.KEY_ALIAS, "changeit")
            .actionsOut("Signature")
            .build();
    WssSecurityProcessor processor = spy(new WssSecurityProcessor(signing));
    doReturn(false).when(processor).canVerifyInbound();
    doReturn(true).when(processor).canSignOutbound();
    installWssProcessor(processor);

    HttpOutcome outcome =
        driveRequestOutcome(
            echoEnvelope11(), "text/xml", signing, completedPayload("<result>ok</result>"));

    assertEquals(200, outcome.status(), outcome.body());
    Document doc = SoapEnvelopeHelper.parseResponse(outcome.body());
    assertEquals(SOAP11_NS, doc.getDocumentElement().getNamespaceURI(), outcome.body());
    assertTrue(outcome.body().contains("result"), outcome.body());
    verify(processor, times(1)).processOutbound(anyString());
  }

  /** A listener profile carrying the main fixture metadata and a WSS fault-status override. */
  private SecurityProfile listenerProfileWithFault(int faultStatus) throws Exception {
    return SecurityProfile.builder(TEST_PROFILE_NAME)
        .wsdlPath(fixtureWsdlPath())
        .serviceName(FIXTURE_SERVICE)
        .portName(FIXTURE_PORT)
        .faultStatus(faultStatus)
        .build();
  }

  /** Asserts the last dispatched consumer request carried the resolved one-way operation name. */
  private void assertDispatchedAsReceive() {
    ArgumentCaptor<ConsumerRequest> requestCaptor = ArgumentCaptor.forClass(ConsumerRequest.class);
    verify(cxfServerManager, atLeastOnce()).handleSoapRequest(requestCaptor.capture());
    assertEquals("ReceiveMessageOp", requestCaptor.getValue().getOperation());
  }

  private static void assertSenderFault(String body, String version) {
    if ("1.2".equals(version)) {
      assertTrue(body.contains("Sender"), body);
    } else {
      assertTrue(body.contains("Client") || body.contains("Sender"), body);
    }
  }

  /** The transport-availability 503: Retry-After, plaintext body, never a fault or 202. */
  private void assertTransportUnavailable(HttpOutcome outcome) {
    assertEquals(503, outcome.status(), outcome.body());
    assertNotEquals(202, outcome.status());
    assertTrue(outcome.body().contains("routes unavailable"), outcome.body());
    assertFalse(outcome.body().contains("Envelope"), outcome.body());
    assertFalse(outcome.body().contains("Fault"), outcome.body());
    verify(httpResponse, atLeastOnce()).putHeader("Retry-After", "1");
  }

  @Test
  void soap11FaultPrefixIsDeclared() throws Exception {
    SecurityProfile profile = SecurityProfile.builder(TEST_PROFILE_NAME).build();
    String fault = driveRequest(echoEnvelope11(), "text/xml", profile, failedConsumer());

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
        driveRequest(echoEnvelope12(), "application/soap+xml", profile, failedConsumer());
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
            echoEnvelope12(), "application/soap+xml", overridden, completedServerFault());
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
        driveRequestOutcome(echoEnvelope11(), "text/xml", profile, completedServerFault());
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
            .signatureAlgorithm("http://www.w3.org/2001/04/xmldsig-more#rsa-sha256")
            .signatureDigestAlgorithm("http://www.w3.org/2001/04/xmlenc#sha256")
            .build();
    String requestXml = echoEnvelope(version);
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

  /**
   * A request whose route completion the test holds: the blocking pipeline runs on a daemon worker
   * so the test thread can inspect the pre-completion state and then complete the correlated
   * future, all with bounded latch waits and no sleeps.
   */
  private static final class AsyncDrive {
    final CompletableFuture<ConsumerResponse> completion = new CompletableFuture<>();
    final CountDownLatch dispatched = new CountDownLatch(1);
    final CountDownLatch finished = new CountDownLatch(1);
    private final ArgumentCaptor<Integer> statusCaptor = ArgumentCaptor.forClass(Integer.class);
    private final ArgumentCaptor<String> bodyCaptor = ArgumentCaptor.forClass(String.class);

    void awaitDispatch() throws InterruptedException {
      assertTrue(
          dispatched.await(5, TimeUnit.SECONDS), "request was never dispatched to the route");
    }

    void awaitFinished() throws InterruptedException {
      assertTrue(finished.await(10, TimeUnit.SECONDS), "request worker did not finish");
    }

    int status() {
      return statusCaptor.getValue();
    }

    String body() {
      return bodyCaptor.getValue();
    }
  }

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
   * once. {@code extraHeaders} are appended to the default content-type entry; no SOAPAction header
   * is added by default, so a body-only (absent action) request is the baseline.
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

  /**
   * Starts a request whose route completion is held by the returned {@link AsyncDrive}. The
   * blocking pipeline runs on a daemon worker; the test waits for dispatch with a bounded latch,
   * inspects the unfinished response, then completes the correlated future. Uses the same capture
   * wiring as {@link #driveRequestOutcome}, only with the completion future under test control.
   */
  private AsyncDrive beginAsyncDrive(
      String requestXml,
      String contentType,
      SecurityProfile profile,
      Map<String, String> extraHeaders)
      throws Exception {
    when(profileStore.getProfile(TEST_PROFILE_NAME)).thenReturn(profile);
    AsyncDrive drive = new AsyncDrive();
    when(cxfServerManager.handleSoapRequest(any()))
        .thenAnswer(
            invocation -> {
              drive.dispatched.countDown();
              return drive.completion;
            });
    List<Map.Entry<String, String>> entries = new ArrayList<>();
    entries.add(Map.entry("content-type", contentType));
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

    when(httpResponse.setStatusCode(drive.statusCaptor.capture())).thenReturn(httpResponse);
    when(httpResponse.end(drive.bodyCaptor.capture())).thenAnswer(i -> Future.succeededFuture());

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

    @SuppressWarnings("unchecked")
    Callable<SoapEndpointPublisher.SoapHttpResult> callable =
        callableCaptor.getAllValues().get(lastIndex(callableCaptor));
    @SuppressWarnings("unchecked")
    Handler<AsyncResult<SoapEndpointPublisher.SoapHttpResult>> resultHandler =
        resultHandlerCaptor.getAllValues().get(lastIndex(resultHandlerCaptor));

    Thread worker =
        new Thread(
            () -> {
              try {
                SoapEndpointPublisher.SoapHttpResult result = callable.call();
                resultHandler.handle(Future.succeededFuture(result));
              } catch (Exception e) {
                resultHandler.handle(Future.failedFuture(e));
              } finally {
                drive.finished.countDown();
              }
            },
            "async-drive");
    worker.setDaemon(true);
    worker.start();
    return drive;
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

  /** SOAP 1.1 envelope with the supplied application header elements before the Body. */
  private static String envelope11WithHeaders(String... headerElements) {
    return "<soapenv:Envelope xmlns:soapenv=\""
        + SOAP11_NS
        + "\"><soapenv:Header>"
        + String.join("", headerElements)
        + "</soapenv:Header><soapenv:Body>"
        + ECHO_ELEMENT
        + "</soapenv:Body></soapenv:Envelope>";
  }

  private static String appHeader(String localName, String content) {
    return headerElement(APP_HEADER_NS, "rh", localName, content);
  }

  private static String headerElement(
      String namespace, String prefix, String localName, String content) {
    return "<" + prefix + ":" + localName + " xmlns:" + prefix + "=\"" + namespace + "\">" + content
        + "</" + prefix + ":" + localName + ">";
  }

  /** A listener profile named for the addressed path, carrying the fixture WSDL metadata. */
  private static SecurityProfile listenerHeaderProfile(String name, String... qNames)
      throws Exception {
    return SecurityProfile.builder(name)
        .wsdlPath(fixtureWsdlPath())
        .serviceName(FIXTURE_SERVICE)
        .portName(FIXTURE_PORT)
        .exposedSoapHeaders(List.of(qNames))
        .build();
  }

  private static Set<String> exportedQNames(ConsumerRequest request) {
    return request.getSoapHeadersList().stream()
        .map(header -> "{" + header.getNs() + "}" + header.getLocalName())
        .collect(Collectors.toSet());
  }

  private static SecurityProfile headerAllowListProfile(String... qNames) {
    return SecurityProfile.builder(TEST_PROFILE_NAME).exposedSoapHeaders(List.of(qNames)).build();
  }

  private static SecurityProfile signingHeaderProfile(String... qNames) {
    return SecurityProfile.builder(TEST_PROFILE_NAME)
        .keystore(keystorePath.toString(), "changeit")
        .truststore(keystorePath.toString(), "changeit")
        .sigUser(TestKeystoreHelper.KEY_ALIAS, "changeit")
        .encUser(TestKeystoreHelper.KEY_ALIAS)
        .actionsOut("Signature")
        .actionsIn("Signature")
        .signatureAlgorithm("http://www.w3.org/2001/04/xmldsig-more#rsa-sha256")
        .signatureDigestAlgorithm("http://www.w3.org/2001/04/xmlenc#sha256")
        .exposedSoapHeaders(List.of(qNames))
        .build();
  }

  /** The serialized byte length the exporter sees for one application header in its envelope. */
  private static int exportedHeaderBytes(String localName, String content) throws Exception {
    Document doc =
        SoapEnvelopeHelper.parseResponse(envelope11WithHeaders(appHeader(localName, content)));
    List<SoapHeader> exported =
        SoapHeaderExporter.export(doc, List.of(new QName(APP_HEADER_NS, localName)), null);
    return exported.get(0).getXml().getBytes(StandardCharsets.UTF_8).length;
  }

  /** ASCII content that makes the serialized header exactly {@code targetBytes} long. */
  private static String contentExactlyFilling(String localName, int targetBytes) throws Exception {
    // Measure with one character so the serializer emits paired tags (an empty element is
    // self-closed and has a different overhead).
    int overhead = exportedHeaderBytes(localName, "A") - 1;
    int length = targetBytes - overhead;
    assertTrue(length > 0, "target must exceed the header's serialization overhead");
    String content = "A".repeat(length);
    assertEquals(
        targetBytes,
        exportedHeaderBytes(localName, content),
        "the constructed header must serialize to exactly " + targetBytes + " bytes");
    return content;
  }

  private ConsumerRequest captureDispatchedRequest(int expectedCalls) {
    ArgumentCaptor<ConsumerRequest> requestCaptor = ArgumentCaptor.forClass(ConsumerRequest.class);
    verify(cxfServerManager, times(expectedCalls)).handleSoapRequest(requestCaptor.capture());
    return requestCaptor.getAllValues().get(expectedCalls - 1);
  }

  private static String envelope12(String body) {
    return "<soapenv:Envelope xmlns:soapenv=\""
        + SOAP12_NS
        + "\"><soapenv:Body>"
        + body
        + "</soapenv:Body></soapenv:Envelope>";
  }

  private static String echoEnvelope11() {
    return envelope11(ECHO_ELEMENT);
  }

  private static String echoEnvelope12() {
    return envelope12(ECHO_ELEMENT);
  }

  private static String receiveEnvelope11() {
    return envelope11(RECEIVE_ELEMENT);
  }

  private static String receiveEnvelope12() {
    return envelope12(RECEIVE_ELEMENT);
  }

  private static String echoEnvelope(String version) {
    return "1.2".equals(version) ? echoEnvelope12() : echoEnvelope11();
  }

  private static String receiveEnvelope(String version) {
    return "1.2".equals(version) ? receiveEnvelope12() : receiveEnvelope11();
  }

  private static String soapContentType(String version) {
    return "1.2".equals(version) ? "application/soap+xml" : "text/xml";
  }

  /** The declared-empty-action fixture WSDL path on the test classpath. */
  private static String emptyActionWsdlPath() throws Exception {
    URL wsdl = SoapEndpointPublisherTest.class.getResource("/soap-empty-action.wsdl");
    assertNotNull(wsdl, "empty-action fixture WSDL must be on the test classpath");
    return Path.of(wsdl.toURI()).toString();
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

  /** A non-fault consumer response retaining the supplied serialized payload. */
  private static CompletableFuture<ConsumerResponse> completedPayload(String payload) {
    return CompletableFuture.completedFuture(
        ConsumerResponse.newBuilder()
            .setRequestId("test-payload")
            .setPayload(ByteString.copyFromUtf8(payload))
            .setSecurityProfile(TEST_PROFILE_NAME)
            .build());
  }

  /** A Rust consumer response with an arbitrary fault code and string. */
  private static CompletableFuture<ConsumerResponse> completedFault(
      String faultCode, String faultString) {
    return CompletableFuture.completedFuture(
        ConsumerResponse.newBuilder()
            .setRequestId("test-fault")
            .setSecurityProfile(TEST_PROFILE_NAME)
            .setFault(true)
            .setFaultCode(faultCode)
            .setFaultString(faultString)
            .build());
  }

  /** A Rust route fault with the legacy {@code soap:Client} Sender code. */
  private static CompletableFuture<ConsumerResponse> completedClientFault() {
    return completedFault("soap:Client", "route rejected");
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
