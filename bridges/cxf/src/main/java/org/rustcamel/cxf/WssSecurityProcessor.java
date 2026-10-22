package org.rustcamel.cxf;

import cxf_bridge.SignerIdentity;
import java.io.ByteArrayInputStream;
import java.math.BigInteger;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.security.cert.CertificateEncodingException;
import java.security.cert.X509Certificate;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.function.Supplier;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.security.auth.x500.X500Principal;
import javax.xml.parsers.DocumentBuilder;
import javax.xml.parsers.DocumentBuilderFactory;
import javax.xml.transform.dom.DOMSource;
import org.apache.wss4j.common.WSEncryptionPart;
import org.apache.wss4j.common.cache.MemoryReplayCache;
import org.apache.wss4j.common.cache.ReplayCache;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.apache.wss4j.dom.WSConstants;
import org.apache.wss4j.dom.WSDataRef;
import org.apache.wss4j.dom.engine.WSSConfig;
import org.apache.wss4j.dom.engine.WSSecurityEngine;
import org.apache.wss4j.dom.engine.WSSecurityEngineResult;
import org.apache.wss4j.dom.handler.RequestData;
import org.apache.wss4j.dom.handler.WSHandlerResult;
import org.apache.wss4j.dom.message.WSSecEncrypt;
import org.apache.wss4j.dom.message.WSSecHeader;
import org.apache.wss4j.dom.message.WSSecSignature;
import org.apache.wss4j.dom.message.WSSecTimestamp;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.NodeList;

/**
 * Applies WS-Security processing to SOAP envelopes using WSS4J DOM engine directly. Plain class
 * (not CDI). Takes a {@link SecurityProfile} per instance. Safe for GraalVM native-image.
 */
public class WssSecurityProcessor {

  /** Inbound {@code SIGN} results inspected for Body coverage; a 9th rejects the request. */
  private static final int MAX_SIGN_RESULTS = 8;

  /** Cap on the DER encoding being hashed; larger certificates are rejected, never truncated. */
  private static final int MAX_CERT_DER_BYTES = 64 * 1024;

  /**
   * Result of inbound WS-Security processing: the verified XML and the optional signer descriptor.
   * The signer is present only when a validated, trusted {@code SIGN} result covers the SOAP Body.
   */
  public record VerifiedInboundMessage(String xml, Optional<SignerIdentity> signer) {}

  private final SecurityProfile profile;

  /** Inbound replay protection for timestamp and nonce caches, shared across inbound calls. */
  private final ReplayCache replayCache = new MemoryReplayCache();

  /**
   * Produces a fresh engine per inbound call; supplied by the test seam, {@code pinnedEngine} in
   * production.
   */
  private final Supplier<WSSecurityEngine> engineFactory;

  public WssSecurityProcessor(SecurityProfile profile) {
    this(profile, () -> pinnedEngine(profile));
  }

  /**
   * Builds an engine whose signature processor is pinned to Santuario's provider and scoped to the
   * profile's inbound algorithm suite. WSS4J enables secure validation unconditionally; without the
   * pin the JDK's default XML signature provider applies JDK policy and forbids SHA-1. A GraalVM
   * native image cannot register Santuario's provider at runtime, so the explicit {@link
   * ProfileSignatureProcessor} keeps the Santuario implementation in play when the provider is
   * absent from {@code java.security.Security}.
   */
  private static WSSecurityEngine pinnedEngine(SecurityProfile profile) {
    WSSecurityEngine engine = new WSSecurityEngine();
    WSSConfig config = WSSConfig.getNewInstance();
    config.setProcessor(
        WSConstants.SIGNATURE, new ProfileSignatureProcessor(profile.inboundAlgorithmSuite()));
    engine.setWssConfig(config);
    return engine;
  }

