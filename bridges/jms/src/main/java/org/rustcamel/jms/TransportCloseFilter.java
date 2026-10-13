package org.rustcamel.jms;

import io.quarkus.vertx.http.runtime.filters.Filters;
import io.vertx.core.Context;
import io.vertx.core.Vertx;
import io.vertx.core.http.HttpServerResponse;
import io.vertx.ext.web.RoutingContext;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.enterprise.event.Observes;
import java.util.Locale;
import java.util.concurrent.CompletableFuture;

/**
 * Transport-close signal for gRPC streams (rc-icvzl).
 *
 * <p>When the Rust client dies, the TCP connection drops without an HTTP/2 {@code RST_STREAM}.
 * Vert.x gRPC never calls {@code onCancel}, so the bridge cannot observe the broken stream. This
 * filter registers a response {@code closeHandler} for {@code application/grpc} requests and
 * completes a per-stream {@link CompletableFuture}. The future is stored in the Vert.x context
 * local, which Quarkus propagates across the HTTP/gRPC boundary and into the {@code @Blocking}
 * worker that runs {@link JmsBridgeService#subscribe}. Task 5.2a.2 wires the future to {@code
 * cleanupSubscription}; this class only produces the signal.
 */
@ApplicationScoped
public class TransportCloseFilter {

  /** Vert.x context-local key under which the per-stream transport-close future is stored. */
  static final String LOCAL_KEY = "rc-transport-close";

  /**
   * Filter priority. Positive priorities get a negative route order, so this filter runs before the
   * gRPC route, which is a user route registered by {@code GrpcServerRecorder}.
   */
  static final int PRIORITY = 1;

  void register(@Observes Filters filters) {
    filters.register(this::handle, PRIORITY);
  }

  void handle(RoutingContext rc) {
    if (isGrpc(rc)) {
      Context context = Vertx.currentContext();
      if (context != null) {
        CompletableFuture<Void> transportClose = new CompletableFuture<>();
        context.putLocal(LOCAL_KEY, transportClose);
        HttpServerResponse response = rc.response();
        // Vert.x 4.5.32 fires the response closeHandler on a normal HTTP/2 stream close too
        // (handleClose with ended == true), so complete only on a premature close. `ended()` is the
        // pinned discriminator: a socket cut leaves the response un-ended; a normal RPC completion
        // ends it before the handler runs.
        response.closeHandler(
            ignored -> {
              if (!response.ended()) {
                transportClose.complete(null);
              }
            });
      }
    }
    rc.next();
  }

  /**
   * Returns the transport-close future for the current request, read from the Vert.x context local.
   * Safe to call from a {@code @Blocking} worker: Quarkus propagates the duplicated context into
   * the blocking execution.
   *
   * @return the pending future for the current stream, or {@code null} when not on a gRPC request
   */
  static CompletableFuture<Void> current() {
    Context context = Vertx.currentContext();
    if (context == null) {
      return null;
    }
    Object local = context.getLocal(LOCAL_KEY);
    if (local instanceof CompletableFuture<?> future) {
      @SuppressWarnings("unchecked")
      CompletableFuture<Void> typed = (CompletableFuture<Void>) future;
      return typed;
    }
    return null;
  }

  private static boolean isGrpc(RoutingContext rc) {
    String contentType = rc.request().getHeader("content-type");
    return contentType != null
        && contentType.toLowerCase(Locale.ROOT).startsWith("application/grpc");
  }
}
