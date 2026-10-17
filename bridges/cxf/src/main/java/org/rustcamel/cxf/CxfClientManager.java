package org.rustcamel.cxf;

import jakarta.annotation.PostConstruct;
import jakarta.annotation.PreDestroy;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;
import jakarta.xml.ws.Dispatch;
import jakarta.xml.ws.Service;
import java.io.File;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyStore;
import java.util.Collections;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.logging.Logger;
import javax.net.ssl.KeyManager;
import javax.net.ssl.KeyManagerFactory;
import javax.net.ssl.TrustManager;
import javax.net.ssl.TrustManagerFactory;
import javax.xml.namespace.QName;
import javax.xml.transform.Source;
import org.apache.cxf.Bus;
import org.apache.cxf.BusFactory;
import org.apache.cxf.bus.CXFBusFactory;
import org.apache.cxf.configuration.jsse.TLSClientParameters;
import org.apache.cxf.endpoint.Client;
import org.apache.cxf.jaxws.DispatchImpl;
import org.apache.cxf.service.model.BindingInfo;
import org.apache.cxf.transport.Conduit;
import org.apache.cxf.transport.http.HTTPConduit;

@ApplicationScoped
public class CxfClientManager {
  private static final Logger LOG = Logger.getLogger(CxfClientManager.class.getName());

  private record DispatchKey(
      String wsdl,
      String address,
      String service,
      String port,
      String profile,
      String operation,
      QName inputQName,
      long timeoutMs) {}

  /**
   * A cached dispatch together with the immutable operation shape it was configured for. Callers
   * must not mutate the dispatch request context after it is published; creation is the only
   * writer.
   */
  record ResolvedDispatch(Dispatch<Source> dispatch, WsdlOperationCatalog.Operation operation) {}

  /**
   * An explicitly requested operation does not exist in the selected WSDL port binding. Extends
   * {@link IllegalArgumentException} so manager-level callers keep catching argument-class failures
   * and the legacy explicit-operation contract stays compatible. The rejected dispatch is closed
   * and never cached before this is thrown, so no wire transmission can occur.
   */
  static final class UnknownOperationException extends IllegalArgumentException {
    UnknownOperationException(String message) {
      super(message);
    }
  }

  /**
   * Access-order LRU Dispatch cache, bounded by {@code CXF_MAX_DISPATCHES}. Lookup and cold
   * creation are serialized under the map monitor; the SOAP invoke stays outside it. Each value
   * carries the operation metadata selected at creation, so no parallel metadata cache is needed.
   */
  private final Map<DispatchKey, ResolvedDispatch> dispatches =
      Collections.synchronizedMap(new LinkedHashMap<>(16, 0.75f, true));

  /** Test seam: pins the dispatch-cache bound deterministically. */
  int pinnedMaxDispatches = -1;

  @Inject SecurityProfileStore profileStore;

  @PostConstruct
  void init() {
    int maxDispatches = resolveMaxDispatches();
    LOG.fine("Dispatch cache bound: " + maxDispatches);
    try {
      Bus bus = new CXFBusFactory().createBus();
      LOG.fine("CXF Bus initialized: " + bus);
      BusFactory.setDefaultBus(bus);
      BusFactory.setThreadDefaultBus(bus);
    } catch (Exception e) {
      LOG.severe("Bus init failed: " + e);
      throw e;
    }
  }

  /**
   * Explicit-operation compatibility delegate. Production unnamed dispatch must call {@link
   * #getResolvedDispatch} with the input QName instead. The no-name form is rejected by resolution
   * because it cannot select a unique binding operation.
   */
  public Dispatch<Source> getDispatch(
      String wsdl,
      String address,
      String service,
      String port,
      String operation,
      String profileName,
      long timeoutMs)
      throws Exception {
    return getResolvedDispatch(
            wsdl, address, service, port, operation, profileName, timeoutMs, null)
        .dispatch();
  }

  /**
   * Resolves the selected binding operation for this dispatch and returns the dispatch together
   * with its immutable shape. The cache key includes both the explicit operation name and the
   * inferred input QName so a dispatch configured for one operation can never serve another. On
   * cache miss a temporary dispatch is created, its selected binding is snapshotted, the operation
   * is resolved, and only then is operation-specific request context configured and the entry
   * published. A rejected selection closes the temporary dispatch and is never cached.
   */
  public ResolvedDispatch getResolvedDispatch(
      String wsdl,
      String address,
      String service,
      String port,
      String operation,
      String profileName,
      long timeoutMs,
      QName inputQName)
      throws Exception {
    SecurityProfile profile = profileStore.getProfile(profileName);
    String normalizedOperation = operation == null || operation.isBlank() ? "" : operation;
    DispatchKey key =
        new DispatchKey(
            wsdl, address, service, port, profileName, normalizedOperation, inputQName, timeoutMs);
    try {
      synchronized (dispatches) {
        ResolvedDispatch existing = dispatches.get(key);
        if (existing != null) {
          return existing;
        }
        ResolvedDispatch created =
            createResolvedDispatch(
                wsdl,
                address,
                service,
                port,
                profile,
                normalizedOperation,
                key.timeoutMs(),
                inputQName);
        int maxDispatches = resolveMaxDispatches();
        while (dispatches.size() >= maxDispatches) {
          evictLruDispatch();
        }
        dispatches.put(key, created);
        return created;
      }
    } catch (RuntimeException ex) {
      if (ex.getCause() instanceof Exception nested) {
        throw nested;
      }
      throw ex;
    }
  }

