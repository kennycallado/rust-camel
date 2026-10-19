package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Set;
import org.apache.wss4j.common.crypto.AlgorithmSuite;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.apache.wss4j.dom.engine.WSSecurityEngineResult;
import org.apache.wss4j.dom.handler.RequestData;
import org.apache.wss4j.dom.processor.Processor;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.w3c.dom.Element;

@DisplayName("ProfileSignatureProcessor")
class ProfileSignatureProcessorTest {

  private static final String RSA_SHA1 = "http://www.w3.org/2000/09/xmldsig#rsa-sha1";

  private static Path keystorePath;

  @BeforeAll
  static void setUp() throws Exception {
    keystorePath = TestKeystoreHelper.createTestKeystore();
  }

  @AfterAll
  static void tearDown() throws Exception {
    if (keystorePath != null) {
      Files.deleteIfExists(keystorePath);
    }
  }

  @Test
  @DisplayName("suite carries exactly the effective signature and digest entries")
  void suite_containsExactlyTheEffectiveEntries() {
    SecurityProfile profile =
        SecurityProfile.builder("suite-entries")
            .keystore(keystorePath.toString(), "changeit")
            .allowedSignatureAlgorithms(RSA_SHA1)
            .build();

    AlgorithmSuite suite = profile.inboundAlgorithmSuite();

    assertEquals(Set.of(RSA_SHA1), suite.getSignatureMethods());
    assertEquals(
        Set.copyOf(profile.effectiveAllowedDigestAlgorithms()), suite.getDigestAlgorithms());
    assertEquals(Set.of(), suite.getC14nAlgorithms());
    assertEquals(Set.of(), suite.getEncryptionMethods());
    assertEquals(Set.of(), suite.getKeyWrapAlgorithms());
  }

  @Test
  @DisplayName("suite key-length bounds never reject")
  void suite_boundsAreNonRejecting() {
    AlgorithmSuite suite =
        SecurityProfile.builder("suite-bounds")
            .keystore(keystorePath.toString(), "changeit")
            .build()
            .inboundAlgorithmSuite();

    assertEquals(Integer.MAX_VALUE, suite.getMaximumSymmetricKeyLength());
    assertEquals(0, suite.getMinimumSymmetricKeyLength());
    assertEquals(Integer.MAX_VALUE, suite.getMaximumAsymmetricKeyLength());
    assertEquals(0, suite.getMinimumAsymmetricKeyLength());
    assertEquals(Integer.MAX_VALUE, suite.getMaximumEllipticCurveKeyLength());
    assertEquals(0, suite.getMinimumEllipticCurveKeyLength());
  }

  @Test
  @DisplayName("wrapper installs its suite into a RequestData with no suite")
  void wrapper_installsSuiteIntoCleanRequestData() throws Exception {
    AlgorithmSuite suite = new AlgorithmSuite();
    suite.addSignatureMethod(RSA_SHA1);
    RecordingProcessor delegate = new RecordingProcessor();
    ProfileSignatureProcessor wrapper = new ProfileSignatureProcessor(suite, delegate);
    RequestData requestData = new RequestData();

    wrapper.handleToken(null, requestData);

    assertNotNull(delegate.captured, "delegate was not invoked");
    assertSame(suite, delegate.captured.getAlgorithmSuite());
  }

  @Test
  @DisplayName("wrapper preserves an already-installed suite")
  void wrapper_preservesExistingSuite() throws Exception {
    AlgorithmSuite wrapperSuite = new AlgorithmSuite();
    wrapperSuite.addSignatureMethod(RSA_SHA1);
    AlgorithmSuite originalSuite = new AlgorithmSuite();
    originalSuite.addDigestAlgorithm("http://www.w3.org/2000/09/xmldsig#sha1");
    RecordingProcessor delegate = new RecordingProcessor();
    ProfileSignatureProcessor wrapper = new ProfileSignatureProcessor(wrapperSuite, delegate);
    RequestData requestData = new RequestData();
    requestData.setAlgorithmSuite(originalSuite);

    wrapper.handleToken(null, requestData);

    assertNotNull(delegate.captured, "delegate was not invoked");
    assertSame(originalSuite, delegate.captured.getAlgorithmSuite());
  }

  /** Processor stub that captures the RequestData its delegate receives. */
  private static final class RecordingProcessor implements Processor {
    private RequestData captured;

    @Override
    public List<WSSecurityEngineResult> handleToken(Element element, RequestData requestData)
        throws WSSecurityException {
      this.captured = requestData;
      return List.of();
    }
  }
}