  /** Package-private constructor: engine factory seam for result-fixture tests. */
  WssSecurityProcessor(SecurityProfile profile, Supplier<WSSecurityEngine> engineFactory) {
    // Defense-in-depth: the validated Rust path already refuses such profiles at endpoint
    // construction. Consumer coverage is the fixed replay-defense invariant and cannot be
    // narrowed via SIGNATURE_PARTS.
    if (profile.signatureParts() != null && !profile.signatureParts().isBlank()) {
      throw new IllegalStateException(
          "SIGNATURE_PARTS is not supported on the consumer path: consumer coverage is fixed"
              + " to Body+Timestamp for replay defense");
    }
    this.profile = profile;
    this.engineFactory = engineFactory;
  }

  /** Returns true if outbound signing is possible (keystore configured). */
  public boolean canSignOutbound() {
    return profile.canSignOutbound();
  }

  /** Returns true if inbound verification is possible (truststore or keystore configured). */
  public boolean canVerifyInbound() {
    return profile.canVerifyInbound();
  }

  // --- Outbound processing (action-driven) ---

  /** Apply outbound WS-Security (sign, encrypt, etc.) to a SOAP XML string. */
  public String processOutbound(String soapXml) throws Exception {
    if (!canSignOutbound() || soapXml == null || soapXml.isBlank()) {
      return soapXml;
    }

    String actions = profile.resolveActionsOut();
    Document doc = parseXml(soapXml);
    WSSecHeader secHeader = new WSSecHeader(doc);
    secHeader.insertSecurityHeader();

    if (containsAction(actions, "Timestamp")) {
      // Emitted before the signature so inbound replay detection keys off the timestamp
      new WSSecTimestamp(secHeader).build();
    }

    if (containsAction(actions, "Signature")) {
      WSSecSignature sign = new WSSecSignature(secHeader);
      String keyPass =
          hasText(profile.sigPassword()) ? profile.sigPassword() : profile.keystorePassword();
      sign.setUserInfo(profile.sigUsername(), keyPass);
      sign.setKeyIdentifierType(WSConstants.BST_DIRECT_REFERENCE);
      // Cover Body + Timestamp explicitly so a rewritten Timestamp cannot be processed as
      // fresh. WSS4J 4.0 has no setParts: build() appends the default Body part only when the
      // live getParts() list is empty, so both parts must be added here.
      if (containsAction(actions, "Timestamp")) {
        List<WSEncryptionPart> parts = sign.getParts();
        // The envelope root NS is the SOAP NS — never hardcode SOAP 1.1 here, or SOAP 1.2
        // envelopes fail part resolution at sign time.
        parts.add(
            new WSEncryptionPart("Body", doc.getDocumentElement().getNamespaceURI(), "Content"));
        parts.add(new WSEncryptionPart("Timestamp", WSConstants.WSU_NS, ""));
      }
      // Signature algorithm knobs apply on this consumer signed-response path exactly as on
      // the producer out-interceptor. WSS4J 4.x WSSecSignature setter names: setDigestAlgo,
      // setSigCanonicalization (no setDigestAlgorithm/setCanonicalizationAlgorithm).
      if (hasText(profile.signatureAlgorithm())) {
        sign.setSignatureAlgorithm(profile.signatureAlgorithm());
      }
      if (hasText(profile.signatureDigestAlgorithm())) {
        sign.setDigestAlgo(profile.signatureDigestAlgorithm());
      }
      if (hasText(profile.signatureC14nAlgorithm())) {
        sign.setSigCanonicalization(profile.signatureC14nAlgorithm());
      }
      sign.build(profile.getSignatureCrypto());
    }

    if (containsAction(actions, "Encrypt")) {
      WSSecEncrypt encrypt = new WSSecEncrypt(secHeader);
      encrypt.setUserInfo(profile.encUsername());
      encrypt.setKeyIdentifierType(WSConstants.X509_KEY_IDENTIFIER);
      encrypt.setSymmetricEncAlgorithm(WSConstants.AES_128);
      encrypt.setKeyEncAlgo(WSConstants.KEYTRANSPORT_RSAOAEP);
      encrypt.build(profile.getVerificationCrypto(), generateSymmetricKey());
    }

    return domToString(doc);
  }