  /** Evicts the least-recently-used entry and closes it best-effort. */
  private void evictLruDispatch() {
    Iterator<DispatchKey> iterator = dispatches.keySet().iterator();
    DispatchKey lru = iterator.next();
    ResolvedDispatch evicted = dispatches.remove(lru);
    closeQuietly(evicted.dispatch());
    LOG.fine(
        "Evicted LRU dispatch: "
            + new File(lru.wsdl()).getName()
            + " "
            + lru.address()
            + " "
            + lru.port());
  }

  int cacheSize() {
    return dispatches.size();
  }

  /**
   * Loads and validates every profile's producer transport TLS stores eagerly. A store that cannot
   * be opened (missing file, wrong password, unsupported type) fails here, during bridge startup
   * and before the gRPC {@code ready} line, rather than lazily on the first dispatch. The validated
   * material is not cached: {@link #createDispatch} reloads it per dispatch so each profile keeps
   * its own stores.
   */
  void initTlsStores() {
    for (SecurityProfile profile : profileStore.profiles().values()) {
      if (!profile.hasTls()) {
        continue;
      }
      buildTlsClientParameters(profile);
      LOG.info("Producer transport TLS stores validated for profile '" + profile.name() + "'");
    }
  }

  /**
   * Builds the CXF {@link TLSClientParameters} for one profile: key managers from the configured
   * keystore and trust managers from the configured truststore. The store type defaults to PKCS12
   * when unset (already normalized by {@link SecurityProfile}). Hostname verification is left at
   * its secure default — there is deliberately no option that disables it.
   */
  static TLSClientParameters buildTlsClientParameters(SecurityProfile profile) {
    try {
      TLSClientParameters params = new TLSClientParameters();
      if (hasText(profile.tlsKeystorePath())) {
        params.setKeyManagers(
            loadKeyManagers(
                profile.tlsKeystorePath(),
                profile.tlsKeystorePassword(),
                profile.tlsKeystoreType()));
      }
      if (hasText(profile.tlsTruststorePath())) {
        params.setTrustManagers(
            loadTrustManagers(
                profile.tlsTruststorePath(),
                profile.tlsTruststorePassword(),
                profile.tlsTruststoreType()));
      }
      return params;
    } catch (Exception e) {
      throw new IllegalStateException(
          "Failed to load producer transport TLS stores for profile '"
              + profile.name()
              + "'"
              + ": "
              + e.getMessage(),
          e);
    }
  }

