package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import cxf_bridge.SignerIdentity;
import cxf_bridge.SoapRequest;
import cxf_bridge.SoapResponse;
import io.grpc.Status;
import io.grpc.StatusRuntimeException;
import io.grpc.stub.StreamObserver;
import jakarta.xml.ws.Binding;
import jakarta.xml.ws.Dispatch;
import jakarta.xml.ws.WebServiceException;
import jakarta.xml.ws.soap.SOAPBinding;
import java.io.StringReader;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Set;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import javax.xml.namespace.QName;
import javax.xml.transform.Source;
import javax.xml.transform.stream.StreamSource;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.EnumSource;
import org.mockito.ArgumentCaptor;
import org.mockito.Mock;
import org.mockito.MockitoAnnotations;

@DisplayName("CxfBridgeService")
class CxfBridgeServiceTest {

  private static final long TEST_CAP_BYTES = 1024;
  private static final Pattern OBSERVED_BYTES = Pattern.compile("(\\d+) bytes");

  @Mock BridgeConfig bridgeConfig;
  @Mock CxfClientManager clientManager;
  @Mock SecurityProfileStore profileStore;

  CxfBridgeService service;
  AutoCloseable mocks;

  @BeforeEach
  void setUp() {
    mocks = MockitoAnnotations.openMocks(this);

    service = new CxfBridgeService();
    service.bridgeConfig = bridgeConfig;
    service.clientManager = clientManager;
    service.profileStore = profileStore;

    when(bridgeConfig.connectionTimeoutMs()).thenReturn(5000);
    when(profileStore.getProfile(anyString())).thenReturn(mock(SecurityProfile.class));
  }

  @AfterEach
  void tearDown() throws Exception {
    mocks.close();
  }

  @SuppressWarnings("unchecked")
  private void dispatchReturns(Source response) throws Exception {
    Dispatch<Source> dispatch = mock(Dispatch.class);
    Binding soap11 = binding(SOAPBinding.SOAP11HTTP_BINDING);
    when(dispatch.getBinding()).thenReturn(soap11);
    when(dispatch.invoke(any(Source.class))).thenReturn(response);
    resolvedReturns(dispatch, false);
  }

  private static Binding binding(String bindingId) {
    Binding binding = mock(Binding.class);
    when(binding.getBindingID()).thenReturn(bindingId);
    return binding;
  }

  /** Installs the resolved-dispatch lookup used by the service after Task 1.3. */
  private void resolvedReturns(Dispatch<Source> dispatch, boolean oneWay) throws Exception {
    WsdlOperationCatalog.Operation operation =
        new WsdlOperationCatalog.Operation(
            new QName("urn:test", "Op"),
            Set.of(new QName("urn:test", "req")),
            "urn:test:action",
            oneWay);
    when(clientManager.getResolvedDispatch(
            anyString(),
            anyString(),
            anyString(),
            anyString(),
            anyString(),
            anyString(),
            anyLong(),
            any()))
        .thenReturn(new CxfClientManager.ResolvedDispatch(dispatch, operation));
  }

  /**
   * Installs a dispatch whose binding reports {@code bindingId}, counts invokes, and captures the
   * payload source handed to {@code dispatch.invoke}. The mock performs NO envelope serialization —
   * CXF serializes the request on the wire, not at the dispatch.
   */
  @SuppressWarnings("unchecked")
  private Dispatch<Source> capturingDispatch(
      String bindingId, Source response, AtomicInteger invokeCount, List<String> capturedPayloads)
      throws Exception {
    Dispatch<Source> dispatch = mock(Dispatch.class);
    Binding dispatchBinding = binding(bindingId);
    when(dispatch.getBinding()).thenReturn(dispatchBinding);
    when(dispatch.invoke(any(Source.class)))
        .thenAnswer(
            invocation -> {
              invokeCount.incrementAndGet();
              capturedPayloads.add(SoapEnvelopeHelper.sourceToString(invocation.getArgument(0)));
              return response;
            });
    resolvedReturns(dispatch, false);
    return dispatch;
  }