  // --- Inbound processing (enforce required actions) ---

  /** Apply inbound WS-Security; delegates with an unspecified SOAP version. */
  public VerifiedInboundMessage processInbound(String soapXml) throws Exception {
    return processInbound(soapXml, null);
  }

  /**
   * Apply inbound WS-Security (verify signature, decrypt, etc.) to a SOAP XML string. {@code
   * soapVersion} is the version the listener detected before this call; the overload exists so the
   * listener never re-detects it after. Returns the verified XML together with the optional signer
   * descriptor: a validated, trusted {@code SIGN} result that covers the SOAP Body. When inbound
   * verification is impossible, or the input is null or blank, the input XML is returned with no
   * signer.
   */
  public VerifiedInboundMessage processInbound(String soapXml, String soapVersion)
      throws Exception {
    if (!canVerifyInbound() || soapXml == null || soapXml.isBlank()) {
      return new VerifiedInboundMessage(soapXml, Optional.empty());
    }

    Document doc = parseXml(soapXml);

    WSSecurityEngine engine = engineFactory.get();
    RequestData requestData = new RequestData();
    requestData.setSigVerCrypto(profile.getVerificationCrypto());
    requestData.setDecCrypto(profile.getSignatureCrypto());
    requestData.setCallbackHandler(profile.createPasswordCallback());
    requestData.setTimestampReplayCache(replayCache);
    requestData.setNonceReplayCache(replayCache);
    // Per-profile freshness window; defaults equal the WSS4J defaults.
    requestData.setTimeStampTTL(profile.timestampTtlSeconds());
    requestData.setTimeStampFutureTTL(profile.timestampFutureToleranceSeconds());

    WSHandlerResult handlerResult = engine.processSecurityHeader(doc, requestData);
    List<WSSecurityEngineResult> results =
        handlerResult != null ? handlerResult.getResults() : null;

    // Enforce required actions
    String requiredActions = profile.resolveActionsIn();

    // Sign-budget preflight FIRST: SIGN results are counted by action tag alone, before any
    // coverage or metadata pass reads references or certificates. A 9th SIGN result rejects on
    // the bound itself. Gated to Signature-required profiles; others stay on the normal WSS
    // path with no signer policy at all.
    if (containsAction(requiredActions, "Signature")) {
      enforceSignResultBudget(results);
    }

    enforceRequiredActions(results, requiredActions);

    // WSS4J 4.0 has no required-parts hook on RequestData, so timestamp signature coverage
    // is enforced here against the verified references.
    if (containsAction(requiredActions, "Timestamp")
        && containsAction(requiredActions, "Signature")) {
      verifyTimestampSignatureCoverage(doc, results);
    }

    // Signer metadata extraction and every signer policy check run only when the profile's
    // inbound actions require a Signature. Without it the message stays on the normal WSS
    // path and no signer is exported, whatever the verified SIGN results contain.
    Optional<SignerIdentity> signer = Optional.empty();
    if (containsAction(requiredActions, "Signature")) {
      signer = extractBodySigner(doc, results);
    }

    return new VerifiedInboundMessage(domToString(doc), signer);
  }

  // --- Helpers ---

