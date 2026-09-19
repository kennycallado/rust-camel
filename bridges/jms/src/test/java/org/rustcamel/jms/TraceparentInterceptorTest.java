package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.Mockito.mock;

import io.grpc.Metadata;
import io.grpc.ServerCall;
import io.grpc.ServerCallHandler;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import java.util.logging.Handler;
import java.util.logging.Level;
import java.util.logging.LogRecord;
import java.util.logging.Logger;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/**
 * traceparent surfacing of {@link TraceparentInterceptor} at the service boundary.
 *
 * <p>An arriving W3C {@code traceparent} must reach the boundary intact: logged verbatim through
 * JUL and visible via {@link TraceparentInterceptor#CONTEXT_KEY} while {@code startCall} executes.
 * Absent or unrelated metadata must pass through untouched: no log record, no context value,
 * exactly one forwarded {@code startCall}.
 */
class TraceparentInterceptorTest {

  private static final String TRACEPARENT =
      "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

  private static final Logger LOGGER = Logger.getLogger(TraceparentInterceptor.class.getName());

  private final TraceparentInterceptor interceptor = new TraceparentInterceptor();
  private final LogCaptureHandler julRecords = new LogCaptureHandler();
  private Level previousLevel;

  @BeforeEach
  void setUp() {
    previousLevel = LOGGER.getLevel();
    LOGGER.setLevel(Level.INFO);
    LOGGER.addHandler(julRecords);
  }

  @AfterEach
  void tearDown() {
    LOGGER.removeHandler(julRecords);
    LOGGER.setLevel(previousLevel);
  }

  @Test
  void traceparentArrivesIntactThroughServiceBoundary() {
    RecordingCallHandler next = new RecordingCallHandler();
    Metadata metadata = new Metadata();
    metadata.put(TraceparentInterceptor.METADATA_KEY, TRACEPARENT);

    ServerCall.Listener<Object> listener = interceptor.interceptCall(call(), metadata, next);

    // grpc >= 1.81 wraps the produced listener (ContextualizedServerCallListener) to keep the
    // attached context current on listener callbacks, so assert delegation, not identity.
    listener.onMessage(new Object());
    assertEquals(1, next.delivered.get());
    assertEquals(TRACEPARENT, next.contextValue.get());
    assertEquals(1, next.calls.get());
    assertEquals(1, julRecords.records.size());
    assertEquals(
        "bridge received traceparent=" + TRACEPARENT, julRecords.records.get(0).getMessage());
  }

  @Test
  void absentMetadataPassesThroughUntouched() {
    RecordingCallHandler next = new RecordingCallHandler();

    ServerCall.Listener<Object> listener = interceptor.interceptCall(call(), new Metadata(), next);

    assertSame(next.listener.get(), listener);
    listener.onMessage(new Object());
    assertEquals(1, next.delivered.get());
    assertEquals(1, next.calls.get());
    assertNull(next.contextValue.get());
    assertTrue(julRecords.records.isEmpty());
  }

  @Test
  void unknownMetadataIsIgnored() {
    RecordingCallHandler next = new RecordingCallHandler();
    Metadata metadata = new Metadata();
    metadata.put(Metadata.Key.of("x-custom", Metadata.ASCII_STRING_MARSHALLER), "unrelated");

    ServerCall.Listener<Object> listener = interceptor.interceptCall(call(), metadata, next);

    assertSame(next.listener.get(), listener);
    listener.onMessage(new Object());
    assertEquals(1, next.delivered.get());
    assertEquals(1, next.calls.get());
    assertNull(next.contextValue.get());
    assertTrue(julRecords.records.isEmpty());
  }

  @SuppressWarnings("unchecked")
  private static ServerCall<Object, Object> call() {
    return mock(ServerCall.class);
  }

  /**
   * Hand-rolled fake that records what {@code startCall} observed (ambient context value,
   * invocation count) and what it produced (a listener that reports delivery).
   */
  private static final class RecordingCallHandler implements ServerCallHandler<Object, Object> {
    final AtomicInteger calls = new AtomicInteger();
    final AtomicInteger delivered = new AtomicInteger();
    final AtomicReference<ServerCall.Listener<Object>> listener = new AtomicReference<>();
    final AtomicReference<String> contextValue = new AtomicReference<>();

    @Override
    public ServerCall.Listener<Object> startCall(
        ServerCall<Object, Object> call, Metadata headers) {
      calls.incrementAndGet();
      contextValue.set(TraceparentInterceptor.CONTEXT_KEY.get());
      ServerCall.Listener<Object> created =
          new ServerCall.Listener<Object>() {
            @Override
            public void onMessage(Object message) {
              delivered.incrementAndGet();
            }
          };
      listener.set(created);
      return created;
    }
  }

  /** JUL handler that captures every record published to the interceptor's logger. */
  private static final class LogCaptureHandler extends Handler {
    final List<LogRecord> records = new ArrayList<>();

    @Override
    public void publish(LogRecord record) {
      records.add(record);
    }

    @Override
    public void flush() {}

    @Override
    public void close() {}
  }
}
