package org.rustcamel.cxf;

import java.net.URI;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Properties;
import java.util.Set;
import java.util.logging.Logger;
import javax.security.auth.callback.Callback;
import javax.security.auth.callback.CallbackHandler;
import javax.xml.namespace.QName;
import org.apache.cxf.binding.soap.SoapMessage;
import org.apache.cxf.interceptor.Interceptor;
import org.apache.cxf.message.Message;
import org.apache.cxf.ws.security.wss4j.WSS4JInInterceptor;
import org.apache.cxf.ws.security.wss4j.WSS4JOutInterceptor;
import org.apache.wss4j.common.ConfigurationConstants;
import org.apache.wss4j.common.crypto.AlgorithmSuite;
import org.apache.wss4j.common.crypto.Crypto;
import org.apache.wss4j.common.crypto.CryptoFactory;
import org.apache.wss4j.common.ext.WSPasswordCallback;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.apache.wss4j.dom.WSConstants;
import org.apache.wss4j.dom.engine.WSSConfig;
import org.apache.wss4j.dom.handler.RequestData;
import org.apache.wss4j.dom.handler.WSHandlerConstants;

/**
 * Per-profile security configuration. Plain Java object — NOT a CDI bean. Instantiated by {@link
 * SecurityProfileStore} at startup. Caches Crypto instances lazily (volatile + synchronized
 * double-checked locking).
 */
public class SecurityProfile {
  private static final Logger LOG = Logger.getLogger(SecurityProfile.class.getName());

  /**
   * Default SIGNATURE_PARTS when both Timestamp and Signature are active and the operator did not
   * configure parts. Grammar jar-verified against wss4j-ws-security-dom 4.0.1:
   * WSHandler.splitEncParts splits entries on ';' and resolves a bare keyword against the SOAP
   * envelope namespace in Content mode, so Body stays bare (SOAP 1.1/1.2 safe) while Timestamp must
   * carry the WSU namespace explicitly.
   */
  private static final String DEFAULT_SIGNATURE_PARTS =
      "Body;{}{" + WSConstants.WSU_NS + "}Timestamp";

  /** Secure default inbound signature algorithm allow-list (SHA-2 only). */
  static final List<String> DEFAULT_ALLOWED_SIGNATURE_ALGORITHMS =
      List.of(
          "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256",
          "http://www.w3.org/2001/04/xmldsig-more#rsa-sha384",
          "http://www.w3.org/2001/04/xmldsig-more#rsa-sha512");

  /** Secure default inbound digest algorithm allow-list (SHA-2 only). */
  static final List<String> DEFAULT_ALLOWED_DIGEST_ALGORITHMS =
      List.of(
          "http://www.w3.org/2001/04/xmlenc#sha256",
          "http://www.w3.org/2001/04/xmldsig-more#sha384",
          "http://www.w3.org/2001/04/xmlenc#sha512");

  /** SHA-1 algorithm URIs recognized by the inbound allow-list check. */
  static final List<String> SHA1_ALGORITHM_URIS =
      List.of(
          "http://www.w3.org/2000/09/xmldsig#rsa-sha1",
          "http://www.w3.org/2000/09/xmldsig#dsa-sha1",
          "http://www.w3.org/2000/09/xmldsig#hmac-sha1",
          "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha1",
          "http://www.w3.org/2000/09/xmldsig#sha1");

  /** Default inbound WS-Security Timestamp TTL in seconds (same as the WSS4J default). */
  public static final int DEFAULT_TIMESTAMP_TTL_SECONDS = 300;

  /**
   * Default inbound WS-Security Timestamp future tolerance in seconds (same as the WSS4J default).
   */
  public static final int DEFAULT_TIMESTAMP_FUTURE_TOLERANCE_SECONDS = 60;

  /**
   * Option keys under which the in-memory crypto {@link Properties} objects live. WSS4J resolves
   * crypto by ref-id string: SIG_PROP_REF_ID / ENC_PROP_REF_ID / DEC_PROP_REF_ID name a key whose
   * value WSS4J looks up as the Properties object itself (WSHandler#getString + loadCrypto).
   */
  private static final String SIG_CRYPTO_REF_ID = "sigCryptoProperties";

  private static final String ENC_CRYPTO_REF_ID = "encCryptoProperties";

  private static final String DEC_CRYPTO_REF_ID = "decCryptoProperties";

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

  // --- Producer transport TLS (nested [profiles.security.tls]) ---
  private final String tlsKeystorePath;
  private final String tlsKeystorePassword;
  private final String tlsKeystoreType;
  private final String tlsTruststorePath;
  private final String tlsTruststorePassword;
  private final String tlsTruststoreType;

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

  // --- Inbound algorithm allow-lists (raw csv; null means the secure defaults) ---
  private final String allowedSignatureAlgorithms;
  private final String allowedDigestAlgorithms;

  // --- Inbound trust relaxation (per-profile opt-in; default false) ---
  private final boolean allowExpiredPinnedCertificate;

  // --- Inbound Timestamp freshness window (null = WSS4J default) ---
  private final Integer timestampTtlSeconds;
  private final Integer timestampFutureToleranceSeconds;

  /**
   * Optional per-profile HTTP status override for security (WSS) faults only. {@code null} keeps
   * the default matrix (SOAP 1.1: 500; SOAP 1.2 Sender: 400, Receiver: 500). Validated by {@link
   * SecurityProfileStore} to the range 400-599.
   */
  private final Integer faultStatus;