  private void enforceRequiredActions(List<WSSecurityEngineResult> results, String requiredActions)
      throws WSSecurityException {
    if (results == null || results.isEmpty()) {
      throw new WssRequirementException(
          WSSecurityException.ErrorCode.SECURITY_ERROR, "No WS-Security header found in message");
    }

    Set<Integer> foundActions = new HashSet<>();
    for (WSSecurityEngineResult result : results) {
      Integer action = (Integer) result.get(WSSecurityEngineResult.TAG_ACTION);
      if (action != null) foundActions.add(action);
    }

    if (containsAction(requiredActions, "Signature")) {
      boolean signatureFound = foundActions.contains(WSConstants.SIGN);
      if (!signatureFound) {
        throw new WssRequirementException(
            WSSecurityException.ErrorCode.FAILED_CHECK,
            "Required Signature action not found in message");
      }
    }
    if (containsAction(requiredActions, "Encrypt")) {
      boolean encryptFound = foundActions.contains(WSConstants.ENCR);
      if (!encryptFound) {
        throw new WssRequirementException(
            WSSecurityException.ErrorCode.FAILED_CHECK,
            "Required Encrypt action not found in message");
      }
    }
    if (containsAction(requiredActions, "Timestamp")) {
      boolean timestampFound = foundActions.contains(WSConstants.TS);
      if (!timestampFound) {
        throw new WssRequirementException(
            WSSecurityException.ErrorCode.FAILED_CHECK,
            "Required Timestamp action not found in message");
      }
    }
  }

  /**
   * Fails when the wsu:Timestamp element is absent or not covered by the verified signature. A
   * Timestamp rewritten after signing breaks its digest reference and never reaches this check, but
   * a stripped or unsigned Timestamp would otherwise validate.
   */
  private void verifyTimestampSignatureCoverage(Document doc, List<WSSecurityEngineResult> results)
      throws WSSecurityException {
    NodeList timestampNodes = doc.getElementsByTagNameNS(WSConstants.WSU_NS, "Timestamp");
    if (timestampNodes.getLength() == 0) {
      throw new WssRequirementException(
          WSSecurityException.ErrorCode.FAILED_CHECK,
          "Required wsu:Timestamp element not found in message");
    }

    Element timestamp = (Element) timestampNodes.item(0);
    String timestampId = timestamp.getAttributeNS(WSConstants.WSU_NS, "Id");

    for (WSSecurityEngineResult result : results) {
      Integer action = (Integer) result.get(WSSecurityEngineResult.TAG_ACTION);
      if (action == null || action != WSConstants.SIGN) continue;
      Object refsObj = result.get(WSSecurityEngineResult.TAG_DATA_REF_URIS);
      if (!(refsObj instanceof List<?>)) continue;
      for (Object obj : (List<?>) refsObj) {
        if (!(obj instanceof WSDataRef ref)) continue;
        String signedWsuId = ref.getWsuId();
        if (hasText(signedWsuId) && signedWsuId.equals(timestampId)) return;
        Element protectedElement = ref.getProtectedElement();
        if (protectedElement != null && protectedElement.isSameNode(timestamp)) return;
      }
    }
    throw new WssRequirementException(
        WSSecurityException.ErrorCode.FAILED_CHECK,
        "wsu:Timestamp is not covered by the message signature");
  }

  /** Rejects on a 9th {@code SIGN} result by action tag alone; never reads refs or certificates. */
  private static void enforceSignResultBudget(List<WSSecurityEngineResult> results)
      throws WSSecurityException {
    int signSeen = 0;
    if (results == null) return;
    for (WSSecurityEngineResult result : results) {
      Integer action = (Integer) result.get(WSSecurityEngineResult.TAG_ACTION);
      if (action != null && action == WSConstants.SIGN && ++signSeen > MAX_SIGN_RESULTS) {
        throw new WssRequirementException(
            WSSecurityException.ErrorCode.SECURITY_ERROR,
            "SIGN results exceed the inspection budget: at most 8 are permitted");
      }
    }
  }

