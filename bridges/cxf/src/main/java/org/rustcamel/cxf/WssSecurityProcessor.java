package org.rustcamel.cxf;

import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.security.NoSuchAlgorithmException;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.xml.parsers.DocumentBuilder;
import javax.xml.parsers.DocumentBuilderFactory;
import javax.xml.transform.dom.DOMSource;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.apache.wss4j.dom.WSConstants;
import org.apache.wss4j.dom.engine.WSSecurityEngine;
import org.apache.wss4j.dom.engine.WSSecurityEngineResult;
import org.apache.wss4j.dom.handler.RequestData;
import org.apache.wss4j.dom.handler.WSHandlerResult;
import org.apache.wss4j.dom.message.WSSecEncrypt;
import org.apache.wss4j.dom.message.WSSecHeader;
import org.apache.wss4j.dom.message.WSSecSignature;
import org.w3c.dom.Document;

/**
 * Applies WS-Security processing to SOAP envelopes using WSS4J DOM engine directly. Plain class
 * (not CDI). Takes a {@link SecurityProfile} per instance. Safe for GraalVM native-image.
 */
public class WssSecurityProcessor {

  private final SecurityProfile profile;

  public WssSecurityProcessor(SecurityProfile profile) {
    this.profile = profile;
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

    if (containsAction(actions, "Signature")) {
      WSSecSignature sign = new WSSecSignature(secHeader);
      String keyPass =
          hasText(profile.sigPassword()) ? profile.sigPassword() : profile.keystorePassword();
      sign.setUserInfo(profile.sigUsername(), keyPass);
      sign.setKeyIdentifierType(WSConstants.BST_DIRECT_REFERENCE);
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

  /** Apply inbound WS-Security (verify signature, decrypt, etc.) to a SOAP XML string. */
  public String processInbound(String soapXml) throws Exception {
    if (!canVerifyInbound() || soapXml == null || soapXml.isBlank()) {
      return soapXml;
    }

    Document doc = parseXml(soapXml);

    WSSecurityEngine engine = new WSSecurityEngine();
    RequestData requestData = new RequestData();
    requestData.setSigVerCrypto(profile.getVerificationCrypto());
    requestData.setDecCrypto(profile.getSignatureCrypto());
    requestData.setCallbackHandler(profile.createPasswordCallback());

    WSHandlerResult handlerResult = engine.processSecurityHeader(doc, requestData);
    List<WSSecurityEngineResult> results =
        handlerResult != null ? handlerResult.getResults() : null;

    // Enforce required actions
    String requiredActions = profile.resolveActionsIn();
    enforceRequiredActions(results, requiredActions);

    return domToString(doc);
  }

  // --- Helpers ---

  private void enforceRequiredActions(List<WSSecurityEngineResult> results, String requiredActions)
      throws WSSecurityException {
    if (results == null || results.isEmpty()) {
      throw new WSSecurityException(
          WSSecurityException.ErrorCode.SECURITY_ERROR,
          "noSecurity",
          new Object[] {"No WS-Security header found in message"});
    }

    Set<Integer> foundActions = new HashSet<>();
    for (WSSecurityEngineResult result : results) {
      Integer action = (Integer) result.get(WSSecurityEngineResult.TAG_ACTION);
      if (action != null) foundActions.add(action);
    }

    if (containsAction(requiredActions, "Signature")) {
      boolean signatureFound = foundActions.contains(WSConstants.SIGN);
      if (!signatureFound) {
        throw new WSSecurityException(
            WSSecurityException.ErrorCode.FAILED_CHECK,
            "noSecurity",
            new Object[] {"Required Signature action not found in message"});
      }
    }
    if (containsAction(requiredActions, "Encrypt")) {
      boolean encryptFound = foundActions.contains(WSConstants.ENCR);
      if (!encryptFound) {
        throw new WSSecurityException(
            WSSecurityException.ErrorCode.FAILED_CHECK,
            "noSecurity",
            new Object[] {"Required Encrypt action not found in message"});
      }
    }
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
}
