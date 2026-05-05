package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.mockito.Mock;
import org.mockito.MockitoAnnotations;

class WssSecurityProcessorTest {

  @Mock BridgeConfig config;

  WssSecurityProcessor processor;

  @BeforeEach
  void setUp() {
    MockitoAnnotations.openMocks(this);
    processor = new WssSecurityProcessor();
    processor.config = config;
  }

  // --- canSignOutbound ---

  @Test
  void canSignOutbound_returnsFalse_whenKeystorePathIsNull() {
    when(config.keystorePath()).thenReturn(null);
    assertFalse(processor.canSignOutbound());
  }

  @Test
  void canSignOutbound_returnsFalse_whenKeystorePathIsEmpty() {
    when(config.keystorePath()).thenReturn("");
    assertFalse(processor.canSignOutbound());
  }

  @Test
  void canSignOutbound_returnsFalse_whenKeystorePathIsBlank() {
    when(config.keystorePath()).thenReturn("   ");
    assertFalse(processor.canSignOutbound());
  }

  @Test
  void canSignOutbound_returnsTrue_whenKeystorePathIsSet() {
    when(config.keystorePath()).thenReturn("/path/to/keystore.jks");
    assertTrue(processor.canSignOutbound());
  }

  // --- canVerifyInbound ---

  @Test
  void canVerifyInbound_returnsFalse_whenNeitherKeystoreNorTruststoreSet() {
    when(config.keystorePath()).thenReturn(null);
    when(config.truststorePath()).thenReturn(null);
    assertFalse(processor.canVerifyInbound());
  }

  @Test
  void canVerifyInbound_returnsTrue_whenTruststorePathIsSet() {
    when(config.truststorePath()).thenReturn("/path/to/truststore.jks");
    when(config.keystorePath()).thenReturn(null);
    assertTrue(processor.canVerifyInbound());
  }

  @Test
  void canVerifyInbound_returnsTrue_whenKeystorePathIsSet() {
    when(config.keystorePath()).thenReturn("/path/to/keystore.jks");
    when(config.truststorePath()).thenReturn(null);
    assertTrue(processor.canVerifyInbound());
  }

  // --- isEnabled (backward compat) ---

  @Test
  void isEnabled_returnsFalse_whenNeitherKeystoreNorTruststoreSet() {
    when(config.keystorePath()).thenReturn(null);
    when(config.truststorePath()).thenReturn(null);
    assertFalse(processor.isEnabled());
  }

  @Test
  void isEnabled_returnsTrue_whenKeystorePathIsSet() {
    when(config.keystorePath()).thenReturn("/path/to/keystore.jks");
    assertTrue(processor.isEnabled());
  }

  @Test
  void isEnabled_returnsTrue_whenTruststorePathIsSet() {
    when(config.truststorePath()).thenReturn("/path/to/truststore.jks");
    when(config.keystorePath()).thenReturn(null);
    assertTrue(processor.isEnabled());
  }

  // --- processOutbound / processInbound passthrough when disabled ---

  @Test
  void processOutbound_returnsInput_whenDisabled() throws Exception {
    when(config.keystorePath()).thenReturn(null);
    String input =
        "<soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\"><soap:Body><test/></soap:Body></soap:Envelope>";
    assertEquals(input, processor.processOutbound(input));
  }

  @Test
  void processInbound_returnsInput_whenDisabled() throws Exception {
    when(config.keystorePath()).thenReturn(null);
    when(config.truststorePath()).thenReturn(null);
    String input =
        "<soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\"><soap:Body><test/></soap:Body></soap:Envelope>";
    assertEquals(input, processor.processInbound(input));
  }

  @Test
  void processOutbound_returnsNull_whenInputIsNull() throws Exception {
    when(config.keystorePath()).thenReturn(null);
    assertNull(processor.processOutbound(null));
  }

  @Test
  void processInbound_returnsNull_whenInputIsNull() throws Exception {
    when(config.keystorePath()).thenReturn(null);
    assertNull(processor.processInbound(null));
  }

  @Test
  void processOutbound_returnsBlank_whenInputIsBlank() throws Exception {
    when(config.keystorePath()).thenReturn(null);
    assertEquals("", processor.processOutbound(""));
  }

  @Test
  void processInbound_returnsBlank_whenInputIsBlank() throws Exception {
    when(config.keystorePath()).thenReturn(null);
    assertEquals("", processor.processInbound(""));
  }
}