  /**
   * Exports the signer descriptor from the validated {@code SIGN} results that cover the SOAP Body.
   * Only called when the profile's inbound actions require a Signature, after {@link
   * #enforceSignResultBudget} has bounded the {@code SIGN} count to at most 8. Certificates sharing
   * one fingerprint count as one signer; more than one distinct Body-covering fingerprint rejects
   * the request as ambiguous. A Body-covering result without a verified certificate rejects
   * fail-closed. No Body-covering result at all is a missing required coverage.
   */
  private Optional<SignerIdentity> extractBodySigner(
      Document doc, List<WSSecurityEngineResult> results) throws WSSecurityException {
    Element body = SoapEnvelopeHelper.directBodyElement(doc);
    Map<String, X509Certificate> byFingerprint = new LinkedHashMap<>();
    if (results != null) {
      for (WSSecurityEngineResult result : results) {
        Integer action = (Integer) result.get(WSSecurityEngineResult.TAG_ACTION);
        if (action == null || action != WSConstants.SIGN) continue;
        if (!coversBody(result, body)) continue;
        Object certObj = result.get(WSSecurityEngineResult.TAG_X509_CERTIFICATE);
        if (!(certObj instanceof X509Certificate cert)) {
          throw new WssRequirementException(
              WSSecurityException.ErrorCode.SECURITY_ERROR,
              "Body-covering signature carries no verified certificate");
        }
        byFingerprint.putIfAbsent(fingerprintOf(cert), cert);
      }
    }

    if (byFingerprint.isEmpty()) {
      throw new WssRequirementException(
          WSSecurityException.ErrorCode.FAILED_CHECK,
          "Required Signature does not cover the SOAP Body");
    }
    if (byFingerprint.size() > 1) {
      throw new WssRequirementException(
          WSSecurityException.ErrorCode.SECURITY_ERROR,
          "multiple distinct Body-covering signers are ambiguous");
    }
    return Optional.of(exportSignerIdentity(byFingerprint.values().iterator().next()));
  }

  /**
   * True when a {@code SIGN} result covers the authoritative SOAP Body: the envelope's direct
   * {@code Body} child, never a same-named header element. A data reference with a known protected
   * element counts only when that element is the Body itself, so a header element masquerading
   * through a shared wsu:Id never counts; a reference without a protected element falls back to a
   * direct wsu:Id match against the Body's own wsu:Id.
   */
  private static boolean coversBody(WSSecurityEngineResult result, Element body) {
    if (body == null) return false;
    Object refsObj = result.get(WSSecurityEngineResult.TAG_DATA_REF_URIS);
    if (!(refsObj instanceof List<?> refs)) return false;
    String bodyWsuId = body.getAttributeNS(WSConstants.WSU_NS, "Id");
    for (Object obj : refs) {
      if (!(obj instanceof WSDataRef ref)) continue;
      Element protectedElement = ref.getProtectedElement();
      if (protectedElement != null) {
        if (protectedElement.isSameNode(body)) return true;
        continue;
      }
      String signedWsuId = ref.getWsuId();
      if (hasText(signedWsuId) && hasText(bodyWsuId) && signedWsuId.equals(bodyWsuId)) return true;
    }
    return false;
  }

  /** Builds the bounded wire descriptor from one verified signer certificate. */
  private static SignerIdentity exportSignerIdentity(X509Certificate cert)
      throws WSSecurityException {
    byte[] der = encodedDer(cert);
    BigInteger serialNumber = cert.getSerialNumber();
    if (serialNumber.signum() < 0) {
      throw new WssRequirementException(
          WSSecurityException.ErrorCode.SECURITY_ERROR,
          "signer certificate serial number is negative");
    }
    return SignerIdentity.newBuilder()
        .setSubject(cert.getSubjectX500Principal().getName(X500Principal.RFC2253))
        .setIssuer(cert.getIssuerX500Principal().getName(X500Principal.RFC2253))
        .setSerial(serialNumber.toString(16).toLowerCase(Locale.ROOT))
        .setFingerprintSha256(fingerprintOf(cert))
        .build();
  }

