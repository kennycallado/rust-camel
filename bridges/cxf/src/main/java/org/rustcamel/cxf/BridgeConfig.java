package org.rustcamel.cxf;

import jakarta.enterprise.context.ApplicationScoped;
import org.eclipse.microprofile.config.inject.ConfigProperty;

@ApplicationScoped
public class BridgeConfig {
  @ConfigProperty(name = "cxf.wsdl.path", defaultValue = "")
  String wsdlPath;

  @ConfigProperty(name = "cxf.service.name", defaultValue = "")
  String serviceName;

  @ConfigProperty(name = "cxf.port.name", defaultValue = "")
  String portName;

  @ConfigProperty(name = "cxf.address")
  java.util.Optional<String> address;

  @ConfigProperty(name = "cxf.security.username")
  java.util.Optional<String> securityUsername;

  @ConfigProperty(name = "cxf.security.password")
  java.util.Optional<String> securityPassword;

  @ConfigProperty(name = "cxf.keystore.path")
  java.util.Optional<String> keystorePath;

  @ConfigProperty(name = "cxf.keystore.password")
  java.util.Optional<String> keystorePassword;

  @ConfigProperty(name = "cxf.truststore.path")
  java.util.Optional<String> truststorePath;

  @ConfigProperty(name = "cxf.truststore.password")
  java.util.Optional<String> truststorePassword;

  @ConfigProperty(name = "cxf.security.actions.out", defaultValue = "")
  String securityActionsOut;

  @ConfigProperty(name = "cxf.security.actions.in", defaultValue = "")
  String securityActionsIn;

  @ConfigProperty(name = "cxf.sig.username", defaultValue = "clientkey")
  String sigUsername;

  @ConfigProperty(name = "cxf.sig.password", defaultValue = "")
  String sigPassword;

  @ConfigProperty(name = "cxf.enc.username", defaultValue = "serverkey")
  String encUsername;

  @ConfigProperty(name = "cxf.connection.timeout.ms", defaultValue = "30000")
  int connectionTimeoutMs;

  @ConfigProperty(name = "cxf.max.concurrent.requests", defaultValue = "100")
  int maxConcurrentRequests;

  @ConfigProperty(name = "cxf.consumer.timeout.ms", defaultValue = "60000")
  int consumerTimeoutMs;

  public String wsdlPath() {
    return wsdlPath;
  }

  public String serviceName() {
    return serviceName;
  }

  public String portName() {
    return portName;
  }

  public String address() {
    return address.orElse(null);
  }

  public String securityUsername() {
    return securityUsername.orElse(null);
  }

  public String securityPassword() {
    return securityPassword.orElse(null);
  }

  public String keystorePath() {
    return keystorePath.orElse(null);
  }

  public String keystorePassword() {
    return keystorePassword.orElse(null);
  }

  public String truststorePath() {
    return truststorePath.orElse(null);
  }

  public String truststorePassword() {
    return truststorePassword.orElse(null);
  }

  public String sigUsername() {
    return sigUsername;
  }

  public String sigPassword() {
    return sigPassword;
  }

  public String encUsername() {
    return encUsername;
  }

  public int connectionTimeoutMs() {
    return connectionTimeoutMs;
  }

  public int maxConcurrentRequests() {
    return maxConcurrentRequests;
  }

  public String securityActionsOut() {
    return securityActionsOut;
  }

  public String securityActionsIn() {
    return securityActionsIn;
  }

  public int consumerTimeoutMs() {
    return consumerTimeoutMs;
  }
}
