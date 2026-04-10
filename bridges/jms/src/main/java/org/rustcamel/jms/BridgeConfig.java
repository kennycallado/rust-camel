package org.rustcamel.jms;

import jakarta.enterprise.context.ApplicationScoped;

@ApplicationScoped
public class BridgeConfig {
  public String brokerUrl() {
    return System.getenv().getOrDefault("BRIDGE_BROKER_URL", "tcp://localhost:61616");
  }

  public String brokerType() {
    return System.getenv().getOrDefault("BRIDGE_BROKER_TYPE", "activemq").toLowerCase();
  }

  public String username() {
    return System.getenv("BRIDGE_USERNAME");
  }

  public String password() {
    return System.getenv("BRIDGE_PASSWORD");
  }
}