  /** The certificate DER encoding, rejected when it exceeds 64 KiB — never truncated. */
  private static byte[] encodedDer(X509Certificate cert) throws WSSecurityException {
    byte[] der;
    try {
      der = cert.getEncoded();
    } catch (CertificateEncodingException e) {
      throw new WssRequirementException(
          WSSecurityException.ErrorCode.SECURITY_ERROR,
          "signer certificate cannot be DER encoded: " + e.getMessage());
    }
    if (der == null || der.length > MAX_CERT_DER_BYTES) {
      throw new WssRequirementException(
          WSSecurityException.ErrorCode.SECURITY_ERROR,
          "signer certificate DER encoding exceeds 64 KiB");
    }
    return der;
  }

  /** SHA-256 of the DER certificate as 64 lowercase hex characters. */
  private static String fingerprintOf(X509Certificate cert) throws WSSecurityException {
    byte[] digest;
    try {
      digest = MessageDigest.getInstance("SHA-256").digest(encodedDer(cert));
    } catch (NoSuchAlgorithmException e) {
      throw new WssRequirementException(
          WSSecurityException.ErrorCode.SECURITY_ERROR,
          "SHA-256 is unavailable: " + e.getMessage());
    }
    StringBuilder hex = new StringBuilder(digest.length * 2);
    for (byte b : digest) {
      hex.append(Character.forDigit((b >> 4) & 0xf, 16)).append(Character.forDigit(b & 0xf, 16));
    }
    return hex.toString();
  }

  private static boolean containsAction(String actions, String token) {
    if (actions == null || actions.isBlank()) return false;
    for (String part : actions.split("\\s+")) {
      if (part.equalsIgnoreCase(token)) return true;
    }
    return false;
  }

  private static SecretKey generateSymmetricKey() throws NoSuchAlgorithmException {
    KeyGenerator keyGen = KeyGenerator.getInstance("AES");
    keyGen.init(128);
    return keyGen.generateKey();
  }

  private Document parseXml(String xml) throws Exception {
    DocumentBuilderFactory dbf = DocumentBuilderFactory.newInstance();
    dbf.setNamespaceAware(true);
    dbf.setFeature("http://xml.org/sax/features/external-general-entities", false);
    dbf.setFeature("http://xml.org/sax/features/external-parameter-entities", false);
    dbf.setFeature("http://apache.org/xml/features/nonvalidating/load-external-dtd", false);
    dbf.setFeature("http://apache.org/xml/features/disallow-doctype-decl", true);
    dbf.setXIncludeAware(false);

    DocumentBuilder db = dbf.newDocumentBuilder();
    return db.parse(new ByteArrayInputStream(xml.getBytes(StandardCharsets.UTF_8)));
  }

  private String domToString(Document doc) throws Exception {
    return SoapEnvelopeHelper.sourceToString(new DOMSource(doc));
  }

  private static boolean hasText(String value) {
    return value != null && !value.isBlank();
  }

  /**
   * WSSecurityException whose message survives verbatim. Every String-bearing constructor of wss4j
   * 4.0.1's WSSecurityException resolves its message through Santuario's I18n bundle
   * (org/apache/xml/security/resource/xmlsecurity), and the msgId used at the requirement throw
   * sites is absent from that bundle, so the lookup fell back to a generic "No message with ID"
   * text and the MessageFormat pass (no placeholder) dropped the Object[] detail args — every
   * coverage/required-action failure rendered byte-identical diagnostics (bd rc-sggo9). A
   * resource-bundle override is not viable here: WSSecurityException never consults wss4j's own
   * messages bundle for these constructors, and swapping the shared Santuario bundle would hijack
   * i18n for every other msgId. This subclass keeps the WSSecurityException type and ErrorCode
   * (SOAP fault-code mapping unchanged) while returning the exact per-failure detail from
   * getMessage(), leaving bundle-backed msgIds untouched.
   */
  private static final class WssRequirementException extends WSSecurityException {

    private static final long serialVersionUID = 1L;

    private final String detail;

    WssRequirementException(WSSecurityException.ErrorCode errorCode, String detail) {
      super(errorCode);
      this.detail = detail;
    }

    @Override
    public String getMessage() {
      return detail;
    }
  }
}
