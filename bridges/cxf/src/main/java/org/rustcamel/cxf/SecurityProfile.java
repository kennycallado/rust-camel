package org.rustcamel.cxf;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Properties;
import java.util.logging.Logger;
import javax.security.auth.callback.Callback;
import javax.security.auth.callback.CallbackHandler;
import org.apache.cxf.interceptor.Interceptor;
import org.apache.cxf.message.Message;
import org.apache.cxf.ws.security.wss4j.WSS4JInInterceptor;
import org.apache.cxf.ws.security.wss4j.WSS4JOutInterceptor;
import org.apache.wss4j.common.ConfigurationConstants;
import org.apache.wss4j.common.crypto.Crypto;
import org.apache.wss4j.common.crypto.CryptoFactory;
import org.apache.wss4j.common.ext.WSPasswordCallback;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.apache.wss4j.dom.engine.WSSConfig;
import org.apache.wss4j.dom.handler.WSHandlerConstants;

/**
 * Per-profile security configuration. Plain Java object — NOT a CDI bean. Instantiated by {@link
 * SecurityProfileStore} at startup. Caches Crypto instances lazily (volatile + synchronized
 * double-checked locking).
 */
public class SecurityProfile {
  private static final Logger LOG = Logger.getLogger(SecurityProfile.class.getName());

  static {
    WSSConfig.init();
  }

  // --- Identity ---
  private final String name;

  // --- WSDL / endpoint defaults ---
  private final String wsdlPath;
  private final String serviceName;
  private final String portName;
  private final String address;

  // --- Keystore / truststore ---
  private final String keystorePath;
  private final String keystorePassword;
  private final String truststorePath;
  private final String truststorePassword;

  // --- Signature / encryption users ---
  private final String sigUsername;
  private final String sigPassword;
  private final String encUsername;

  // --- Actions ---
  private final String securityActionsOut;
  private final String securityActionsIn;

  // --- Signature algorithms ---
  private final String signatureAlgorithm;
  private final String signatureDigestAlgorithm;
  private final String signatureC14nAlgorithm;
  private final String signatureParts;

  // --- Cached Crypto (lazy, thread-safe) ---
  private volatile Crypto signatureCrypto;
  private volatile Crypto verificationCrypto;
  private final Object cryptoLock = new Object();

  private SecurityProfile(Builder b) {
    this.name = b.name;
    this.wsdlPath = b.wsdlPath;
    this.serviceName = b.serviceName;
    this.portName = b.portName;
    this.address = b.address;
    this.keystorePath = b.keystorePath;
    this.keystorePassword = b.keystorePassword;
    this.truststorePath = b.truststorePath;
    this.truststorePassword = b.truststorePassword;
    this.sigUsername = hasText(b.sigUsername) ? b.sigUsername : "clientkey";
    this.sigPassword = b.sigPassword;
    this.encUsername = hasText(b.encUsername) ? b.encUsername : "serverkey";
    this.securityActionsOut = b.securityActionsOut;
    this.securityActionsIn = b.securityActionsIn;
    this.signatureAlgorithm = b.signatureAlgorithm;
    this.signatureDigestAlgorithm = b.signatureDigestAlgorithm;
    this.signatureC14nAlgorithm = b.signatureC14nAlgorithm;
    this.signatureParts = b.signatureParts;
  }

  // --- Accessors ---