  // --- Exposed application SOAP headers (validated allow-list) ---
  private final List<QName> exposedSoapHeaders;

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
    this.tlsKeystorePath = b.tlsKeystorePath;
    this.tlsKeystorePassword = b.tlsKeystorePassword;
    this.tlsKeystoreType = normalizeTlsType(b.tlsKeystoreType);
    this.tlsTruststorePath = b.tlsTruststorePath;
    this.tlsTruststorePassword = b.tlsTruststorePassword;
    this.tlsTruststoreType = normalizeTlsType(b.tlsTruststoreType);
    this.sigUsername = hasText(b.sigUsername) ? b.sigUsername : "clientkey";
    this.sigPassword = b.sigPassword;
    this.encUsername = hasText(b.encUsername) ? b.encUsername : "serverkey";
    this.securityActionsOut = b.securityActionsOut;
    this.securityActionsIn = b.securityActionsIn;
    this.signatureAlgorithm = b.signatureAlgorithm;
    this.signatureDigestAlgorithm = b.signatureDigestAlgorithm;
    this.signatureC14nAlgorithm = b.signatureC14nAlgorithm;
    this.signatureParts = b.signatureParts;
    this.allowedSignatureAlgorithms = b.allowedSignatureAlgorithms;
    this.allowedDigestAlgorithms = b.allowedDigestAlgorithms;
    this.allowExpiredPinnedCertificate = b.allowExpiredPinnedCertificate;
    this.timestampTtlSeconds = b.timestampTtlSeconds;
    this.timestampFutureToleranceSeconds = b.timestampFutureToleranceSeconds;
    this.faultStatus = b.faultStatus;
    this.exposedSoapHeaders = b.exposedSoapHeaders;
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

  public String tlsKeystorePath() {
    return tlsKeystorePath;
  }

  public String tlsKeystorePassword() {
    return tlsKeystorePassword;
  }

  public String tlsKeystoreType() {
    return tlsKeystoreType;
  }

  public String tlsTruststorePath() {
    return tlsTruststorePath;
  }

  public String tlsTruststorePassword() {
    return tlsTruststorePassword;
  }