  private static KeyManager[] loadKeyManagers(String path, String password, String type)
      throws Exception {
    KeyStore keyStore = loadStore(path, password, type);
    KeyManagerFactory factory =
        KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm());
    factory.init(keyStore, password == null ? null : password.toCharArray());
    return factory.getKeyManagers();
  }

  private static TrustManager[] loadTrustManagers(String path, String password, String type)
      throws Exception {
    KeyStore trustStore = loadStore(path, password, type);
    TrustManagerFactory factory =
        TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
    factory.init(trustStore);
    return factory.getTrustManagers();
  }

  private static KeyStore loadStore(String path, String password, String type) throws Exception {
    KeyStore store = KeyStore.getInstance(hasText(type) ? type : "PKCS12");
    char[] resolvedPassword = password == null ? null : password.toCharArray();
    try (InputStream in = Files.newInputStream(Path.of(path))) {
      store.load(in, resolvedPassword);
    }
    return store;
  }

  private static boolean hasText(String value) {
    return value != null && !value.isBlank();
  }

  private int resolveMaxDispatches() {
    return pinnedMaxDispatches > 0 ? pinnedMaxDispatches : BridgeConfig.parseMaxDispatches();
  }

  void closeQuietly(Dispatch<Source> dispatch) {
    if (dispatch instanceof DispatchImpl<Source> impl) {
      try {
        impl.close();
      } catch (Exception e) {
        LOG.fine("Dispatch close failed: " + e);
      }
    }
  }

  private Dispatch<Source> createDispatch(
      String wsdl,
      String address,
      String service,
      String port,
      SecurityProfile profile,
      long timeoutMs) {
    try {
      QName serviceQName = service.startsWith("{") ? QName.valueOf(service) : new QName(service);
      QName portQName = port.startsWith("{") ? QName.valueOf(port) : new QName(port);

      Service jaxwsService = Service.create(new File(wsdl).toURI().toURL(), serviceQName);
      Dispatch<Source> dispatch =
          jaxwsService.createDispatch(portQName, Source.class, Service.Mode.PAYLOAD);

      if (address != null && !address.isBlank()) {
        dispatch.getRequestContext().put("jakarta.xml.ws.service.endpoint.address", address);
      }

      String timeout = String.valueOf(timeoutMs);
      dispatch.getRequestContext().put("jakarta.xml.ws.client.connectionTimeout", timeout);
      dispatch.getRequestContext().put("jakarta.xml.ws.client.receiveTimeout", timeout);

      if (profile.hasSecurity() && dispatch instanceof DispatchImpl<Source> cxfDispatch) {
        var out = profile.createOutInterceptor();
        if (out != null) {
          cxfDispatch.getClient().getOutInterceptors().add(out);
        }
        var in = profile.createInInterceptor();
        if (in != null) {
          cxfDispatch.getClient().getInInterceptors().add(in);
        }
      }

      if (profile.hasTls() && dispatch instanceof DispatchImpl<Source> tlsDispatch) {
        attachTlsClientParameters(tlsDispatch, profile);
      }
      return dispatch;
    } catch (Exception e) {
      throw new RuntimeException(e);
    }
  }

  /**
   * Creates a dispatch, resolves its selected binding operation, configures the operation-specific
   * request context, and returns both together. Any failure after creation closes the temporary
   * dispatch and propagates the original argument exception without caching it.
   */
  private ResolvedDispatch createResolvedDispatch(
      String wsdl,
      String address,
      String service,
      String port,
      SecurityProfile profile,
      String operation,
      long timeoutMs,
      QName inputQName) {
    Dispatch<Source> dispatch = null;
    try {
      dispatch = createDispatch(wsdl, address, service, port, profile, timeoutMs);
      BindingInfo binding = selectedBinding(dispatch);
      WsdlOperationCatalog.Operation resolved;
      try {
        resolved = WsdlOperationCatalog.fromBinding(binding).resolveProducer(operation, inputQName);
      } catch (WsdlOperationCatalog.InvalidOperationException e) {
        if (operation.isEmpty()) {
          throw e;
        }
        throw new UnknownOperationException(
            "Unknown operation '" + operation + "' in WSDL binding " + binding.getName());
      }
      applyOperationContext(dispatch, resolved);
      return new ResolvedDispatch(dispatch, resolved);
    } catch (RuntimeException e) {
      closeQuietly(dispatch);
      throw e;
    } catch (Exception e) {
      closeQuietly(dispatch);
      throw new RuntimeException(e);
    }
  }

  private static BindingInfo selectedBinding(Dispatch<Source> dispatch) {
    if (!(dispatch instanceof DispatchImpl<Source> cxfDispatch)) {
      throw new IllegalStateException("Operation resolution requires a CXF dispatch");
    }
    return cxfDispatch.getClient().getEndpoint().getEndpointInfo().getBinding();
  }

  /**
   * Writes operation-specific request context exactly once, at creation. The SOAPAction comes from
   * the resolved operation's metadata. An input-only operation is deliberately non-robust. Nothing
   * here runs after the dispatch is published, so a cached entry is never mutated.
   */
  private static void applyOperationContext(
      Dispatch<Source> dispatch, WsdlOperationCatalog.Operation operation) {
    Map<String, Object> context = dispatch.getRequestContext();
    context.put("jakarta.xml.ws.soap.http.soapaction.use", Boolean.TRUE);
    context.put("jakarta.xml.ws.soap.http.soapaction.uri", operation.soapAction());
    if (operation.oneWay()) {
      context.put("org.apache.cxf.oneway.robust", Boolean.FALSE);
    }
  }

  /**
   * Attaches the profile's transport TLS material to the dispatch's CXF {@link HTTPConduit}. A
   * configured TLS block with a non-HTTP conduit is a hard failure: silently ignoring it would
   * downgrade the connection to the JVM defaults.
   */
  private static void attachTlsClientParameters(
      DispatchImpl<Source> dispatch, SecurityProfile profile) {
    Client client = dispatch.getClient();
    Conduit conduit = client == null ? null : client.getConduit();
    if (!(conduit instanceof HTTPConduit httpConduit)) {
      throw new IllegalStateException(
          "CXF dispatch conduit is not an HTTPConduit for profile '"
              + profile.name()
              + "': "
              + (conduit == null ? "null" : conduit.getClass().getName()));
    }
    httpConduit.setTlsClientParameters(buildTlsClientParameters(profile));
  }

  @PreDestroy
  void close() {
    synchronized (dispatches) {
      for (ResolvedDispatch resolved : dispatches.values()) {
        closeQuietly(resolved.dispatch());
      }
      dispatches.clear();
    }
  }
}
