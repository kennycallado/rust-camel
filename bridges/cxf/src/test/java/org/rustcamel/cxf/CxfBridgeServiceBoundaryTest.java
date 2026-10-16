package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import cxf_bridge.SoapRequest;
import cxf_bridge.SoapResponse;
import io.grpc.Status;
import io.grpc.stub.StreamObserver;
import io.vertx.core.Vertx;
import io.vertx.core.http.HttpServer;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Error translation at the gRPC service boundary on the real dispatch-creation path (real {@link
 * CxfClientManager}, real WSDL fixture, local wire capture). A valid profile with an unknown
 * explicit operation must surface as an argument-class error naming the operation and binding — not
 * as the profile-not-found NOT_FOUND a blanket IllegalArgumentException mapping produced.
 */
@DisplayName("CxfBridgeService boundary error translation")
class CxfBridgeServiceBoundaryTest {
  private static final String NS = "http://example.com/hello";

  private final List<String> captured = new CopyOnWriteArrayList<>();
  private CxfClientManager manager;
  private CxfBridgeService service;
  private SecurityProfileStore profileStore;
  private Vertx vertx;
  private HttpServer server;

  @BeforeEach
  void setUp() {
    profileStore = mock(SecurityProfileStore.class);
    when(profileStore.getProfile("plain")).thenReturn(SecurityProfile.builder("plain").build());
    when(profileStore.getProfile("nosuchprofile"))
        .thenThrow(new IllegalArgumentException("Unknown security profile: nosuchprofile"));

    manager = new CxfClientManager();
    manager.profileStore = profileStore;

    BridgeConfig bridgeConfig = mock(BridgeConfig.class);
    when(bridgeConfig.connectionTimeoutMs()).thenReturn(5000);

    service = new CxfBridgeService();
    service.bridgeConfig = bridgeConfig;
    service.clientManager = manager;
    service.profileStore = profileStore;

    vertx = Vertx.vertx();
  }

  @AfterEach
  void tearDown() throws Exception {
    manager.close();
    vertx.close().toCompletionStage().toCompletableFuture().get(5, TimeUnit.SECONDS);
  }

  @Test
  @DisplayName(
      "valid profile + unknown operation fails INVALID_ARGUMENT naming operation and binding, pre-wire")
  void unknownOperationIsArgumentErrorNotProfileNotFound() throws Exception {
    startCapture();
    RecordingObserver observer = new RecordingObserver();

    assertTimeoutPreemptively(
        Duration.ofSeconds(10),
        () -> service.invoke(request("plain", "noSuchOperation"), observer));

    Throwable error = observer.error.get();
    assertNotNull(error, "unknown operation must fail the call with onError");
    Status status = Status.fromThrowable(error);
    assertEquals(
        Status.Code.INVALID_ARGUMENT,
        status.getCode(),
        "unknown operation is an argument error, not a missing profile: " + status);
    String description = status.getDescription();
    assertTrue(
        description != null && description.contains("noSuchOperation"),
        "description must name the operation: " + description);
    assertTrue(
        description.contains("HelloBinding"), "description must name the binding: " + description);
    assertTrue(captured.isEmpty(), "unknown operation must fail before any wire transmission");
    assertEquals(0, manager.cacheSize(), "rejected dispatch must not be published to the cache");
    assertTrue(observer.responses.isEmpty(), "no payload may be forwarded");
    assertFalse(observer.completed.get(), "the call must not complete");
  }

  @Test
  @DisplayName("unknown profile still fails NOT_FOUND from profile resolution")
  void unknownProfileStaysNotFound() throws Exception {
    startCapture();
    RecordingObserver observer = new RecordingObserver();

    assertTimeoutPreemptively(
        Duration.ofSeconds(10),
        () -> service.invoke(request("nosuchprofile", "sayHello"), observer));

    Throwable error = observer.error.get();
    assertNotNull(error, "an unknown profile must fail the call with onError");
    Status status = Status.fromThrowable(error);
    assertEquals(Status.Code.NOT_FOUND, status.getCode());
    String description = status.getDescription();
    assertTrue(
        description != null && description.contains("nosuchprofile"),
        "description must name the missing profile: " + description);
    assertTrue(captured.isEmpty(), "no wire transmission may occur without a profile");
    assertEquals(0, manager.cacheSize(), "no dispatch may be created without a profile");
  }

  private void startCapture() throws Exception {
    server =
        vertx
            .createHttpServer()
            .requestHandler(
                request ->
                    request.bodyHandler(
                        body -> {
                          captured.add(body.toString());
                          request.response().end("<unused/>");
                        }))
            .listen(0, "127.0.0.1")
            .toCompletionStage()
            .toCompletableFuture()
            .get(5, TimeUnit.SECONDS);
  }

  private SoapRequest request(String profile, String operation) throws Exception {
    return SoapRequest.newBuilder()
        .setSecurityProfile(profile)
        .setWsdlPath(Path.of(getClass().getResource("/soap-actions.wsdl").toURI()).toString())
        .setServiceName("{" + NS + "}HelloService")
        .setPortName("{" + NS + "}HelloPort")
        .setAddress("http://127.0.0.1:" + server.actualPort() + "/service")
        .setOperation(operation)
        .setPayload(
            com.google.protobuf.ByteString.copyFromUtf8(
                "<sayHelloRequest xmlns=\"" + NS + "\">World</sayHelloRequest>"))
        .setTimeoutMs(2000)
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
}
