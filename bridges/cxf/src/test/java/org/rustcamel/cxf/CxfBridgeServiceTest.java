package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import cxf_bridge.SoapRequest;
import cxf_bridge.SoapResponse;
import io.grpc.Status;
import io.grpc.StatusRuntimeException;
import io.grpc.stub.StreamObserver;
import jakarta.xml.ws.Dispatch;
import java.io.StringReader;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import javax.xml.transform.Source;
import javax.xml.transform.stream.StreamSource;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
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
    when(dispatch.invoke(any(Source.class))).thenReturn(response);
    when(clientManager.getDispatch(
            anyString(),
            anyString(),
            anyString(),
            anyString(),
            anyString(),
            anyString(),
            anyLong()))
        .thenReturn(dispatch);
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
  @DisplayName("malformed CXF_MAX_BODY_BYTES value fails loud naming env var and raw value")
  void malformedCapEnvFailsLoud() {
    IllegalStateException ex =
        assertThrows(IllegalStateException.class, () -> BridgeConfig.parseMaxBodyBytes("abc"));
    assertTrue(
        ex.getMessage().contains("CXF_MAX_BODY_BYTES"),
        "failure must name CXF_MAX_BODY_BYTES: " + ex.getMessage());
    assertTrue(ex.getMessage().contains("abc"), "failure must echo the raw value");
  }
}
