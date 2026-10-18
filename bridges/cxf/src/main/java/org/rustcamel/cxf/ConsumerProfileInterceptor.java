package org.rustcamel.cxf;

import cxf_bridge.CxfBridgeGrpc;
import io.grpc.Context;
import io.grpc.Contexts;
import io.grpc.Metadata;
import io.grpc.ServerCall;
import io.grpc.ServerCallHandler;
import io.grpc.ServerInterceptor;
import jakarta.enterprise.context.ApplicationScoped;

/**
 * Copies the {@code x-cxf-profile} request header of an {@code OpenConsumerStream} call into the
 * gRPC {@link Context} under {@link #PROFILE_CONTEXT_KEY}.
 *
 * <p>The interceptor only transports the announced profile (possibly {@code null}); it does not
 * validate it. Matching is exact and case-sensitive in {@code CxfBridgeService.openConsumerStream}.
 */
@ApplicationScoped
public class ConsumerProfileInterceptor implements ServerInterceptor {

  static final Metadata.Key<String> PROFILE_METADATA_KEY =
      Metadata.Key.of("x-cxf-profile", Metadata.ASCII_STRING_MARSHALLER);

  static final Context.Key<String> PROFILE_CONTEXT_KEY = Context.key("cxf-consumer-profile");

  @Override
  public <ReqT, RespT> ServerCall.Listener<ReqT> interceptCall(
      ServerCall<ReqT, RespT> call, Metadata headers, ServerCallHandler<ReqT, RespT> next) {
    if (!CxfBridgeGrpc.getOpenConsumerStreamMethod()
        .getFullMethodName()
        .equals(call.getMethodDescriptor().getFullMethodName())) {
      return next.startCall(call, headers);
    }
    Context ctx =
        Context.current().withValue(PROFILE_CONTEXT_KEY, headers.get(PROFILE_METADATA_KEY));
    return Contexts.interceptCall(ctx, call, headers, next);
  }
}
