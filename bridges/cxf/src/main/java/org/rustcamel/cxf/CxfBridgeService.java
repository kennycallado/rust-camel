package org.rustcamel.cxf;

import com.google.protobuf.ByteString;
import cxf_bridge.*;
import io.grpc.Status;
import io.grpc.stub.StreamObserver;
import io.quarkus.grpc.GrpcService;
import io.smallrye.common.annotation.Blocking;
import jakarta.inject.Inject;
import jakarta.xml.ws.soap.SOAPFaultException;
import java.io.StringReader;
import java.lang.reflect.UndeclaredThrowableException;
import java.util.logging.Level;
import java.util.logging.Logger;
import javax.xml.transform.Source;
import javax.xml.transform.stream.StreamSource;

@GrpcService
@Blocking
public class CxfBridgeService extends CxfBridgeGrpc.CxfBridgeImplBase {
  static {
    System.setProperty("jakarta.xml.ws.spi.Provider", "org.apache.cxf.jaxws.spi.ProviderImpl");
  }

  private static final Logger LOG = Logger.getLogger(CxfBridgeService.class.getName());

  @Inject BridgeConfig bridgeConfig;

  @Inject CxfClientManager clientManager;

  @Inject CxfServerManager serverManager;

  @Inject SecurityProfileStore profileStore;

  @Override
  public void invoke(SoapRequest request, StreamObserver<SoapResponse> responseObserver) {
    try {
      String profileName = request.getSecurityProfile();
      if (profileName == null || profileName.isBlank()) {
        responseObserver.onError(
            Status.INVALID_ARGUMENT.withDescription("security_profile is required").asException());
        return;
      }

      // Resolve profile — wsdl/service/port come from profile or request overrides
      SecurityProfile profile = profileStore.getProfile(profileName);

      String wsdl = request.getWsdlPath().isBlank() ? profile.wsdlPath() : request.getWsdlPath();
      String service =
          request.getServiceName().isBlank() ? profile.serviceName() : request.getServiceName();
      String port = request.getPortName().isBlank() ? profile.portName() : request.getPortName();
      String address = request.getAddress().isBlank() ? profile.address() : request.getAddress();

      var dispatch =
          clientManager.getDispatch(
              wsdl, address, service, port, request.getOperation(), profileName);

      int timeout =
          request.getTimeoutMs() > 0 ? request.getTimeoutMs() : bridgeConfig.connectionTimeoutMs();
      dispatch
          .getRequestContext()
          .put("jakarta.xml.ws.client.connectionTimeout", String.valueOf(timeout));
      dispatch
          .getRequestContext()
          .put("jakarta.xml.ws.client.receiveTimeout", String.valueOf(timeout));

      String payload = request.getPayload().toStringUtf8();
      String soapVersion = request.getHeadersOrDefault("soap-version", "1.1");
      String envelope = SoapEnvelopeHelper.wrapInEnvelope(payload, soapVersion);

      Source responseSource;
      try {
        responseSource = dispatch.invoke(new StreamSource(new StringReader(envelope)));
      } catch (SOAPFaultException sfe) {
        // SOAP fault — surface to caller as a structured fault response.
        var fault = sfe.getFault();
        String code = fault != null && fault.getFaultCode() != null ? fault.getFaultCode() : "";
        String reason =
            fault != null && fault.getFaultString() != null ? fault.getFaultString() : sfe.getMessage();
        SoapResponse faultResp =
            SoapResponse.newBuilder()
                .setFault(true)
                .setFaultCode(code == null ? "" : code)
                .setFaultString(reason == null ? "" : reason)
                .build();
        LOG.log(
            Level.FINE,
            "SOAP invoke returned fault: {0}/{1}, code={2}",
            new Object[] {service, port, code});
        responseObserver.onNext(faultResp);
        responseObserver.onCompleted();
        return;
      }
      // Service.Mode.PAYLOAD: dispatch returns just the SOAP body content (no envelope wrapper).
      // The serialized source is the response payload directly — no envelope extraction needed.
      String responseBody = toXmlString(responseSource);
      boolean fault = false;

      SoapResponse.Builder builder =
          SoapResponse.newBuilder().setPayload(ByteString.copyFromUtf8(responseBody));

      LOG.log(
          Level.FINE,
          "SOAP invoke completed: {0}/{1}, fault={2}",
          new Object[] {service, port, fault});

      responseObserver.onNext(builder.build());
      responseObserver.onCompleted();
    } catch (IllegalArgumentException e) {
      // Unknown profile from profileStore.getProfile()
      LOG.log(Level.WARNING, "Profile not found: {0}", request.getSecurityProfile());
      responseObserver.onError(
          Status.NOT_FOUND
              .withDescription("profile not found: " + request.getSecurityProfile())
              .withCause(e)
              .asException());
    } catch (Throwable e) {
      if (e instanceof ExceptionInInitializerError eiie) {
        LOG.log(Level.SEVERE, "ExceptionInInitializerError cause: " + eiie.getException());
      }
      Throwable root = e;
      while (true) {
        Throwable next = null;
        if (root instanceof ExceptionInInitializerError eiie && eiie.getException() != null) {
          next = eiie.getException();
        } else if (root instanceof UndeclaredThrowableException ute
            && ute.getUndeclaredThrowable() != null) {
          next = ute.getUndeclaredThrowable();
        } else if (root.getCause() != null && root.getCause() != root) {
          next = root.getCause();
        }
        if (next == null || next == root) {
          break;
        }
        root = next;
      }
      String msg = root.getClass().getName() + ": " + root.getMessage();
      LOG.log(
          Level.SEVERE,
          "SOAP invoke failed for {0}/{1}",
          new Object[] {request.getServiceName(), request.getPortName()});
      LOG.log(Level.SEVERE, "Full exception: ", e);
      responseObserver.onError(Status.INTERNAL.withDescription(msg).withCause(e).asException());
    }
  }

  @Override
  public StreamObserver<ConsumerResponse> openConsumerStream(
      StreamObserver<ConsumerRequest> responseObserver) {
    serverManager.setConsumerRequestObserver(responseObserver);
    return new StreamObserver<>() {
      @Override
      public void onNext(ConsumerResponse value) {
        serverManager.completeFromConsumer(value);
      }

      @Override
      public void onError(Throwable t) {
        serverManager.cleanup();
      }

      @Override
      public void onCompleted() {
        serverManager.cleanup();
      }
    };
  }

  @Override
  public void health(HealthRequest request, StreamObserver<HealthResponse> responseObserver) {
    responseObserver.onNext(
        HealthResponse.newBuilder().setHealthy(true).setMessage("SERVING").build());
    responseObserver.onCompleted();
  }

  private static String toXmlString(Source source) throws Exception {
    java.io.ByteArrayOutputStream out = new java.io.ByteArrayOutputStream();
    javax.xml.transform.TransformerFactory.newInstance()
        .newTransformer()
        .transform(source, new javax.xml.transform.stream.StreamResult(out));
    return out.toString(java.nio.charset.StandardCharsets.UTF_8);
  }
}
