package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import org.junit.jupiter.api.Test;

class SecurityConfigTest {

  private SecurityConfig buildConfig(String actionsOut, String actionsIn) {
    BridgeConfig bc = mock(BridgeConfig.class);
    when(bc.securityActionsOut()).thenReturn(actionsOut);
    when(bc.securityActionsIn()).thenReturn(actionsIn);
    when(bc.securityUsername()).thenReturn(null);
    when(bc.securityPassword()).thenReturn(null);
    when(bc.keystorePath()).thenReturn(null);
    when(bc.keystorePassword()).thenReturn(null);
    when(bc.truststorePath()).thenReturn(null);
    when(bc.truststorePassword()).thenReturn(null);
    when(bc.sigUsername()).thenReturn("clientkey");
    when(bc.encUsername()).thenReturn("serverkey");
    return new SecurityConfig(bc);
  }

  @Test
  void resolveActionsOut_defaultsToSignature() {
    assertEquals("Signature", buildConfig("", "").resolveActionsOut());
  }

  @Test
  void resolveActionsOut_defaultsToSignatureWhenNull() {
    assertEquals("Signature", buildConfig(null, "").resolveActionsOut());
  }

  @Test
  void resolveActionsOut_usesExplicitValue() {
    assertEquals("Signature Encrypt", buildConfig("Signature Encrypt", "").resolveActionsOut());
  }

  @Test
  void resolveActionsIn_defaultsToSignature() {
    assertEquals("Signature", buildConfig("", "").resolveActionsIn());
  }

  @Test
  void resolveActionsIn_defaultsToSignatureWhenNull() {
    assertEquals("Signature", buildConfig("", null).resolveActionsIn());
  }

  @Test
  void resolveActionsIn_usesExplicitValue() {
    assertEquals("Signature Encrypt", buildConfig("", "Signature Encrypt").resolveActionsIn());
  }
}
