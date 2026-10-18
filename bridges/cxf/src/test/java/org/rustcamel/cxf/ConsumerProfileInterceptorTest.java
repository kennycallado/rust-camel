package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import cxf_bridge.CxfBridgeGrpc;
import io.grpc.Metadata;
import io.grpc.MethodDescriptor;
import io.grpc.ServerCall;
import io.grpc.ServerCallHandler;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import org.junit.jupiter.api.Test;

class ConsumerProfileInterceptorTest {

  private final AtomicReference<String> stored = new AtomicReference<>("unset");
  private final AtomicInteger runs = new AtomicInteger();

  @SuppressWarnings({"rawtypes", "unchecked"})
  private void run(MethodDescriptor<?, ?> method, Metadata headers) {
    ServerCall call = mock(ServerCall.class);
    when(call.getMethodDescriptor()).thenReturn(method);
    ServerCallHandler handler =
        (c, h) -> {
          runs.incrementAndGet();
          stored.set(ConsumerProfileInterceptor.PROFILE_CONTEXT_KEY.get());
          return new ServerCall.Listener<Object>() {};
        };
    new ConsumerProfileInterceptor().interceptCall(call, headers, handler);
  }

  @Test
  void copiesProfileHeaderIntoContext() {
    Metadata h = new Metadata();
    h.put(ConsumerProfileInterceptor.PROFILE_METADATA_KEY, "v11");
    run(CxfBridgeGrpc.getOpenConsumerStreamMethod(), h);
    assertEquals("v11", stored.get());
    assertEquals(1, runs.get());
  }

  @Test
  void missingHeaderYieldsNullProfile() {
    run(CxfBridgeGrpc.getOpenConsumerStreamMethod(), new Metadata());
    assertNull(stored.get());
    assertEquals(1, runs.get());
  }

  @Test
  void otherMethodsPassThroughWithoutContext() {
    Metadata h = new Metadata();
    h.put(ConsumerProfileInterceptor.PROFILE_METADATA_KEY, "v11");
    run(CxfBridgeGrpc.getHealthMethod(), h);
    assertEquals(1, runs.get());
    assertNull(stored.get());
  }
}
