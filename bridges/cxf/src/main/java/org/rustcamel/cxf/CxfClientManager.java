package org.rustcamel.cxf;

import jakarta.annotation.PostConstruct;
import jakarta.annotation.PreDestroy;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;
import jakarta.xml.ws.Dispatch;
import jakarta.xml.ws.Service;
import java.io.File;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
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
  private final Map<String, Dispatch<Source>> dispatches = new ConcurrentHashMap<>();

  @Inject BridgeConfig bridgeConfig;

  @Inject SecurityConfig securityConfig;

  @PostConstruct
  void init() {
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

  public Dispatch<Source> getDispatch(String wsdl, String service, String port, String operation)
      throws Exception {
    return getDispatch(wsdl, "", service, port, operation);
  }

  public Dispatch<Source> getDispatch(
      String wsdl, String address, String service, String port, String operation) throws Exception {
    String key = wsdl + "#" + address + "#" + service + "#" + port;
    try {
      Dispatch<Source> dispatch =
          dispatches.computeIfAbsent(key, k -> createDispatch(wsdl, address, service, port));
      if (operation != null && !operation.isBlank()) {
        dispatch.getRequestContext().put("jakarta.xml.ws.soap.http.soapaction.use", Boolean.TRUE);
        dispatch.getRequestContext().put("jakarta.xml.ws.soap.http.soapaction.uri", operation);
      }
      return dispatch;
    } catch (RuntimeException ex) {
      if (ex.getCause() instanceof Exception nested) {
        throw nested;
      }
      throw ex;
    }
  }

  private Dispatch<Source> createDispatch(
      String wsdl, String address, String service, String port) {
    try {
      QName serviceQName = service.startsWith("{") ? QName.valueOf(service) : new QName(service);
      QName portQName = port.startsWith("{") ? QName.valueOf(port) : new QName(port);

      Service jaxwsService = Service.create(new File(wsdl).toURI().toURL(), serviceQName);
      Dispatch<Source> dispatch =
          jaxwsService.createDispatch(portQName, Source.class, Service.Mode.PAYLOAD);

      if (address != null && !address.isBlank()) {
        dispatch.getRequestContext().put("jakarta.xml.ws.service.endpoint.address", address);
      }

      String timeout = String.valueOf(bridgeConfig.connectionTimeoutMs());
      dispatch.getRequestContext().put("jakarta.xml.ws.client.connectionTimeout", timeout);
      dispatch.getRequestContext().put("jakarta.xml.ws.client.receiveTimeout", timeout);

      if (securityConfig.hasSecurity() && dispatch instanceof DispatchImpl<Source> cxfDispatch) {
        var out = securityConfig.createOutInterceptor();
        if (out != null) {
          cxfDispatch.getClient().getOutInterceptors().add(out);
        }
        var in = securityConfig.createInInterceptor();
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
    dispatches.clear();
  }
}
