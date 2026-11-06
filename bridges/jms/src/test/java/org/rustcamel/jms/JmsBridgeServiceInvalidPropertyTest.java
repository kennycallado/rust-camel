package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyMap;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.Mockito.doThrow;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.times;
import static org.mockito.Mockito.verify;

import com.google.protobuf.ByteString;
import io.grpc.Status;
import io.grpc.StatusException;
import io.grpc.StatusRuntimeException;
import io.grpc.stub.StreamObserver;
import jakarta.enterprise.inject.Instance;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import javax.jms.JMSException;
import jms_bridge.SendRequest;
import jms_bridge.SendResponse;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/**
 * gRPC status mapping of {@link JmsBridgeService#send} failures.
 *
 * <p>A rejected property write is a sender error ({@code INVALID_ARGUMENT}) that must leave broker
 * health untouched; every other send failure keeps the generic {@code INTERNAL} mapping.
 */
class JmsBridgeServiceInvalidPropertyTest {

  private JmsBridgeService service;
  private JmsProducer producer;
  private BrokerProbe probe;
  private BridgeHealthCache cache;

  @BeforeEach
  void setUp() {
    service = new JmsBridgeService();
    service.clientFactory = mock(JmsClientFactory.class);
    service.config = new BridgeConfig(Map.of());
    producer = mock(JmsProducer.class);
    service.producer = producer;
    service.consumerFactory = mock(Instance.class);
    probe = mock(BrokerProbe.class);
    cache = new BridgeHealthCache(probe, true, System::currentTimeMillis);
    service.installHealthCacheForTests(cache);
  }

  @AfterEach
  void tearDown() {
    service.shutdown();
  }

  @Test
  void invalid_property_exception_maps_to_invalid_argument() throws Exception {
    doThrow(new InvalidPropertyException("x-crash-proof"))
        .when(producer)
        .send(anyString(), any(), anyMap(), any());
    RecordingObserver<SendResponse> observer = new RecordingObserver<>();

    service.send(request(), observer);

    Throwable error = observer.error.get();
    assertTrue(
        error instanceof StatusException || error instanceof StatusRuntimeException,
        "error must carry a gRPC status: " + error);
    Status status = Status.fromThrowable(error);
    assertEquals(Status.Code.INVALID_ARGUMENT, status.getCode());
    assertTrue(
        status.getDescription().contains("x-crash-proof"),
        "description must name the rejected property: " + status.getDescription());
    verify(producer, times(1)).send(anyString(), any(), anyMap(), any());
  }

  @Test
  void rejected_send_leaves_health_unchanged() throws Exception {
    long generationBefore = cache.generation();
    HealthSnapshot.Status cachedStatusBefore = cache.snapshotValue().status();
    doThrow(new InvalidPropertyException("x-crash-proof"))
        .when(producer)
        .send(anyString(), any(), anyMap(), any());
    RecordingObserver<SendResponse> observer = new RecordingObserver<>();

    service.send(request(), observer);

    assertEquals(generationBefore, cache.generation(), "send must not invalidate the health cache");
    assertEquals(
        cachedStatusBefore,
        cache.snapshotValue().status(),
        "a rejected header must not publish a verified-up snapshot");
  }

  @Test
  void other_jms_exceptions_still_map_to_internal() throws Exception {
    doThrow(new JMSException("broker down"))
        .when(producer)
        .send(anyString(), any(), anyMap(), any());
    RecordingObserver<SendResponse> observer = new RecordingObserver<>();

    service.send(request(), observer);

    Status status = Status.fromThrowable(observer.error.get());
    assertEquals(Status.Code.INTERNAL, status.getCode());
    assertTrue(
        status.getDescription().contains("broker down"),
        "description must carry the broker failure: " + status.getDescription());
  }

  private static SendRequest request() {
    return SendRequest.newBuilder()
        .setDestination("queue:t")
        .setBody(ByteString.copyFromUtf8("payload"))
        .build();
  }

  private static final class RecordingObserver<T> implements StreamObserver<T> {
    private final List<T> values = new CopyOnWriteArrayList<>();
    private final AtomicReference<Throwable> error = new AtomicReference<>();
    private final AtomicBoolean completed = new AtomicBoolean();

    @Override
    public void onNext(T value) {
      values.add(value);
    }

    @Override
    public void onError(Throwable throwable) {
      error.set(throwable);
    }

    @Override
    public void onCompleted() {
      completed.set(true);
    }
  }
}