  public String tlsTruststoreType() {
    return tlsTruststoreType;
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

  /** Raw comma-separated inbound signature allow-list, or {@code null} when unset. */
  public String allowedSignatureAlgorithmsRaw() {
    return allowedSignatureAlgorithms;
  }

  /** Raw comma-separated inbound digest allow-list, or {@code null} when unset. */
  public String allowedDigestAlgorithmsRaw() {
    return allowedDigestAlgorithms;
  }

  /**
   * Effective inbound signature algorithm allow-list: the configured csv parsed into trimmed,
   * ordered URIs, or {@link #DEFAULT_ALLOWED_SIGNATURE_ALGORITHMS} when unset.
   */
  public List<String> effectiveAllowedSignatureAlgorithms() {
    return allowedSignatureAlgorithms == null
        ? DEFAULT_ALLOWED_SIGNATURE_ALGORITHMS
        : parseAlgorithmList(allowedSignatureAlgorithms);
  }

  /**
   * Effective inbound digest algorithm allow-list: the configured csv parsed into trimmed, ordered
   * URIs, or {@link #DEFAULT_ALLOWED_DIGEST_ALGORITHMS} when unset.
   */
  public List<String> effectiveAllowedDigestAlgorithms() {
    return allowedDigestAlgorithms == null
        ? DEFAULT_ALLOWED_DIGEST_ALGORITHMS
        : parseAlgorithmList(allowedDigestAlgorithms);
  }

  /** True when this profile opts into accepting an expired exactly-pinned inbound certificate. */
  public boolean allowExpiredPinnedCertificate() {
    return allowExpiredPinnedCertificate;
  }

  /**
   * Effective inbound WS-Security Timestamp TTL in seconds, or {@link
   * #DEFAULT_TIMESTAMP_TTL_SECONDS} when unset. Consumer inbound path only; the producer
   * in-interceptor does not apply it.
   */
  public int timestampTtlSeconds() {
    return timestampTtlSeconds != null ? timestampTtlSeconds : DEFAULT_TIMESTAMP_TTL_SECONDS;
  }

  /**
   * Effective inbound WS-Security Timestamp future tolerance in seconds, or {@link
   * #DEFAULT_TIMESTAMP_FUTURE_TOLERANCE_SECONDS} when unset. Consumer inbound path only; the
   * producer in-interceptor does not apply it.
   */
  public int timestampFutureToleranceSeconds() {
    return timestampFutureToleranceSeconds != null
        ? timestampFutureToleranceSeconds
        : DEFAULT_TIMESTAMP_FUTURE_TOLERANCE_SECONDS;
  }

  /** True when either effective inbound allow-list carries a known SHA-1 algorithm URI. */
  public boolean inboundAllowsSha1() {
    return allowsSha1(effectiveAllowedSignatureAlgorithms())
        || allowsSha1(effectiveAllowedDigestAlgorithms());
  }

  /**
   * Builds the WSS4J {@link AlgorithmSuite} that scopes inbound acceptance to this profile's
   * effective allow-lists. Signature methods come from {@link
   * #effectiveAllowedSignatureAlgorithms()} and digest algorithms from {@link
   * #effectiveAllowedDigestAlgorithms()}. The six key-length bounds are set explicitly because
   * WSS4J's ctor defaults (128/256 symmetric, 1024/4096 asymmetric, 160/512 EC) would reject long
   * keys; the suite is also shared by the EncryptedKey, ReferenceList, EncryptedData, and Signature
   * processors, so bounds narrowed to one processor's needs would reject the others. The
   * derived-key length knobs are the ones that default to unchecked, so they are left unset.
   */
  public AlgorithmSuite inboundAlgorithmSuite() {
    AlgorithmSuite suite = new AlgorithmSuite();
    for (String algorithm : effectiveAllowedSignatureAlgorithms()) {
      suite.addSignatureMethod(algorithm);
    }
    for (String algorithm : effectiveAllowedDigestAlgorithms()) {
      suite.addDigestAlgorithm(algorithm);
    }
    suite.setMinimumSymmetricKeyLength(0);
    suite.setMaximumSymmetricKeyLength(Integer.MAX_VALUE);
    suite.setMinimumAsymmetricKeyLength(0);
    suite.setMaximumAsymmetricKeyLength(Integer.MAX_VALUE);
    suite.setMinimumEllipticCurveKeyLength(0);
    suite.setMaximumEllipticCurveKeyLength(Integer.MAX_VALUE);
    return suite;
  }

  /** Optional WSS-only HTTP status override (400-599), or {@code null} for the default matrix. */
  public Integer faultStatus() {
    return faultStatus;
  }

  /** Validated allow-list of application SOAP header QNames, in configuration order. */
  public List<QName> exposedSoapHeaders() {
    return exposedSoapHeaders;
  }

  // --- Predicates ---

  public boolean hasSecurity() {
    return hasText(keystorePath) || hasText(truststorePath);
  }

  /** True when producer transport TLS material is configured for this profile. */
  public boolean hasTls() {
    return hasText(tlsKeystorePath) || hasText(tlsTruststorePath);
  }

  public boolean canSignOutbound() {
    return hasText(keystorePath);
  }

  public boolean canVerifyInbound() {
    return hasText(truststorePath) || hasText(keystorePath);
  }

  // --- Action resolution ---

  /**
   * Single source for the default action list. Shared by runtime resolution and Builder validation
   * so the validated action set can never diverge from the applied one.
   */
  static String resolveActionsOrDefault(String raw) {
    return hasText(raw) ? raw : "Signature";
  }

  public String resolveActionsOut() {
    return resolveActionsOrDefault(securityActionsOut);
  }

  public String resolveActionsIn() {
    return resolveActionsOrDefault(securityActionsIn);
  }

  // --- WSS4J Interceptor factories ---

  public Interceptor<? extends Message> createOutInterceptor() {
    if (!hasSecurity()) return null;

    Map<String, Object> props = new HashMap<>();
    List<String> actions = new ArrayList<>();
    List<String> signatureKnobs = new ArrayList<>();
    String effectiveParts = null;

    if (hasText(keystorePath)) {
      String outActions = resolveActionsOut();
      boolean timestampActive = containsAction(outActions, "Timestamp");
      // Timestamp precedes Signature so signature part resolution sees the materialized
      // wsu:Timestamp element (same order as the manual consumer path).
      if (timestampActive) {
        actions.add(WSHandlerConstants.TIMESTAMP);
      }
      if (containsAction(outActions, "Signature")) {
        actions.add(WSHandlerConstants.SIGNATURE);
        props.put(ConfigurationConstants.SIG_PROP_REF_ID, SIG_CRYPTO_REF_ID);
        props.put(SIG_CRYPTO_REF_ID, createCryptoProperties(keystorePath, keystorePassword));
        props.put(WSHandlerConstants.USER, sigUsername);
        props.put(WSHandlerConstants.SIG_KEY_ID, "DirectReference");
        // WSS4J obtains the signing key password only via callback: ProfilePasswordCallback
        // prefers sigPassword for SIGNATURE usage and falls back to the keystore password.
        props.put(WSHandlerConstants.PW_CALLBACK_REF, createPasswordCallback());
        if (hasText(signatureAlgorithm)) {
          props.put(ConfigurationConstants.SIG_ALGO, signatureAlgorithm);
          signatureKnobs.add("signatureAlgorithm");
        }
        if (hasText(signatureDigestAlgorithm)) {
          props.put(ConfigurationConstants.SIG_DIGEST_ALGO, signatureDigestAlgorithm);
          signatureKnobs.add("signatureDigestAlgorithm");
        }
        if (hasText(signatureC14nAlgorithm)) {
          props.put(ConfigurationConstants.SIG_C14N_ALGO, signatureC14nAlgorithm);
          signatureKnobs.add("signatureC14nAlgorithm");
        }
        if (hasText(signatureParts)) {
          effectiveParts = signatureParts;
          props.put(ConfigurationConstants.SIGNATURE_PARTS, effectiveParts);
          signatureKnobs.add("signatureParts");
        } else if (timestampActive) {
          effectiveParts = DEFAULT_SIGNATURE_PARTS;
          props.put(ConfigurationConstants.SIGNATURE_PARTS, effectiveParts);
        }
      }
      if (containsAction(outActions, "Encrypt")) {
        actions.add(WSHandlerConstants.ENCRYPT);
        String encPath = hasText(truststorePath) ? truststorePath : keystorePath;
        String encPass = hasText(truststorePassword) ? truststorePassword : keystorePassword;
        props.put(ConfigurationConstants.ENC_PROP_REF_ID, ENC_CRYPTO_REF_ID);
        props.put(ENC_CRYPTO_REF_ID, createCryptoProperties(encPath, encPass));
        props.put(ConfigurationConstants.ENCRYPTION_USER, encUsername);
        props.put(
            WSHandlerConstants.ENC_KEY_TRANSPORT,
            "http://www.w3.org/2001/04/xmlenc#rsa-oaep-mgf1p");
        props.put(WSHandlerConstants.ENC_SYM_ALGO, "http://www.w3.org/2001/04/xmlenc#aes128-cbc");
      }
    }

    if (actions.isEmpty()) return null;

    props.put(WSHandlerConstants.ACTION, String.join(" ", actions));
    LOG.info(
        "WS-Security outbound enabled for profile '"
            + name
            + "': actions="
            + actions
            + ", signatureKnobs="
            + signatureKnobs
            + ", signatureParts="
            + (effectiveParts == null ? "none" : effectiveParts));
    return new WSS4JOutInterceptor(props);
  }

  public Interceptor<? extends Message> createInInterceptor() {
    if (!hasText(truststorePath) && !hasText(keystorePath)) return null;

    Map<String, Object> props = new HashMap<>();
    List<String> actions = new ArrayList<>();

    String inActions = resolveActionsIn();
    // Timestamp precedes Signature so signature part resolution sees the materialized
    // wsu:Timestamp element (same order as the outbound path). Build-time composition
    // guarantees Timestamp implies Signature, which implies a truststore.
    if (hasText(truststorePath) && containsAction(inActions, "Timestamp")) {
      actions.add(WSHandlerConstants.TIMESTAMP);
    }
    if (hasText(truststorePath) && containsAction(inActions, "Signature")) {
      actions.add(WSHandlerConstants.SIGNATURE);
      props.put(ConfigurationConstants.SIG_PROP_REF_ID, SIG_CRYPTO_REF_ID);
      props.put(SIG_CRYPTO_REF_ID, createCryptoProperties(truststorePath, truststorePassword));
    }
    if (containsAction(inActions, "Encrypt")) {
      actions.add(WSHandlerConstants.ENCRYPT);
      props.put(ConfigurationConstants.DEC_PROP_REF_ID, DEC_CRYPTO_REF_ID);
      props.put(DEC_CRYPTO_REF_ID, createCryptoProperties(keystorePath, keystorePassword));
      props.put(WSHandlerConstants.PW_CALLBACK_REF, callbackWithPassword(keystorePassword));
    }

    if (actions.isEmpty()) return null;

    // Same enforcement as the consumer path: pin Santuario's XMLDSigRI and scope acceptance to
    // this profile's inbound algorithm suite. WSS4JInInterceptor accepts Processor instances in
    // its PROCESSOR_MAP, so the wrapper survives CXF's WSSConfig setup unchanged.
    AlgorithmSuite inboundSuite = inboundAlgorithmSuite();
    props.put(
        WSS4JInInterceptor.PROCESSOR_MAP,
        Map.of(WSConstants.SIGNATURE, new ProfileSignatureProcessor(inboundSuite)));
    props.put(WSHandlerConstants.ACTION, String.join(" ", actions));
    LOG.info(
        "WS-Security inbound enabled for profile '"
            + name
            + "': actions="
            + actions
            + ", sigCryptoRefId="
            + (actions.contains(WSHandlerConstants.SIGNATURE) ? SIG_CRYPTO_REF_ID : "none")
            + ", decCryptoRefId="
            + (actions.contains(WSHandlerConstants.ENCRYPT) ? DEC_CRYPTO_REF_ID : "none"));
    return new ProfileInInterceptor(props, inboundSuite);
  }

  /**
   * Producer-side in-interceptor that installs the profile suite after CXF's own decoding. {@code
   * WSS4JInInterceptor} runs {@code WSHandler.decodeAlgorithmSuite} before processing and always
   * seeds a fresh, empty suite when the message context is non-null. The wrapper's null-only
   * install would therefore never fire, so the suite is set here instead; the PROCESSOR_MAP entry
   * still supplies the Santuario-pinned signature processor.
   */
  private static final class ProfileInInterceptor extends WSS4JInInterceptor {
    private final AlgorithmSuite inboundSuite;

    ProfileInInterceptor(Map<String, Object> properties, AlgorithmSuite inboundSuite) {
      super(properties);
      this.inboundSuite = inboundSuite;
    }

    @Override
    protected void setAlgorithmSuites(SoapMessage message, RequestData requestData)
        throws WSSecurityException {
      super.setAlgorithmSuites(message, requestData);
      requestData.setAlgorithmSuite(inboundSuite);
    }
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
          verificationCrypto =
              allowExpiredPinnedCertificate
                  ? new PinnedTrustMerlin(createCryptoProperties(path, pass), true)
                  : CryptoFactory.getInstance(createCryptoProperties(path, pass));
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
    /**
     * Action tokens the bridge actually materializes, stored LOWERCASE (entries are compared
     * against lowercased tokens, so membership is case-insensitive and case-consistent by
     * construction). Anything outside these sets — UsernameToken, SAML, SignatureConfirmation, ...
     * — is silently ignored by WSS4J sidecar processing today, so unknown tokens fail the profile
     * build instead (fail-loud, ADR-0033).
     */
    private static final Set<String> SUPPORTED_INBOUND_ACTIONS =
        Set.of("signature", "encrypt", "timestamp");

    private static final Set<String> SUPPORTED_OUTBOUND_ACTIONS =
        Set.of("signature", "encrypt", "timestamp");

    private final String name;
    private String wsdlPath;
    private String serviceName;
    private String portName;
    private String address;
    private String keystorePath;
    private String keystorePassword;
    private String truststorePath;
    private String truststorePassword;
    private String tlsKeystorePath;
    private String tlsKeystorePassword;
    private String tlsKeystoreType;
    private String tlsTruststorePath;
    private String tlsTruststorePassword;
    private String tlsTruststoreType;
    private String sigUsername;
    private String sigPassword;
    private String encUsername;
    private String securityActionsOut;
    private String securityActionsIn;
    private String signatureAlgorithm;
    private String signatureDigestAlgorithm;
    private String signatureC14nAlgorithm;
    private String signatureParts;
    private String allowedSignatureAlgorithms;
    private String allowedDigestAlgorithms;
    private boolean allowExpiredPinnedCertificate;
    private Integer timestampTtlSeconds;
    private Integer timestampFutureToleranceSeconds;
    private Integer faultStatus;
    private List<String> exposedSoapHeadersRaw;
    private List<QName> exposedSoapHeaders = List.of();

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

    public Builder tlsKeystore(String path, String password, String type) {
      this.tlsKeystorePath = path;
      this.tlsKeystorePassword = password;
      this.tlsKeystoreType = type;
      return this;
    }

    public Builder tlsTruststore(String path, String password, String type) {
      this.tlsTruststorePath = path;
      this.tlsTruststorePassword = password;
      this.tlsTruststoreType = type;
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

    /** Replaces the inbound signature allow-list with the comma-separated absolute URIs. */
    public Builder allowedSignatureAlgorithms(String csv) {
      this.allowedSignatureAlgorithms = csv;
      return this;
    }

    /** Replaces the inbound digest allow-list with the comma-separated absolute URIs. */
    public Builder allowedDigestAlgorithms(String csv) {
      this.allowedDigestAlgorithms = csv;
      return this;
    }

    /**
     * Per-profile opt-in for accepting an expired certificate only when it is exactly pinned in the
     * profile's verification store. Default false.
     */
    public Builder allowExpiredPinnedCertificate(boolean v) {
      this.allowExpiredPinnedCertificate = v;
      return this;
    }

    /** Inbound Timestamp TTL in seconds; {@code null} keeps the WSS4J default (300). */
    public Builder timestampTtlSeconds(Integer v) {
      this.timestampTtlSeconds = v;
      return this;
    }

    /** Inbound Timestamp future tolerance in seconds; {@code null} keeps the WSS4J default (60). */
    public Builder timestampFutureToleranceSeconds(Integer v) {
      this.timestampFutureToleranceSeconds = v;
      return this;
    }

    /** Optional WSS-only HTTP status override (400-599); validated by the profile store. */
    public Builder faultStatus(Integer v) {
      this.faultStatus = v;
      return this;
    }

    /**
     * Replaces the application SOAP header allow-list with raw {@code {namespace}local} entries,
     * already split into individual entries by the caller.
     */
    public Builder exposedSoapHeaders(List<String> entries) {
      this.exposedSoapHeadersRaw = entries == null ? null : List.copyOf(entries);
      return this;
    }

    public SecurityProfile build() {
      validateInboundActions();
      validateExpiredPinnedCertificate();
      validateTimestampWindow();
      validateOutboundActions();
      validateSignatureKnobs();
      validateAllowedAlgorithmLists();
      validateExposedSoapHeaders();
      return new SecurityProfile(this);
    }

    /**
     * Fail-loud checks on the inbound Timestamp freshness window (ADR-0088). A non-positive value
     * would otherwise surface only on the first secured message, and a value on a profile without
     * verification material is inert because the consumer path never warns or rejects it.
     */
    private void validateTimestampWindow() {
      validateTimestampKnob("TIMESTAMP_TTL_SECONDS", timestampTtlSeconds);
      validateTimestampKnob("TIMESTAMP_FUTURE_TOLERANCE_SECONDS", timestampFutureToleranceSeconds);
    }

    private void validateTimestampKnob(String knob, Integer v) {
      if (v == null) return;
      String envVar = "CXF_PROFILE_" + name.toUpperCase(Locale.ROOT) + "_" + knob;
      if (v <= 0) {
        throw new IllegalArgumentException(
            envVar + " must be a positive integer number of seconds, but was " + v);
      }
      if (!hasText(truststorePath) && !hasText(keystorePath)) {
        throw new IllegalArgumentException(
            envVar
                + " is set but the profile has no truststore or keystore; the timestamp window"
                + " would be silently inert without verification material (cxf.truststore.path or"
                + " cxf.keystore.path)");
      }
    }

    /**
     * Fail-loud checks on the ALLOWED_* inbound algorithm allow-lists. The lists are exempt from
     * the outbound signing preconditions (a verify-only profile may set them), but they are inert
     * without verification material, so setting one on a profile with no truststore and no keystore
     * would silently leave inbound SHA-1 acceptance to WSS4J defaults. A blank list is rejected
     * because an empty allow-list expresses "allow everything", which is never a safe intent.
     */
    private void validateAllowedAlgorithmLists() {
      validateAllowedAlgorithmList("ALLOWED_SIGNATURE_ALGORITHMS", allowedSignatureAlgorithms);
      validateAllowedAlgorithmList("ALLOWED_DIGEST_ALGORITHMS", allowedDigestAlgorithms);
    }

    private void validateAllowedAlgorithmList(String knob, String raw) {
      if (raw == null) return;
      String envVar = "CXF_PROFILE_" + name.toUpperCase(Locale.ROOT) + "_" + knob;
      List<String> parsed = new ArrayList<>();
      boolean blankSegment = false;
      for (String segment : raw.split(",", -1)) {
        String trimmed = segment.trim();
        if (trimmed.isEmpty()) {
          blankSegment = true;
          continue;
        }
        parsed.add(trimmed);
      }
      if (parsed.isEmpty()) {
        throw new IllegalArgumentException(
            envVar
                + " is set but lists no algorithm URIs; an empty allow-list would allow every"
                + " algorithm, including SHA-1; remove the variable to use the secure defaults"
                + " or list at least one absolute algorithm URI");
      }
      if (blankSegment) {
        throw new IllegalArgumentException(
            envVar
                + " contains a blank algorithm URI segment; remove the empty entry and list"
                + " only absolute algorithm URIs");
      }
      for (String algorithm : parsed) {
        if (!isAbsoluteUri(algorithm)) {
          throw new IllegalArgumentException(
              envVar
                  + " segment '"
                  + algorithm
                  + "' is not an absolute URI; algorithm URIs must be absolute (for example"
                  + " 'http://www.w3.org/2001/04/xmldsig-more#rsa-sha256')");
        }
      }
      if (!hasText(truststorePath) && !hasText(keystorePath)) {
        throw new IllegalArgumentException(
            envVar
                + " is set but the profile has no truststore or keystore; the inbound algorithm"
                + " allow-list would be silently inert without verification material"
                + " (cxf.truststore.path or cxf.keystore.path)");
      }
    }

    /**
     * Fail-loud checks on the application SOAP header allow-list. The grammar is fixed and
     * ASCII-only so the comma-joined env carrier round-trips losslessly. Reserved WS-Security/SOAP
     * namespaces, malformed entries, the 16-entry cap, duplicate QNames, and local names that
     * collide under case-insensitive lookup are all rejected at build.
     */
    private void validateExposedSoapHeaders() {
      String envVar = "CXF_PROFILE_" + name.toUpperCase(Locale.ROOT) + "_EXPOSED_SOAP_HEADERS";
      List<String> raw = exposedSoapHeadersRaw;
      if (raw == null || raw.isEmpty()) {
        exposedSoapHeaders = List.of();
        return;
      }
      if (raw.size() > SoapHeaderExporter.MAX_HEADERS) {
        throw new IllegalArgumentException(
            envVar
                + " lists "
                + raw.size()
                + " entries but at most "
                + SoapHeaderExporter.MAX_HEADERS
                + " application SOAP headers may be exposed per profile");
      }
      List<QName> parsed = new ArrayList<>(raw.size());
      Set<QName> seenQNames = new HashSet<>();
      Set<String> seenLocalNames = new HashSet<>();
      for (String entry : raw) {
        QName qName = parseExposedSoapHeader(envVar, entry);
        if (SoapHeaderExporter.isReservedNamespace(qName.getNamespaceURI())) {
          throw new IllegalArgumentException(
              envVar
                  + " entry '"
                  + entry
                  + "' uses reserved namespace '"
                  + qName.getNamespaceURI()
                  + "'; WS-Security and SOAP envelope headers are never exposed");
        }
        if (!seenQNames.add(qName)) {
          throw new IllegalArgumentException(
              envVar
                  + " lists duplicate QName '"
                  + entry
                  + "'; each header may be listed only once");
        }
        if (!seenLocalNames.add(qName.getLocalPart().toLowerCase(Locale.ROOT))) {
          throw new IllegalArgumentException(
              envVar
                  + " entry '"
                  + entry
                  + "' collides with another entry's local name under case-insensitive"
                  + " comparison; the exchange key is CxfSoapHeader.<localName> and"
                  + " Message::header_ic matches case-insensitively");
        }
        parsed.add(qName);
      }
      exposedSoapHeaders = List.copyOf(parsed);
    }

    /**
     * Fail-loud checks on the RAW configured inbound actions — same discipline as {@link
     * #validateOutboundActions()}: blank/unset actions are raw-exempt (the runtime default
     * "Signature" is only materialized when a truststore exists, and the manual consumer path keeps
     * the keystore fallback). Set membership first: every token must be one the bridge actually
     * materializes — UsernameToken/SAML/SignatureConfirmation are not, so a profile carrying them
     * would silently skip enforcement. Composition second: Timestamp outside a signature is not
     * tamper-evident, so Timestamp requires Signature. Material last: Signature needs verification
     * anchors, which live in the truststore; Encrypt needs decryption keys, which live in the
     * keystore. Without these checks createInInterceptor() silently drops the unmatched action, so
     * the endpoint would skip inbound verification or decryption without any error.
     */
    private void validateInboundActions() {
      String raw = securityActionsIn;
      if (!hasText(raw)) return;
      String unknown = firstUnsupportedAction(raw, SUPPORTED_INBOUND_ACTIONS);
      if (unknown != null) {
        throw new IllegalArgumentException(
            "in-actions include unsupported action '"
                + unknown
                + "' (raw: '"
                + raw
                + "'); supported inbound actions: Signature, Encrypt, Timestamp;"
                + " UsernameToken/SAML/SignatureConfirmation are not materialized");
      }
      if (containsAction(raw, "Timestamp") && !containsAction(raw, "Signature")) {
        throw new IllegalArgumentException(
            "in-actions include Timestamp but not Signature: '"
                + raw
                + "'. A Timestamp emitted outside the signature is not tamper-evident");
      }
      if (containsAction(raw, "Signature") && !hasText(truststorePath)) {
        throw new IllegalArgumentException(
            "in-actions include Signature but no truststore is configured: '"
                + raw
                + "'. Verification anchors live in the truststore (cxf.truststore.path); the"
                + " keystore fallback applies only to the manual consumer path; the truststore"
                + " may point at the same JKS as the keystore");
      }
      if (containsAction(raw, "Encrypt") && !hasText(keystorePath)) {
        throw new IllegalArgumentException(
            "in-actions include Encrypt but no keystore is configured: '"
                + raw
                + "'. Decryption keys live in the keystore (cxf.keystore.path)");
      }
    }

    /**
     * Fail-loud checks on the expired-pinned-certificate opt-in. The relaxation only applies to
     * inbound Signature verification against an exactly-pinned certificate, so an opted-in profile
     * must resolve inbound actions to Signature and carry verification material. Blank inbound
     * actions resolve to Signature through {@link #resolveActionsOrDefault(String)}; an explicit
     * action list without Signature is rejected.
     */
    private void validateExpiredPinnedCertificate() {
      if (!allowExpiredPinnedCertificate) return;
      String envVar =
          "CXF_PROFILE_" + name.toUpperCase(Locale.ROOT) + "_ALLOW_EXPIRED_PINNED_CERTIFICATE";
      String inActions = resolveActionsOrDefault(securityActionsIn);
      if (!containsAction(inActions, "Signature")) {
        throw new IllegalArgumentException(
            envVar
                + " is set but resolved in-actions do not include Signature: '"
                + inActions
                + "'; expired pinned certificate acceptance applies to inbound Signature"
                + " verification only");
      }
      if (!hasText(truststorePath) && !hasText(keystorePath)) {
        throw new IllegalArgumentException(
            envVar
                + " is set but the profile has no verification material; a truststore or keystore"
                + " is required to pin the accepted certificate (cxf.truststore.path or"
                + " cxf.keystore.path)");
      }
    }

    /**
     * Fail-loud checks on the RAW configured outbound actions. Blank/unset actions are raw-exempt:
     * the runtime default ("Signature") is resolved later and validated by {@link
     * #validateSignatureKnobs()}. Set membership first: every token must be one the bridge actually
     * materializes. Composition second: Timestamp outside a signature is not tamper-evident, so
     * Timestamp requires Signature. Material third: Signature, Encrypt, and Timestamp all need
     * crypto material, so a keystore is required. Inbound counterpart: {@link
     * #validateInboundActions()} applies the same membership, composition, and material-per-action
     * discipline to actions.in — together the two validators form one build-time security contract.
     */
    private void validateOutboundActions() {
      String raw = securityActionsOut;
      if (!hasText(raw)) return;
      String unknown = firstUnsupportedAction(raw, SUPPORTED_OUTBOUND_ACTIONS);
      if (unknown != null) {
        throw new IllegalArgumentException(
            "out-actions include unsupported action '"
                + unknown
                + "' (raw: '"
                + raw
                + "'); supported outbound actions: Signature, Encrypt, Timestamp;"
                + " UsernameToken/SAML/SignatureConfirmation are not materialized");
      }
      if (containsAction(raw, "Timestamp") && !containsAction(raw, "Signature")) {
        throw new IllegalArgumentException(
            "out-actions include Timestamp but not Signature: '"
                + raw
                + "'. A Timestamp emitted outside the signature is not tamper-evident");
      }
      if ((containsAction(raw, "Signature")
              || containsAction(raw, "Encrypt")
              || containsAction(raw, "Timestamp"))
          && !hasText(keystorePath)) {
        throw new IllegalArgumentException(
            "out-actions include Signature/Encrypt/Timestamp but no keystore is configured: '"
                + raw
                + "'");
      }
    }

    /**
     * Rejects SIGNATURE_* knobs that would otherwise be dead configuration: they require a
     * Signature out-action, a signing keystore, a valid parts grammar, and absolute-URI algorithms.
     */
    private void validateSignatureKnobs() {
      String knobEnvVar = firstSignatureKnobEnvVar();
      if (knobEnvVar == null) return;

      String outActions = resolveActionsOrDefault(securityActionsOut);
      if (!containsAction(outActions, "Signature")) {
        throw new IllegalArgumentException(
            knobEnvVar + " is set but out-actions do not include Signature: '" + outActions + "'");
      }
      if (!hasText(keystorePath)) {
        throw new IllegalArgumentException(
            knobEnvVar + " is set but no signing keystore is configured");
      }
      if (hasText(signatureAlgorithm) && !isAbsoluteUri(signatureAlgorithm)) {
        throw new IllegalArgumentException(
            "SIGNATURE_ALGORITHM is not an absolute URI: '" + signatureAlgorithm + "'");
      }
      if (hasText(signatureDigestAlgorithm) && !isAbsoluteUri(signatureDigestAlgorithm)) {
        throw new IllegalArgumentException(
            "SIGNATURE_DIGEST_ALGORITHM is not an absolute URI: '"
                + signatureDigestAlgorithm
                + "'");
      }
      if (hasText(signatureC14nAlgorithm) && !isAbsoluteUri(signatureC14nAlgorithm)) {
        throw new IllegalArgumentException(
            "SIGNATURE_C14N_ALGORITHM is not an absolute URI: '" + signatureC14nAlgorithm + "'");
      }
      if (hasText(signatureParts)) {
        validateSignaturePartsSyntax(signatureParts);
      }
    }

    private String firstSignatureKnobEnvVar() {
      if (hasText(signatureAlgorithm)) return "SIGNATURE_ALGORITHM";
      if (hasText(signatureDigestAlgorithm)) return "SIGNATURE_DIGEST_ALGORITHM";
      if (hasText(signatureC14nAlgorithm)) return "SIGNATURE_C14N_ALGORITHM";
      if (hasText(signatureParts)) return "SIGNATURE_PARTS";
      return null;
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

  /** Default the store type to PKCS12 when unset (spec: store type defaults). */
  private static String normalizeTlsType(String type) {
    return hasText(type) ? type : "PKCS12";
  }

  /** Parses a comma-separated algorithm list into trimmed, ordered URIs. */
  static List<String> parseAlgorithmList(String csv) {
    List<String> algorithms = new ArrayList<>();
    for (String segment : csv.split(",", -1)) {
      String trimmed = segment.trim();
      if (!trimmed.isEmpty()) {
        algorithms.add(trimmed);
      }
    }
    return List.copyOf(algorithms);
  }

  /**
   * Parses one {@code {namespace}local} allow-list entry by hand (no whitespace regex). The
   * namespace may not contain braces, commas, or Unicode whitespace; the local name must match
   * {@code [A-Za-z_][A-Za-z0-9._-]*}.
   */
  private static QName parseExposedSoapHeader(String envVar, String entry) {
    if (entry == null || !entry.startsWith("{")) {
      throw malformedExposedSoapHeader(envVar, entry);
    }
    int namespaceEnd = entry.indexOf('}');
    if (namespaceEnd < 0) {
      throw malformedExposedSoapHeader(envVar, entry);
    }
    String namespace = entry.substring(1, namespaceEnd);
    String localName = entry.substring(namespaceEnd + 1);
    if (namespace.isEmpty()
        || localName.isEmpty()
        || namespace.indexOf('{') >= 0
        || namespace.indexOf('}') >= 0
        || localName.indexOf('{') >= 0
        || localName.indexOf('}') >= 0
        || !isValidSoapHeaderLocalName(localName)) {
      throw malformedExposedSoapHeader(envVar, entry);
    }
    for (int i = 0; i < namespace.length(); ) {
      int codePoint = namespace.codePointAt(i);
      if (codePoint == ',' || BlankTlsValue.isWhiteSpaceCodePoint(codePoint)) {
        throw malformedExposedSoapHeader(envVar, entry);
      }
      i += Character.charCount(codePoint);
    }
    return new QName(namespace, localName);
  }

  private static IllegalArgumentException malformedExposedSoapHeader(String envVar, String entry) {
    return new IllegalArgumentException(
        envVar
            + " entry '"
            + entry
            + "' is not a valid {namespace}local QName; the namespace must be non-empty and"
            + " contain no '{', '}', ',' or whitespace, and the local name must match"
            + " [A-Za-z_][A-Za-z0-9._-]*");
  }

  private static boolean isValidSoapHeaderLocalName(String localName) {
    if (localName.isEmpty()) return false;
    if (!isAsciiSoapHeaderNameStart(localName.charAt(0))) return false;
    for (int i = 1; i < localName.length(); i++) {
      if (!isAsciiSoapHeaderNamePart(localName.charAt(i))) return false;
    }
    return true;
  }

  private static boolean isAsciiSoapHeaderNameStart(char c) {
    return (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || c == '_';
  }

  private static boolean isAsciiSoapHeaderNamePart(char c) {
    return isAsciiSoapHeaderNameStart(c) || (c >= '0' && c <= '9') || c == '.' || c == '-';
  }

  private static boolean allowsSha1(List<String> algorithms) {
    for (String algorithm : algorithms) {
      if (SHA1_ALGORITHM_URIS.contains(algorithm)) {
        return true;
      }
    }
    return false;
  }

  /**
   * Validates SIGNATURE_PARTS grammar: ';'-separated segments, each a bare non-empty localName or
   * {@code {modifier}{namespace}localName} with modifier empty or exactly {@code Element}/{@code
   * Content}, namespace possibly empty, and a non-empty localName.
   *
   * @throws IllegalArgumentException naming SIGNATURE_PARTS on the first malformed segment
   */
  static void validateSignaturePartsSyntax(String parts) {
    for (String segment : parts.split(";", -1)) {
      if (!isWellFormedPartSegment(segment)) {
        throw new IllegalArgumentException(
            "SIGNATURE_PARTS has a malformed segment '"
                + segment
                + "': expected 'localName' or '{Element|Content}{namespace}localName'"
                + " segments separated by ';'");
      }
    }
  }

  private static boolean isWellFormedPartSegment(String segment) {
    if (!hasText(segment)) return false;
    // Untrimmed or whitespace-carrying segments would pass through to WSS4J
    // verbatim and fail at first invoke, not at construction — reject here.
    if (!segment.equals(segment.trim())) return false;
    String s = segment;
    if (!s.contains("{") && !s.contains("}"))
      return !s.chars().anyMatch(Character::isWhitespace); // bare localName
    if (!s.startsWith("{")) return false;
    int modifierEnd = s.indexOf('}');
    if (modifierEnd < 0) return false;
    String modifier = s.substring(1, modifierEnd);
    if (!modifier.isEmpty() && !modifier.equals("Element") && !modifier.equals("Content")) {
      return false;
    }
    String rest = s.substring(modifierEnd + 1);
    if (!rest.startsWith("{")) return false;
    int namespaceEnd = rest.indexOf('}');
    if (namespaceEnd < 0) return false;
    String namespace = rest.substring(1, namespaceEnd);
    String localName = rest.substring(namespaceEnd + 1);
    // wss4j splits segments on EVERY '}', so stray braces inside namespace or localName
    // shift fragment boundaries and fail at SIGNING time ("wrong part definition").
    if (namespace.indexOf('{') >= 0 || namespace.indexOf('}') >= 0) return false;
    if (localName.indexOf('{') >= 0 || localName.indexOf('}') >= 0) return false;
    if (localName.chars().anyMatch(Character::isWhitespace)) return false;
    return !localName.isEmpty(); // non-empty localName
  }

  private static boolean isAbsoluteUri(String value) {
    try {
      return URI.create(value).isAbsolute();
    } catch (IllegalArgumentException e) {
      return false;
    }
  }

  private static boolean containsAction(String actions, String token) {
    if (actions == null || actions.isBlank()) return false;
    for (String part : actions.split("\\s+")) {
      if (part.equalsIgnoreCase(token)) return true;
    }
    return false;
  }

  /**
   * First token of {@code raw} not present (case-insensitively, via lowercase comparison) in {@code
   * supported}, or null if every token is supported. Same whitespace token-splitting as {@link
   * #containsAction(String, String)}.
   */
  private static String firstUnsupportedAction(String raw, Set<String> supported) {
    for (String part : raw.split("\\s+")) {
      // Leading/trailing whitespace or doubled separators yield blank parts; skip them
      // (containsAction tolerates the same raw string, so validation must too).
      if (part.isBlank()) continue;
      if (!supported.contains(part.toLowerCase(Locale.ROOT))) return part;
    }
    return null;
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
