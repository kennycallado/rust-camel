package org.rustcamel.cxf;

import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;
import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.security.NoSuchAlgorithmException;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.security.auth.callback.Callback;
import javax.security.auth.callback.CallbackHandler;
import javax.security.auth.callback.UnsupportedCallbackException;
import javax.xml.parsers.DocumentBuilder;
import javax.xml.parsers.DocumentBuilderFactory;
import javax.xml.transform.dom.DOMSource;
import org.apache.wss4j.common.crypto.Crypto;
import org.apache.wss4j.common.crypto.CryptoFactory;
import org.apache.wss4j.common.ext.WSPasswordCallback;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.apache.wss4j.dom.WSConstants;
import org.apache.wss4j.dom.engine.WSSConfig;
import org.apache.wss4j.dom.engine.WSSecurityEngine;
import org.apache.wss4j.dom.engine.WSSecurityEngineResult;
import org.apache.wss4j.dom.handler.RequestData;
import org.apache.wss4j.dom.handler.WSHandlerResult;
import org.apache.wss4j.dom.message.WSSecEncrypt;
import org.apache.wss4j.dom.message.WSSecHeader;
import org.apache.wss4j.dom.message.WSSecSignature;
import org.w3c.dom.Document;

/**
 * Applies WS-Security processing to SOAP envelopes using WSS4J DOM engine directly. Does NOT use
 * Jetty or CXF transport layer — safe for GraalVM native-image.
 */
@ApplicationScoped
public class WssSecurityProcessor {

  @Inject BridgeConfig config;

  @Inject SecurityConfig securityConfig;

  /** Default constructor for CDI injection. */
  public WssSecurityProcessor() {}

  /** Package-private constructor for testing. */
  WssSecurityProcessor(BridgeConfig config) {
    this.config = config;
  }

  /** Package-private constructor for testing with explicit SecurityConfig. */
  WssSecurityProcessor(BridgeConfig config, SecurityConfig securityConfig) {
    this.config = config;
    this.securityConfig = securityConfig;
  }

  // Cached Crypto instances (lazy, thread-safe)
  private volatile Crypto signatureCrypto;
  private volatile Crypto verificationCrypto;
  private volatile Crypto decryptionCrypto;
  private final Object cryptoLock = new Object();

  static {
    WSSConfig.init();
  }

  /** Returns true if outbound signing is possible (keystore configured). */
  public boolean canSignOutbound() {
    return hasText(config.keystorePath());
  }

  /** Returns true if inbound verification is possible (truststore or keystore configured). */
  public boolean canVerifyInbound() {
    return hasText(config.truststorePath()) || hasText(config.keystorePath());
  }

  /**
   * Returns true if WS-Security is configured (keystore or truststore present). Backward compat.
   */
  public boolean isEnabled() {
    return canSignOutbound() || canVerifyInbound();
  }

  // --- Crypto lazy initialization (double-checked locking) ---

  private Crypto getSignatureCrypto() throws WSSecurityException {
    if (signatureCrypto == null) {
      synchronized (cryptoLock) {
        if (signatureCrypto == null) {
          signatureCrypto =
              CryptoFactory.getInstance(
                  SecurityConfig.createCryptoProperties(
                      config.keystorePath(), config.keystorePassword()));
        }
      }
    }
    return signatureCrypto;
  }

  private Crypto getVerificationCrypto() throws WSSecurityException {
    if (verificationCrypto == null) {
      synchronized (cryptoLock) {
        if (verificationCrypto == null) {
          String path =
              hasText(config.truststorePath()) ? config.truststorePath() : config.keystorePath();
          String password =
              hasText(config.truststorePassword())
                  ? config.truststorePassword()
                  : config.keystorePassword();
          verificationCrypto =
              CryptoFactory.getInstance(SecurityConfig.createCryptoProperties(path, password));
        }
      }
    }
    return verificationCrypto;
  }

  private Crypto getDecryptionCrypto() throws WSSecurityException {
    if (decryptionCrypto == null) {
      synchronized (cryptoLock) {
        if (decryptionCrypto == null) {
          decryptionCrypto =
              CryptoFactory.getInstance(
                  SecurityConfig.createCryptoProperties(
                      config.keystorePath(), config.keystorePassword()));
        }
      }
    }
    return decryptionCrypto;
  }

  // --- Outbound processing (action-driven) ---

  /** Apply outbound WS-Security (sign, encrypt, etc.) to a SOAP XML string. */
  public String processOutbound(String soapXml) throws Exception {
    if (!canSignOutbound() || soapXml == null || soapXml.isBlank()) {
      return soapXml;
    }

    String actions = resolveActionsOut();
    Document doc = parseXml(soapXml);
    WSSecHeader secHeader = new WSSecHeader(doc);
    secHeader.insertSecurityHeader();

    if (containsAction(actions, "Signature")) {
      WSSecSignature sign = new WSSecSignature(secHeader);
      String keyPass =
          hasText(config.sigPassword()) ? config.sigPassword() : config.keystorePassword();
      sign.setUserInfo(config.sigUsername(), keyPass);
      sign.setKeyIdentifierType(WSConstants.BST_DIRECT_REFERENCE);
      sign.build(getSignatureCrypto());
    }

    if (containsAction(actions, "Encrypt")) {
      WSSecEncrypt encrypt = new WSSecEncrypt(secHeader);
      encrypt.setUserInfo(config.encUsername());
      encrypt.setKeyIdentifierType(WSConstants.X509_KEY_IDENTIFIER);
      encrypt.setSymmetricEncAlgorithm(WSConstants.AES_128);
      encrypt.setKeyEncAlgo(WSConstants.KEYTRANSPORT_RSAOAEP);
      encrypt.build(getVerificationCrypto(), generateSymmetricKey());
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
    requestData.setSigVerCrypto(getVerificationCrypto());
    requestData.setDecCrypto(getDecryptionCrypto());
    requestData.setCallbackHandler(new PasswordCallbackHandler(config));

    WSHandlerResult handlerResult = engine.processSecurityHeader(doc, requestData);
    List<WSSecurityEngineResult> results =
        handlerResult != null ? handlerResult.getResults() : null;

    // Enforce required actions
    String requiredActions = resolveActionsIn();
    enforceRequiredActions(results, requiredActions);

    return domToString(doc);
  }

  // --- Helpers ---

  private String resolveActionsOut() {
    return securityConfig.resolveActionsOut();
  }

  private String resolveActionsIn() {
    return securityConfig.resolveActionsIn();
  }

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
    keyGen.init(128); // Must match symmetric algorithm declared on WSSecEncrypt (AES-128-CBC)
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

  /** Password callback handler for WSS4J inbound verification. */
  private static class PasswordCallbackHandler implements CallbackHandler {

    private final BridgeConfig config;

    PasswordCallbackHandler(BridgeConfig config) {
      this.config = config;
    }

    @Override
    public void handle(Callback[] callbacks) throws IOException, UnsupportedCallbackException {
      for (Callback cb : callbacks) {
        if (cb instanceof WSPasswordCallback pc) {
          String pass;
          if (pc.getUsage() == WSPasswordCallback.SIGNATURE) {
            pass = hasText(config.sigPassword()) ? config.sigPassword() : config.keystorePassword();
          } else {
            pass = config.keystorePassword();
          }
          if (pass != null) pc.setPassword(pass);
        } else {
          throw new UnsupportedCallbackException(
              cb, "Unrecognized callback type: " + cb.getClass().getName());
        }
      }
    }
  }
}
