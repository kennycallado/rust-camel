package org.rustcamel.cxf;

import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;
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
import org.apache.wss4j.common.ext.WSPasswordCallback;
import org.apache.wss4j.dom.engine.WSSConfig;
import org.apache.wss4j.dom.handler.WSHandlerConstants;

@ApplicationScoped
public class SecurityConfig {
  private static final Logger LOG = Logger.getLogger(SecurityConfig.class.getName());

  @Inject BridgeConfig bridgeConfig;

  public boolean hasSecurity() {
    return hasText(bridgeConfig.securityUsername())
        || hasText(bridgeConfig.keystorePath())
        || hasText(bridgeConfig.truststorePath());
  }

  public Interceptor<? extends Message> createOutInterceptor() {
    WSSConfig.init();

    if (!hasSecurity()) {
      return null;
    }

    Map<String, Object> props = new HashMap<>();
    List<String> actions = new ArrayList<>();
    boolean hasUsernameToken = false;
    boolean hasX509 = false;

    if (hasText(bridgeConfig.securityUsername())) {
      hasUsernameToken = true;
      actions.add(WSHandlerConstants.USERNAME_TOKEN);
      props.put(WSHandlerConstants.USER, bridgeConfig.securityUsername());
      props.put(ConfigurationConstants.PASSWORD_TYPE, "PasswordText");
      props.put(
          WSHandlerConstants.PW_CALLBACK_REF,
          callbackWithPassword(bridgeConfig.securityPassword()));
    }

    if (hasText(bridgeConfig.keystorePath())) {
      hasX509 = true;
      actions.add(WSHandlerConstants.SIGNATURE);
      actions.add(WSHandlerConstants.ENCRYPT);
      props.put(
          ConfigurationConstants.SIG_PROP_REF_ID,
          createCryptoProperties(bridgeConfig.keystorePath(), bridgeConfig.keystorePassword()));
      props.put(
          ConfigurationConstants.ENC_PROP_REF_ID,
          createCryptoProperties(
              resolveTruststorePathForEncryption(), resolveTruststorePasswordForEncryption()));
      String signatureUser =
          hasText(bridgeConfig.securityUsername())
              ? bridgeConfig.securityUsername()
              : bridgeConfig.sigUsername();
      props.put(WSHandlerConstants.USER, signatureUser);
      props.put(ConfigurationConstants.ENCRYPTION_USER, bridgeConfig.encUsername());
      props.put(WSHandlerConstants.SIG_KEY_ID, "DirectReference");
      props.put(
          WSHandlerConstants.ENC_KEY_TRANSPORT,
          "http://www.w3.org/2001/04/xmlenc#rsa-oaep-mgf1p");
      props.put(WSHandlerConstants.ENC_SYM_ALGO, "http://www.w3.org/2001/04/xmlenc#aes256-cbc");
    }

    if (actions.isEmpty()) {
      return null;
    }

    props.put(WSHandlerConstants.ACTION, String.join(" ", actions));
    LOG.info(
        "WS-Security enabled: UsernameToken="
            + hasUsernameToken
            + ", X509="
            + hasX509);
    return new WSS4JOutInterceptor(props);
  }

  public Interceptor<? extends Message> createInInterceptor() {
    WSSConfig.init();

    if (!hasText(bridgeConfig.truststorePath()) && !hasText(bridgeConfig.keystorePath())) {
      return null;
    }

    Map<String, Object> props = new HashMap<>();
    List<String> actions = new ArrayList<>();
    boolean hasX509 = false;

    if (hasText(bridgeConfig.truststorePath())) {
      hasX509 = true;
      actions.add(WSHandlerConstants.SIGNATURE);
      props.put(
          ConfigurationConstants.SIG_PROP_REF_ID,
          createCryptoProperties(bridgeConfig.truststorePath(), bridgeConfig.truststorePassword()));
      actions.add(WSHandlerConstants.ENCRYPT);
      props.put(
          ConfigurationConstants.DEC_PROP_REF_ID,
          createCryptoProperties(bridgeConfig.keystorePath(), bridgeConfig.keystorePassword()));
      props.put(
          WSHandlerConstants.PW_CALLBACK_REF,
          callbackWithPassword(bridgeConfig.keystorePassword()));
    }

    if (actions.isEmpty()) {
      return null;
    }

    props.put(WSHandlerConstants.ACTION, String.join(" ", actions));
    LOG.info(
        "WS-Security enabled: UsernameToken="
            + false
            + ", X509="
            + hasX509);
    return new WSS4JInInterceptor(props);
  }

  private String resolveTruststorePathForEncryption() {
    return hasText(bridgeConfig.truststorePath())
        ? bridgeConfig.truststorePath()
        : bridgeConfig.keystorePath();
  }

  private String resolveTruststorePasswordForEncryption() {
    return hasText(bridgeConfig.truststorePassword())
        ? bridgeConfig.truststorePassword()
        : bridgeConfig.keystorePassword();
  }

  private static CallbackHandler callbackWithPassword(String password) {
    return (Callback[] callbacks) -> {
      for (Callback callback : callbacks) {
        if (callback instanceof WSPasswordCallback wsPasswordCallback && password != null) {
          wsPasswordCallback.setPassword(password);
        }
      }
    };
  }

  private static Properties createCryptoProperties(String path, String password) {
    Properties crypto = new Properties();
    if (!hasText(path)) {
      return crypto;
    }
    crypto.put("org.apache.wss4j.crypto.provider", "org.apache.wss4j.common.crypto.Merlin");
    crypto.put("org.apache.wss4j.crypto.merlin.keystore.type", "JKS");
    crypto.put("org.apache.wss4j.crypto.merlin.keystore.file", path);
    if (password != null) {
      crypto.put("org.apache.wss4j.crypto.merlin.keystore.password", password);
    }
    return crypto;
  }

  private static boolean hasText(String value) {
    return value != null && !value.isBlank();
  }
}
