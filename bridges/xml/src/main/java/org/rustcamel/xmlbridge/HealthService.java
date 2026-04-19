package org.rustcamel.xmlbridge;

import io.quarkus.grpc.GrpcService;
import io.smallrye.mutiny.Uni;
import xml_bridge.HealthCheckRequest;
import xml_bridge.HealthCheckResponse;
import xml_bridge.MutinyHealthGrpc;

@GrpcService
public class HealthService extends MutinyHealthGrpc.HealthImplBase {
  @Override
  public Uni<HealthCheckResponse> check(HealthCheckRequest request) {
    return Uni.createFrom().item(HealthCheckResponse.newBuilder().setStatus("SERVING").build());
  }
}
