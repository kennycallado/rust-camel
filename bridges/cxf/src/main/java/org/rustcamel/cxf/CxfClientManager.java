package org.rustcamel.cxf;

import jakarta.annotation.PostConstruct;
import jakarta.annotation.PreDestroy;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;
import jakarta.xml.ws.Dispatch;
import jakarta.xml.ws.Service;
import java.io.File;
import java.util.Collections;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.logging.Logger;
import javax.xml.namespace.QName;
import javax.xml.transform.Source;
import org.apache.cxf.Bus;
import org.apache.cxf.BusFactory;
import org.apache.cxf.bus.CXFBusFactory;
import org.apache.cxf.jaxws.DispatchImpl;

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
      long timeoutMs) {}

  /**
   * Access-order LRU Dispatch cache, bounded by {@code CXF_MAX_DISPATCHES}. Lookup and cold
   * creation are serialized under the map monitor; the SOAP invoke stays outside it.
   */
  private final Map<DispatchKey, Dispatch<Source>> dispatches =
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

  public Dispatch<Source> getDispatch(
      String wsdl,
      String address,
      String service,
      String port,
      String operation,
      String profileName,
      long timeoutMs)
      throws Exception {
    SecurityProfile profile = profileStore.getProfile(profileName);
    String normalizedOperation = operation == null || operation.isBlank() ? "" : operation;
    DispatchKey key =
        new DispatchKey(wsdl, address, service, port, profileName, normalizedOperation, timeoutMs);
    try {
      synchronized (dispatches) {
        Dispatch<Source> existing = dispatches.get(key);
        if (existing != null) {
          return existing;
        }
        Dispatch<Source> created =
            createDispatch(
                wsdl, address, service, port, profile, normalizedOperation, key.timeoutMs());
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
    Dispatch<Source> evicted = dispatches.remove(lru);
    closeQuietly(evicted);
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
      String operation,
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

      if (operation != null && !operation.isBlank()) {
        dispatch.getRequestContext().put("jakarta.xml.ws.soap.http.soapaction.use", Boolean.TRUE);
        dispatch.getRequestContext().put("jakarta.xml.ws.soap.http.soapaction.uri", operation);
      }

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
      return dispatch;
    } catch (Exception e) {
      throw new RuntimeException(e);
    }
  }

  @PreDestroy
  void close() {
    synchronized (dispatches) {
      for (Dispatch<Source> dispatch : dispatches.values()) {
        closeQuietly(dispatch);
      }
      dispatches.clear();
    }
  }
}
