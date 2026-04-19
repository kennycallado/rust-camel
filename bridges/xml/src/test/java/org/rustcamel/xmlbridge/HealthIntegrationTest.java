package org.rustcamel.xmlbridge;

import static org.junit.jupiter.api.Assertions.assertEquals;

import io.quarkus.grpc.GrpcClient;
import io.quarkus.test.junit.QuarkusTest;
import org.junit.jupiter.api.Test;
import xml_bridge.HealthCheckRequest;
import xml_bridge.MutinyHealthGrpc;

@QuarkusTest
class HealthIntegrationTest {

  @GrpcClient("health")
  MutinyHealthGrpc.MutinyHealthStub health;

  @Test
  void checkReturnsServing() {
    var resp = health.check(HealthCheckRequest.newBuilder().build()).await().indefinitely();
    assertEquals("SERVING", resp.getStatus());
  }
}
