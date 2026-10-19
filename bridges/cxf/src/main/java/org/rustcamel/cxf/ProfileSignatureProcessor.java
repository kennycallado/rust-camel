package org.rustcamel.cxf;

import java.util.List;
import org.apache.jcp.xml.dsig.internal.dom.XMLDSigRI;
import org.apache.wss4j.common.crypto.AlgorithmSuite;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.apache.wss4j.dom.engine.WSSecurityEngineResult;
import org.apache.wss4j.dom.handler.RequestData;
import org.apache.wss4j.dom.processor.Processor;
import org.apache.wss4j.dom.processor.SignatureProcessor;
import org.w3c.dom.Element;

/**
 * Inbound signature processor that pins Santuario's {@link XMLDSigRI} and scopes acceptance to a
 * profile's {@link AlgorithmSuite}.
 *
 * <p>The pin exists because GraalVM native images cannot register security providers at runtime, so
 * WSS4J's own provider registration path is unavailable. Without it the JDK's default XML signature
 * provider falls back to JDK policy, which forbids SHA-1 digests once secure validation is enabled
 * (as it unconditionally is for inbound processing). Constructing the delegate with an explicit
 * {@code XMLDSigRI} instance keeps the Santuario implementation in play even when the provider is
 * absent from the runtime {@code java.security} registry.
 *
 * <p>MUST-VERIFY probe result: the pin alone is sufficient. With secure validation enabled and the
 * provider absent, an {@code rsa-sha1}+{@code sha1} signature verifies once the delegate is pinned
 * to {@code new XMLDSigRI()}, and {@link RequestData#setAlgorithmSuite(AlgorithmSuite)} confines
 * acceptance to the configured algorithms. The probe artefact lived in a session-local scratch
 * directory and is not committed; the argument above is the durable record (see ADR-0085).
 *
 * <p>The suite's key-length bounds are non-rejecting by design. The EncryptedKey, ReferenceList,
 * EncryptedData, and Signature processors all read the same suite, so a suite narrowed to the
 * signature's needs would reject the other processors depending on header order. Leaving every
 * bound open keeps acceptance algorithm-scoped and order-independent. Instances are immutable, so
 * concurrent {@link #handleToken(Element, RequestData)} calls are safe.
 */
public final class ProfileSignatureProcessor implements Processor {

  private final AlgorithmSuite suite;
  private final Processor delegate;

  /** Production constructor: pins the Santuario provider and scopes acceptance to {@code suite}. */
  public ProfileSignatureProcessor(AlgorithmSuite suite) {
    this(suite, new SignatureProcessor(new XMLDSigRI()));
  }

  /** Package-private test seam: lets tests observe the delegate call and its RequestData. */
  ProfileSignatureProcessor(AlgorithmSuite suite, Processor delegate) {
    this.suite = suite;
    this.delegate = delegate;
  }

  @Override
  public List<WSSecurityEngineResult> handleToken(Element headerElement, RequestData requestData)
      throws WSSecurityException {
    if (requestData.getAlgorithmSuite() == null) {
      requestData.setAlgorithmSuite(suite);
    }
    return delegate.handleToken(headerElement, requestData);
  }
}
