package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;

import org.junit.jupiter.api.Test;

class WssSecurityProcessorTest {

  private WssSecurityProcessor withProfile(SecurityProfile profile) {
    return new WssSecurityProcessor(profile);
  }

  // --- canSignOutbound ---

  @Test
  void canSignOutbound_returnsFalse_whenKeystorePathIsNull() {
    SecurityProfile profile = SecurityProfile.builder("test").build();
    assertFalse(withProfile(profile).canSignOutbound());
  }

  @Test
  void canSignOutbound_returnsFalse_whenKeystorePathIsEmpty() {
    SecurityProfile profile = SecurityProfile.builder("test").keystore("", "pass").build();
    assertFalse(withProfile(profile).canSignOutbound());
  }

  @Test
  void canSignOutbound_returnsFalse_whenKeystorePathIsBlank() {
    SecurityProfile profile = SecurityProfile.builder("test").keystore("   ", "pass").build();
    assertFalse(withProfile(profile).canSignOutbound());
  }

  @Test
  void canSignOutbound_returnsTrue_whenKeystorePathIsSet() {
    SecurityProfile profile =
        SecurityProfile.builder("test").keystore("/path/to/keystore.jks", "pass").build();
    assertTrue(withProfile(profile).canSignOutbound());
  }

  // --- canVerifyInbound ---

  @Test
  void canVerifyInbound_returnsFalse_whenNeitherKeystoreNorTruststoreSet() {
    SecurityProfile profile = SecurityProfile.builder("test").build();
    assertFalse(withProfile(profile).canVerifyInbound());
  }

  @Test
  void canVerifyInbound_returnsTrue_whenTruststorePathIsSet() {
    SecurityProfile profile =
        SecurityProfile.builder("test").truststore("/path/to/truststore.jks", "pass").build();
    assertTrue(withProfile(profile).canVerifyInbound());
  }

  @Test
  void canVerifyInbound_returnsTrue_whenKeystorePathIsSet() {
    SecurityProfile profile =
        SecurityProfile.builder("test").keystore("/path/to/keystore.jks", "pass").build();
    assertTrue(withProfile(profile).canVerifyInbound());
  }

  // --- processOutbound / processInbound passthrough when disabled ---

  @Test
  void processOutbound_returnsInput_whenDisabled() throws Exception {
    SecurityProfile profile = SecurityProfile.builder("test").build();
    String input =
        "<soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\"><soap:Body><test/></soap:Body></soap:Envelope>";
    assertEquals(input, withProfile(profile).processOutbound(input));
  }

  @Test
  void processInbound_returnsInput_whenDisabled() throws Exception {
    SecurityProfile profile = SecurityProfile.builder("test").build();
    String input =
        "<soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\"><soap:Body><test/></soap:Body></soap:Envelope>";
    assertEquals(input, withProfile(profile).processInbound(input));
  }

  @Test
  void processOutbound_returnsNull_whenInputIsNull() throws Exception {
    SecurityProfile profile = SecurityProfile.builder("test").build();
    assertNull(withProfile(profile).processOutbound(null));
  }

  @Test
  void processInbound_returnsNull_whenInputIsNull() throws Exception {
    SecurityProfile profile = SecurityProfile.builder("test").build();
    assertNull(withProfile(profile).processInbound(null));
  }

  @Test
  void processOutbound_returnsBlank_whenInputIsBlank() throws Exception {
    SecurityProfile profile = SecurityProfile.builder("test").build();
    assertEquals("", withProfile(profile).processOutbound(""));
  }

  @Test
  void processInbound_returnsBlank_whenInputIsBlank() throws Exception {
    SecurityProfile profile = SecurityProfile.builder("test").build();
    assertEquals("", withProfile(profile).processInbound(""));
  }
}