  public String name() {
    return name;
  }

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
    return address;
  }

  public String keystorePath() {
    return keystorePath;
  }

  public String keystorePassword() {
    return keystorePassword;
  }

  public String truststorePath() {
    return truststorePath;
  }

  public String truststorePassword() {
    return truststorePassword;
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

  public String securityActionsOut() {
    return securityActionsOut;
  }

  public String securityActionsIn() {
    return securityActionsIn;
  }

  public String signatureAlgorithm() {
    return signatureAlgorithm;
  }

  public String signatureDigestAlgorithm() {
    return signatureDigestAlgorithm;
  }

  public String signatureC14nAlgorithm() {
    return signatureC14nAlgorithm;
  }

  public String signatureParts() {
    return signatureParts;
  }

  // --- Predicates ---

  public boolean hasSecurity() {
    return hasText(keystorePath) || hasText(truststorePath);
  }

  public boolean canSignOutbound() {
    return hasText(keystorePath);
  }

  public boolean canVerifyInbound() {
    return hasText(truststorePath) || hasText(keystorePath);
  }

  // --- Action resolution ---

  public String resolveActionsOut() {
    return hasText(securityActionsOut) ? securityActionsOut : "Signature";
  }

  public String resolveActionsIn() {
    return hasText(securityActionsIn) ? securityActionsIn : "Signature";
  }

  // --- WSS4J Interceptor factories ---

  public Interceptor<? extends Message> createOutInterceptor() {
    if (!hasSecurity()) return null;

    Map<String, Object> props = new HashMap<>();
    List<String> actions = new ArrayList<>();

    if (hasText(keystorePath)) {
      String outActions = resolveActionsOut();
      if (containsAction(outActions, "Signature")) {
        actions.add(WSHandlerConstants.SIGNATURE);
        props.put(
            ConfigurationConstants.SIG_PROP_REF_ID,
            createCryptoProperties(keystorePath, keystorePassword));
        props.put(WSHandlerConstants.USER, sigUsername);
        props.put(WSHandlerConstants.SIG_KEY_ID, "DirectReference");
      }
      if (containsAction(outActions, "Encrypt")) {
        actions.add(WSHandlerConstants.ENCRYPT);
        String encPath = hasText(truststorePath) ? truststorePath : keystorePath;
        String encPass = hasText(truststorePassword) ? truststorePassword : keystorePassword;
        props.put(ConfigurationConstants.ENC_PROP_REF_ID, createCryptoProperties(encPath, encPass));
        props.put(ConfigurationConstants.ENCRYPTION_USER, encUsername);
        props.put(
            WSHandlerConstants.ENC_KEY_TRANSPORT,
            "http://www.w3.org/2001/04/xmlenc#rsa-oaep-mgf1p");
        props.put(WSHandlerConstants.ENC_SYM_ALGO, "http://www.w3.org/2001/04/xmlenc#aes128-cbc");
      }
    }

    if (actions.isEmpty()) return null;

    props.put(WSHandlerConstants.ACTION, String.join(" ", actions));
    LOG.info("WS-Security outbound enabled for profile '" + name + "': actions=" + actions);
    return new WSS4JOutInterceptor(props);
  }

  public Interceptor<? extends Message> createInInterceptor() {
    if (!hasText(truststorePath) && !hasText(keystorePath)) return null;

    Map<String, Object> props = new HashMap<>();
    List<String> actions = new ArrayList<>();

    String inActions = resolveActionsIn();
    if (hasText(truststorePath) && containsAction(inActions, "Signature")) {
      actions.add(WSHandlerConstants.SIGNATURE);
      props.put(
          ConfigurationConstants.SIG_PROP_REF_ID,
          createCryptoProperties(truststorePath, truststorePassword));
    }
    if (containsAction(inActions, "Encrypt")) {
      actions.add(WSHandlerConstants.ENCRYPT);
      props.put(
          ConfigurationConstants.DEC_PROP_REF_ID,
          createCryptoProperties(keystorePath, keystorePassword));
      props.put(WSHandlerConstants.PW_CALLBACK_REF, callbackWithPassword(keystorePassword));
    }

    if (actions.isEmpty()) return null;

    props.put(WSHandlerConstants.ACTION, String.join(" ", actions));
    LOG.info("WS-Security inbound enabled for profile '" + name + "': actions=" + actions);
    return new WSS4JInInterceptor(props);
  }

  // --- Crypto lazy initialization (double-checked locking) ---

  public Crypto getSignatureCrypto() throws WSSecurityException {
    if (signatureCrypto == null) {
      synchronized (cryptoLock) {
        if (signatureCrypto == null) {
          signatureCrypto =
              CryptoFactory.getInstance(createCryptoProperties(keystorePath, keystorePassword));
        }
      }
    }
    return signatureCrypto;
  }

  public Crypto getVerificationCrypto() throws WSSecurityException {
    if (verificationCrypto == null) {
      synchronized (cryptoLock) {
        if (verificationCrypto == null) {
          String path = hasText(truststorePath) ? truststorePath : keystorePath;
          String pass = hasText(truststorePassword) ? truststorePassword : keystorePassword;
          verificationCrypto = CryptoFactory.getInstance(createCryptoProperties(path, pass));
        }
      }
    }
    return verificationCrypto;
  }

  // --- Password callback ---

  public CallbackHandler createPasswordCallback() {
    return new ProfilePasswordCallback(this);
  }

  // --- Static utility ---

  public static Properties createCryptoProperties(String path, String password) {
    Properties crypto = new Properties();
    if (!hasText(path)) return crypto;
    crypto.put("org.apache.wss4j.crypto.provider", "org.apache.wss4j.common.crypto.Merlin");
    crypto.put("org.apache.wss4j.crypto.merlin.keystore.type", "JKS");
    crypto.put("org.apache.wss4j.crypto.merlin.keystore.file", path);
    if (password != null) {
      crypto.put("org.apache.wss4j.crypto.merlin.keystore.password", password);
    }
    return crypto;
  }

  // --- Builder ---

  public static Builder builder(String name) {
    return new Builder(name);
  }

  public static class Builder {
    private final String name;
    private String wsdlPath;
    private String serviceName;
    private String portName;
    private String address;
    private String keystorePath;
    private String keystorePassword;
    private String truststorePath;
    private String truststorePassword;
    private String sigUsername;
    private String sigPassword;
    private String encUsername;
    private String securityActionsOut;
    private String securityActionsIn;
    private String signatureAlgorithm;
    private String signatureDigestAlgorithm;
    private String signatureC14nAlgorithm;
    private String signatureParts;

    private Builder(String name) {
      this.name = name;
    }

    public Builder wsdlPath(String v) {
      this.wsdlPath = v;
      return this;
    }

    public Builder serviceName(String v) {
      this.serviceName = v;
      return this;
    }

    public Builder portName(String v) {
      this.portName = v;
      return this;
    }

    public Builder address(String v) {
      this.address = v;
      return this;
    }

    public Builder keystore(String path, String password) {
      this.keystorePath = path;
      this.keystorePassword = password;
      return this;
    }

    public Builder truststore(String path, String password) {
      this.truststorePath = path;
      this.truststorePassword = password;
      return this;
    }

    public Builder sigUser(String username, String password) {
      this.sigUsername = username;
      this.sigPassword = password;
      return this;
    }

    public Builder encUser(String username) {
      this.encUsername = username;
      return this;
    }

    public Builder actionsOut(String v) {
      this.securityActionsOut = v;
      return this;
    }

    public Builder actionsIn(String v) {
      this.securityActionsIn = v;
      return this;
    }

    public Builder signatureAlgorithm(String v) {
      this.signatureAlgorithm = v;
      return this;
    }

    public Builder signatureDigestAlgorithm(String v) {
      this.signatureDigestAlgorithm = v;
      return this;
    }

    public Builder signatureC14nAlgorithm(String v) {
      this.signatureC14nAlgorithm = v;
      return this;
    }

    public Builder signatureParts(String v) {
      this.signatureParts = v;
      return this;
    }

    public SecurityProfile build() {
      return new SecurityProfile(this);
    }
  }

  // --- Internal helpers ---

  private static CallbackHandler callbackWithPassword(String password) {
    return (Callback[] callbacks) -> {
      for (Callback cb : callbacks) {
        if (cb instanceof WSPasswordCallback pc && password != null) {
          pc.setPassword(password);
        }
      }
    };
  }

  private static boolean hasText(String v) {
    return v != null && !v.isBlank();
  }

  private static boolean containsAction(String actions, String token) {
    if (actions == null || actions.isBlank()) return false;
    for (String part : actions.split("\\s+")) {
      if (part.equalsIgnoreCase(token)) return true;
    }
    return false;
  }

  /** Password callback handler for WSS4J inbound processing. */
  private static class ProfilePasswordCallback implements CallbackHandler {
    private final SecurityProfile profile;

    ProfilePasswordCallback(SecurityProfile profile) {
      this.profile = profile;
    }

    @Override
    public void handle(Callback[] callbacks) {
      for (Callback cb : callbacks) {
        if (cb instanceof WSPasswordCallback pc) {
          String pass =
              (pc.getUsage() == WSPasswordCallback.SIGNATURE)
                  ? (hasText(profile.sigPassword())
                      ? profile.sigPassword()
                      : profile.keystorePassword())
                  : profile.keystorePassword();
          if (pass != null) pc.setPassword(pass);
        }
      }
    }
  }
}
