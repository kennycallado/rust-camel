package org.rustcamel.cxf;

import jakarta.enterprise.context.ApplicationScoped;
import org.eclipse.microprofile.config.inject.ConfigProperty;

@ApplicationScoped
public class BridgeConfig {
  @ConfigProperty(name = "cxf.address")
  java.util.Optional<String> address;

  @ConfigProperty(name = "cxf.connection.timeout.ms", defaultValue = "30000")
  int connectionTimeoutMs;

  @ConfigProperty(name = "cxf.max.concurrent.requests", defaultValue = "100")
  int maxConcurrentRequests;

  @ConfigProperty(name = "cxf.consumer.timeout.ms", defaultValue = "60000")
  int consumerTimeoutMs;

  public String address() {
    return address.orElse(null);
  }

  public int connectionTimeoutMs() {
    return connectionTimeoutMs;
  }

  public int maxConcurrentRequests() {
    return maxConcurrentRequests;
  }

  public int consumerTimeoutMs() {
    return consumerTimeoutMs;
  }
}
