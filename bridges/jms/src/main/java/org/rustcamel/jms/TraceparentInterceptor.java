package org.rustcamel.jms;

import io.grpc.Context;
import io.grpc.Contexts;
import io.grpc.Metadata;
import io.grpc.ServerCall;
import io.grpc.ServerCallHandler;
import io.grpc.ServerInterceptor;
import io.quarkus.grpc.GlobalInterceptor;
import jakarta.enterprise.context.ApplicationScoped;
import java.util.logging.Logger;

/**
 * Surfaces the incoming W3C {@code traceparent} metadata at the gRPC service boundary.
 *
 * <p>When the bridge receives a {@code traceparent} header, its exact value is logged (the bridge's
 * traceparent log surface) and stored in the ambient {@link Context} so code behind the service
 * boundary can read it via {@link #CONTEXT_KEY}. Absent or unrelated metadata is forwarded
 * untouched. {@code JmsBridgeService} itself stays unmodified.
 */
@ApplicationScoped
@GlobalInterceptor
public class TraceparentInterceptor implements ServerInterceptor {

  private static final Logger LOG = Logger.getLogger(TraceparentInterceptor.class.getName());

  static final Metadata.Key<String> METADATA_KEY =
      Metadata.Key.of("traceparent", Metadata.ASCII_STRING_MARSHALLER);

  static final Context.Key<String> CONTEXT_KEY = Context.key("bridge-traceparent");

  @Override
  public <ReqT, RespT> ServerCall.Listener<ReqT> interceptCall(
      ServerCall<ReqT, RespT> call, Metadata headers, ServerCallHandler<ReqT, RespT> next) {
    String traceparent = headers.get(METADATA_KEY);
    if (traceparent == null) {
      return next.startCall(call, headers);
    }
    LOG.info("bridge received traceparent=" + traceparent);
    return Contexts.interceptCall(
        Context.current().withValue(CONTEXT_KEY, traceparent), call, headers, next);
  }
}
