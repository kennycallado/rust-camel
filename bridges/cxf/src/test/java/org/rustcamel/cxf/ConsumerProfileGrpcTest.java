package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.when;

import cxf_bridge.ConsumerRequest;
import cxf_bridge.ConsumerResponse;
import cxf_bridge.CxfBridgeGrpc;
import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import io.grpc.Metadata;
import io.grpc.Status;
import io.grpc.stub.MetadataUtils;
import io.grpc.stub.StreamObserver;
import io.quarkus.test.junit.QuarkusMock;
import io.quarkus.test.junit.QuarkusTest;
import jakarta.inject.Inject;
import java.time.Duration;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.locks.LockSupport;
import java.util.function.BooleanSupplier;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/**
 * Proves over a real gRPC transport that the Context value set by {@link
 * ConsumerProfileInterceptor} reaches the {@code @Blocking} {@code openConsumerStream} handler.
 */
@QuarkusTest
class ConsumerProfileGrpcTest {

  private static final Metadata.Key<String> PROFILE_KEY =
      ConsumerProfileInterceptor.PROFILE_METADATA_KEY;

  @Inject CxfServerManager serverManager;

  @BeforeEach
  void installProfileStore() {
    SecurityProfileStore store = mock(SecurityProfileStore.class);
    when(store.profiles()).thenReturn(Map.of("v11", SecurityProfile.builder("v11").build()));
    QuarkusMock.installMockForType(store, SecurityProfileStore.class);
  }

  @Test
  void streamRegistersUnderAnnouncedProfile() throws Exception {
    ManagedChannel channel = channel();
    try {
      StreamObserver<ConsumerResponse> inbound =
          openStream(channel, "v11", new CompletableFuture<>());
      assertTrue(
          awaitTrue(() -> serverManager.hasConsumerObserver("v11"), Duration.ofSeconds(5)),
          "handler must register the stream under the announced profile");
      inbound.onCompleted();
      assertTrue(
          awaitTrue(() -> !serverManager.hasConsumerObserver("v11"), Duration.ofSeconds(5)),
          "closing the stream must release the profile");
    } finally {
      channel.shutdownNow().awaitTermination(5, TimeUnit.SECONDS);
    }
  }

  @Test
  void miscasedProfileIsRejectedOverRealGrpc() throws Exception {
    ManagedChannel channel = channel();
    StreamObserver<ConsumerResponse> first = null;
    try {
      first = openStream(channel, "v11", new CompletableFuture<>());
      assertTrue(
          awaitTrue(() -> serverManager.hasConsumerObserver("v11"), Duration.ofSeconds(5)),
          "v11 stream must register");

      CompletableFuture<Throwable> error = new CompletableFuture<>();
      StreamObserver<ConsumerResponse> second = openStream(channel, "V11", error);
      Throwable t = error.get(5, TimeUnit.SECONDS);
      assertEquals(Status.Code.INVALID_ARGUMENT, Status.fromThrowable(t).getCode());
      assertFalse(serverManager.hasConsumerObserver("V11"));
      assertTrue(
          serverManager.hasConsumerObserver("v11"),
          "rejected open must not disturb the registered profile");
      second.onCompleted();
    } finally {
      if (first != null) {
        first.onCompleted();
      }
      channel.shutdownNow().awaitTermination(5, TimeUnit.SECONDS);
    }
  }

  private static ManagedChannel channel() {
    return ManagedChannelBuilder.forAddress("localhost", 9001).usePlaintext().build();
  }

  private static StreamObserver<ConsumerResponse> openStream(
      ManagedChannel channel, String profile, CompletableFuture<Throwable> error) {
    Metadata headers = new Metadata();
    headers.put(PROFILE_KEY, profile);
    StreamObserver<ConsumerRequest> inbound =
        new StreamObserver<>() {
          @Override
          public void onNext(ConsumerRequest value) {}

          @Override
          public void onError(Throwable t) {
            error.complete(t);
          }

          @Override
          public void onCompleted() {}
        };
    return CxfBridgeGrpc.newStub(channel)
        .withInterceptors(MetadataUtils.newAttachHeadersInterceptor(headers))
        .openConsumerStream(inbound);
  }

  private static boolean awaitTrue(BooleanSupplier cond, Duration budget) {
    long deadline = System.nanoTime() + budget.toNanos();
    while (!cond.getAsBoolean()) {
      if (System.nanoTime() >= deadline) {
        return cond.getAsBoolean();
      }
      LockSupport.parkNanos(TimeUnit.MILLISECONDS.toNanos(20));
    }
    return true;
  }
}