  private static SoapRequest request() {
    return SoapRequest.newBuilder()
        .setSecurityProfile("prof")
        .setWsdlPath("/fake.wsdl")
        .setServiceName("Svc")
        .setPortName("Port")
        .setAddress("http://localhost:8080/svc")
        .setOperation("op")
        .setPayload(com.google.protobuf.ByteString.copyFromUtf8("<req/>"))
        .build();
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

  @Test
  @DisplayName("oversized producer response fails RESOURCE_EXHAUSTED, no payload forwarded")
  void oversizedResponseRejectedWithResourceExhausted() throws Exception {
    service.pinnedMaxBodyBytes = TEST_CAP_BYTES;
    String oversized = "<response>" + "x".repeat(4096) + "</response>";
    assertTrue(
        oversized.getBytes(StandardCharsets.UTF_8).length > TEST_CAP_BYTES,
        "fixture must serialize past the cap");
    dispatchReturns(new StreamSource(new StringReader(oversized)));
    RecordingObserver observer = new RecordingObserver();

    service.invoke(request(), observer);

    Throwable error = observer.error.get();
    assertNotNull(error, "oversized response must fail the call with onError");
    assertInstanceOf(StatusRuntimeException.class, error);
    StatusRuntimeException sre = (StatusRuntimeException) error;
    assertEquals(Status.Code.RESOURCE_EXHAUSTED, sre.getStatus().getCode());
    String description = sre.getStatus().getDescription();
    assertTrue(
        description != null && description.contains("CXF_MAX_BODY_BYTES"),
        "description must name the env var: " + description);
    Matcher matcher = OBSERVED_BYTES.matcher(description);
    assertTrue(matcher.find(), "description must carry the observed byte count: " + description);
    assertTrue(
        Long.parseLong(matcher.group(1)) > TEST_CAP_BYTES,
        "observed count must exceed the cap: " + description);
    assertTrue(observer.responses.isEmpty(), "no payload may be forwarded");
    assertFalse(observer.completed.get(), "the call must not complete after RESOURCE_EXHAUSTED");
  }

  @Test
  @DisplayName("producer response serializing to exactly the cap passes unchanged")
  void responseAtExactlyCapPasses() throws Exception {
    String base = "<response><data>" + "x".repeat(900) + "</data></response>";
    int trialBytes =
        CxfBridgeService.toXmlString(new StreamSource(new StringReader(base)), Long.MAX_VALUE)
            .getBytes(StandardCharsets.UTF_8)
            .length;
    long padding = TEST_CAP_BYTES - trialBytes;
    assertTrue(padding >= 7, "trial serialization must leave room for an XML comment");

    String comment = "<!--" + "p".repeat((int) padding - 7) + "-->";
    String padded = "<response><data>" + "x".repeat(900) + "</data>" + comment + "</response>";

    String out =
        CxfBridgeService.toXmlString(new StreamSource(new StringReader(padded)), TEST_CAP_BYTES);

    assertEquals(TEST_CAP_BYTES, out.getBytes(StandardCharsets.UTF_8).length);
  }

  @Test
  @DisplayName("SOAP version derives from the binding, not the soap-version header")
  void soapVersionComesFromBindingNotHeader() throws Exception {
    String payload =
        "<hel:sayHelloRequest xmlns:hel=\"http://example.com/hello\">"
            + "<hel:name>World</hel:name></hel:sayHelloRequest>";
    AtomicInteger invokes = new AtomicInteger();
    List<String> dispatched = new ArrayList<>();
    Dispatch<Source> dispatch =
        capturingDispatch(
            SOAPBinding.SOAP11HTTP_BINDING,
            new StreamSource(new StringReader("<resp/>")),
            invokes,
            dispatched);

    SoapRequest req =
        SoapRequest.newBuilder()
            .setSecurityProfile("prof")
            .setWsdlPath("/fake.wsdl")
            .setServiceName("Svc")
            .setPortName("Port")
            .setAddress("http://localhost:8080/svc")
            .setOperation("op")
            .setPayload(com.google.protobuf.ByteString.copyFromUtf8(payload))
            .putHeaders("soap-version", "1.2")
            .build();
    RecordingObserver observer = new RecordingObserver();

    service.invoke(req, observer);

    assertNull(observer.error.get(), "SOAP 1.1 binding must be accepted");
    assertEquals(1, invokes.get(), "the dispatch must be invoked exactly once");
    assertEquals(1, dispatched.size(), "the dispatch must receive one payload");
    assertEquals(
        payload,
        dispatched.get(0),
        "the bare operation element must reach the dispatch unchanged; neither the binding nor the "
            + "soap-version header may wrap it in an Envelope at the dispatch");
    verify(dispatch.getBinding()).getBindingID();
  }

  @Test
  @DisplayName("pre-wrapped SOAP Envelope payload is rejected before any dispatch invoke")
  void prewrappedEnvelopeIsRejectedBeforeInvoke() throws Exception {
    String payload =
        "<soapenv:Envelope xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<soapenv:Body><ping/></soapenv:Body></soapenv:Envelope>";
    AtomicInteger invokes = new AtomicInteger();
    List<String> dispatched = new ArrayList<>();
    capturingDispatch(
        SOAPBinding.SOAP11HTTP_BINDING,
        new StreamSource(new StringReader("<resp/>")),
        invokes,
        dispatched);

    SoapRequest req =
        SoapRequest.newBuilder()
            .setSecurityProfile("prof")
            .setWsdlPath("/fake.wsdl")
            .setServiceName("Svc")
            .setPortName("Port")
            .setAddress("http://localhost:8080/svc")
            .setOperation("op")
            .setPayload(com.google.protobuf.ByteString.copyFromUtf8(payload))
            .build();
    RecordingObserver observer = new RecordingObserver();

    service.invoke(req, observer);

    Throwable error = observer.error.get();
    assertNotNull(error, "a pre-wrapped Envelope must fail the call with onError");
    assertEquals(Status.Code.INVALID_ARGUMENT, Status.fromThrowable(error).getCode());
    assertEquals(0, invokes.get(), "no dispatch invoke may occur for a pre-wrapped Envelope");
  }

  @Test
  @DisplayName("unsupported binding ID fails INVALID_ARGUMENT naming it and dispatches nothing")
  void unsupportedBindingNoDispatch() throws Exception {
    String unsupportedBindingId = "urn:custom";
    AtomicInteger invokes = new AtomicInteger();
    List<String> dispatched = new ArrayList<>();
    capturingDispatch(
        unsupportedBindingId, new StreamSource(new StringReader("<resp/>")), invokes, dispatched);

    SoapRequest req =
        SoapRequest.newBuilder()
            .setSecurityProfile("prof")
            .setWsdlPath("/fake.wsdl")
            .setServiceName("Svc")
            .setPortName("Port")
            .setAddress("http://localhost:8080/svc")
            .setOperation("op")
            .setPayload(com.google.protobuf.ByteString.copyFromUtf8("<req/>"))
            .build();
    RecordingObserver observer = new RecordingObserver();

    service.invoke(req, observer);

    Throwable error = observer.error.get();
    assertNotNull(error, "an unsupported binding ID must fail the call with onError");
    assertEquals(Status.Code.INVALID_ARGUMENT, Status.fromThrowable(error).getCode());
    String description = Status.fromThrowable(error).getDescription();
    assertTrue(
        description != null && description.contains(unsupportedBindingId),
        "description must name the binding ID: " + description);
    assertEquals(0, invokes.get(), "no dispatch invoke may occur for an unsupported binding");
  }

  @Test
  @DisplayName("malformed CXF_MAX_BODY_BYTES value fails loud naming env var and raw value")
  void malformedCapEnvFailsLoud() {
    IllegalStateException ex =
        assertThrows(IllegalStateException.class, () -> BridgeConfig.parseMaxBodyBytes("abc"));
    assertTrue(
        ex.getMessage().contains("CXF_MAX_BODY_BYTES"),
        "failure must name CXF_MAX_BODY_BYTES: " + ex.getMessage());
    assertTrue(ex.getMessage().contains("abc"), "failure must echo the raw value");
  }

  private static SoapRequest requestWithTimeout(long timeoutMs) {
    return SoapRequest.newBuilder()
        .setSecurityProfile("prof")
        .setWsdlPath("/fake.wsdl")
        .setServiceName("Svc")
        .setPortName("Port")
        .setAddress("http://localhost:8080/svc")
        .setOperation("op")
        .setPayload(com.google.protobuf.ByteString.copyFromUtf8("<req/>"))
        .setTimeoutMs(timeoutMs)
        .build();
  }

  @Test
  @DisplayName("legacy negative decoded timeout_ms is rejected INVALID_ARGUMENT before dispatch")
  void legacyNegativeTimeoutIsRejected() throws Exception {
    SoapRequest req = requestWithTimeout(-1L);
    RecordingObserver observer = new RecordingObserver();

    service.invoke(req, observer);

    Throwable error = observer.error.get();
    assertNotNull(error, "a negative decoded timeout must fail with onError");
    assertEquals(Status.Code.INVALID_ARGUMENT, Status.fromThrowable(error).getCode());
    verify(clientManager, never())
        .getResolvedDispatch(
            anyString(),
            anyString(),
            anyString(),
            anyString(),
            anyString(),
            anyString(),
            anyLong(),
            any());
    assertTrue(observer.responses.isEmpty(), "no payload may be forwarded");
  }

  @Test
  @DisplayName("timeout_ms above the 32-bit bound is rejected INVALID_ARGUMENT before dispatch")
  void timeoutAboveI32MaxIsRejected() throws Exception {
    SoapRequest req = requestWithTimeout(2147483648L);
    RecordingObserver observer = new RecordingObserver();

    service.invoke(req, observer);

    Throwable error = observer.error.get();
    assertNotNull(error, "an oversized timeout must fail with onError");
    assertEquals(Status.Code.INVALID_ARGUMENT, Status.fromThrowable(error).getCode());
    verify(clientManager, never())
        .getResolvedDispatch(
            anyString(),
            anyString(),
            anyString(),
            anyString(),
            anyString(),
            anyString(),
            anyLong(),
            any());
    assertTrue(observer.responses.isEmpty(), "no payload may be forwarded");
  }

  @Test
  @DisplayName("in-range timeout_ms is applied to the dispatch")
  void inRangeTimeoutIsApplied() throws Exception {
    dispatchReturns(new StreamSource(new StringReader("<resp/>")));
    SoapRequest req = requestWithTimeout(5000L);
    RecordingObserver observer = new RecordingObserver();

    service.invoke(req, observer);

    assertNull(observer.error.get(), "an in-range timeout must be accepted");
    ArgumentCaptor<Long> timeout = ArgumentCaptor.forClass(Long.class);
    verify(clientManager)
        .getResolvedDispatch(
            anyString(),
            anyString(),
            anyString(),
            anyString(),
            anyString(),
            anyString(),
            timeout.capture(),
            any());
    assertEquals(5000L, timeout.getValue());
  }

  @Test
  @DisplayName("generated SignerIdentity round-trips its four string fields")
  void generatedSignerIdentityRoundTrips() throws Exception {
    SignerIdentity signer =
        SignerIdentity.newBuilder()
            .setSubject("CN=client,OU=pki")
            .setIssuer("CN=root-ca")
            .setSerial("0123abc")
            .setFingerprintSha256("0".repeat(64))
            .build();

    SignerIdentity parsed = SignerIdentity.parseFrom(signer.toByteArray());

    assertEquals(signer, parsed, "serialized SignerIdentity must round-trip unchanged");
    assertEquals("CN=client,OU=pki", parsed.getSubject());
    assertEquals("CN=root-ca", parsed.getIssuer());
    assertEquals("0123abc", parsed.getSerial());
    assertEquals(64, parsed.getFingerprintSha256().length());
  }

  @Test
  @DisplayName("one-way operation invokes once and returns an empty non-fault acknowledgment")
  void oneWayInvokesOnceAndReturnsEmptySuccess() throws Exception {
    Dispatch<Source> dispatch = mock(Dispatch.class);
    Binding soap11 = binding(SOAPBinding.SOAP11HTTP_BINDING);
    when(dispatch.getBinding()).thenReturn(soap11);
    resolvedReturns(dispatch, true);
    RecordingObserver observer = new RecordingObserver();

    service.invoke(request(), observer);

    verify(dispatch).invokeOneWay(any(Source.class));
    verify(dispatch, never()).invoke(any(Source.class));
    assertNull(observer.error.get(), "a one-way dispatch must not fail the call");
    assertEquals(1, observer.responses.size(), "exactly one acknowledgment response");
    SoapResponse ack = observer.responses.get(0);
    assertFalse(ack.getFault(), "the acknowledgment must not be a fault");
    assertEquals(0, ack.getPayload().size(), "one-way acknowledgment carries empty bytes");
    assertTrue(observer.completed.get(), "the observer must complete");
  }

  @Test
  @DisplayName("one-way local failure does not fall back to request-reply or fabricate success")
  void oneWayLocalFailureDoesNotFallbackOrSucceed() throws Exception {
    Dispatch<Source> dispatch = mock(Dispatch.class);
    Binding soap11 = binding(SOAPBinding.SOAP11HTTP_BINDING);
    when(dispatch.getBinding()).thenReturn(soap11);
    doThrow(new WebServiceException("transport down"))
        .when(dispatch)
        .invokeOneWay(any(Source.class));
    resolvedReturns(dispatch, true);
    RecordingObserver observer = new RecordingObserver();

    service.invoke(request(), observer);

    assertNotNull(observer.error.get(), "a local one-way failure must surface as onError");
    assertFalse(observer.completed.get(), "a failed one-way send must not complete successfully");
    assertTrue(observer.responses.isEmpty(), "no success response may be emitted");
    verify(dispatch).invokeOneWay(any(Source.class));
    verify(dispatch, never()).invoke(any(Source.class));
  }

  /** Distinct request-reply failure shapes: absent Source, empty bytes, and a CXF parse failure. */
  private enum ReplyContent {
    NULL_SOURCE,
    ZERO_LENGTH,
    PARSER_EXCEPTION
  }

  @ParameterizedTest
  @EnumSource(ReplyContent.class)
  @DisplayName("request-reply with missing content is an error and never falls back")
  void requestReplyMissingContentIsAnError(ReplyContent content) throws Exception {
    Dispatch<Source> dispatch = mock(Dispatch.class);
    Binding soap11 = binding(SOAPBinding.SOAP11HTTP_BINDING);
    when(dispatch.getBinding()).thenReturn(soap11);
    switch (content) {
      case NULL_SOURCE -> when(dispatch.invoke(any(Source.class))).thenReturn(null);
      case ZERO_LENGTH ->
          when(dispatch.invoke(any(Source.class)))
              .thenReturn(new StreamSource(new StringReader("")));
      case PARSER_EXCEPTION ->
          when(dispatch.invoke(any(Source.class)))
              .thenThrow(new WebServiceException("empty SOAP response"));
    }
    resolvedReturns(dispatch, false);
    RecordingObserver observer = new RecordingObserver();

    service.invoke(request(), observer);

    assertNotNull(observer.error.get(), "missing request-reply content must be an error");
    assertNotEquals(Status.Code.OK, Status.fromThrowable(observer.error.get()).getCode());
    assertTrue(observer.responses.isEmpty(), "no payload may be fabricated");
    verify(dispatch).invoke(any(Source.class));
    verify(dispatch, never()).invokeOneWay(any(Source.class));
  }
}
