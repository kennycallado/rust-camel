package org.rustcamel.cxf;

import com.google.protobuf.ByteString;
import cxf_bridge.*;
import io.grpc.Status;
import io.grpc.stub.StreamObserver;
import io.quarkus.grpc.GrpcService;
import io.smallrye.common.annotation.Blocking;
import jakarta.annotation.PostConstruct;
import jakarta.inject.Inject;
import jakarta.xml.ws.Dispatch;
import jakarta.xml.ws.soap.SOAPBinding;
import jakarta.xml.ws.soap.SOAPFaultException;
import java.io.StringReader;
import java.lang.reflect.UndeclaredThrowableException;
import java.util.logging.Level;
import java.util.logging.Logger;
import javax.xml.namespace.QName;
import javax.xml.transform.Source;
import javax.xml.transform.stream.StreamSource;
import org.w3c.dom.Document;
import org.w3c.dom.Element;

@GrpcService
@Blocking
public class CxfBridgeService extends CxfBridgeGrpc.CxfBridgeImplBase {
  static {
    System.setProperty("jakarta.xml.ws.spi.Provider", "org.apache.cxf.jaxws.spi.ProviderImpl");
  }

  private static final Logger LOG = Logger.getLogger(CxfBridgeService.class.getName());

  private static final String SOAP11_BINDING_ID = SOAPBinding.SOAP11HTTP_BINDING;
  private static final String SOAP12_BINDING_ID = SOAPBinding.SOAP12HTTP_BINDING;

  private static final String SOAP11_NS = "http://schemas.xmlsoap.org/soap/envelope/";
  private static final String SOAP12_NS = "http://www.w3.org/2003/05/soap-envelope";

  /**
   * Static protocol-failure description for an operation that declares a WSDL output but whose
   * request-reply dispatch yielded no serializable SOAP content. Deliberately version-neutral so it
   * never leaks a fabricated payload or a fallback one-way interpretation.
   */
  private static final String EMPTY_SOAP_RESPONSE_DESCRIPTION =
      "empty SOAP response: the operation declares an output but the server returned no body";

  @Inject BridgeConfig bridgeConfig;

  @Inject CxfClientManager clientManager;

  @Inject CxfServerManager serverManager;

  @Inject SecurityProfileStore profileStore;

  /**
   * Test seam: pins the producer response-body cap deterministically. {@code -1} (production) lets
   * {@link #responseCapBytes()} resolve the cap from {@link BridgeConfig#parseMaxBodyBytes()} on
   * first use; a positive value wins over the environment when set before first resolution (in CDI
   * construction the {@code @PostConstruct} hook resolves eagerly, so late pins are ignored).
   */
  long pinnedMaxBodyBytes = -1;

  private long resolvedResponseCapBytes = -1;

  @PostConstruct
  void initResponseCap() {
    // Eager startup validation (ADR-0033): resolve and memoize the cap at post-construct.
    responseCapBytes();
  }

  private long responseCapBytes() {
    if (resolvedResponseCapBytes > 0) {
      return resolvedResponseCapBytes;
    }
    // Resolve once and memoize. A pin set after construction but before first invoke wins over the
    // environment, matching the publisher's per-request pin semantics.
    resolvedResponseCapBytes =
        pinnedMaxBodyBytes > 0 ? pinnedMaxBodyBytes : BridgeConfig.parseMaxBodyBytes();
    return resolvedResponseCapBytes;
  }

  @Override
  public void invoke(SoapRequest request, StreamObserver<SoapResponse> responseObserver) {
    try {
      String profileName = request.getSecurityProfile();
      if (profileName == null || profileName.isBlank()) {
        responseObserver.onError(
            Status.INVALID_ARGUMENT.withDescription("security_profile is required").asException());
        return;
      }

      // Resolve profile — wsdl/service/port come from profile or request overrides. The lookup is
      // the only path that maps to NOT_FOUND: a later IllegalArgumentException (for example an
      // unknown explicit operation) is an argument problem, not a missing profile.
      SecurityProfile profile;
      try {
        profile = profileStore.getProfile(profileName);
      } catch (IllegalArgumentException e) {
        LOG.log(Level.WARNING, "Profile not found: {0}", profileName);
        responseObserver.onError(
            Status.NOT_FOUND
                .withDescription("profile not found: " + profileName)
                .withCause(e)
                .asException());
        return;
      }

      String wsdl = request.getWsdlPath().isBlank() ? profile.wsdlPath() : request.getWsdlPath();
      String service =
          request.getServiceName().isBlank() ? profile.serviceName() : request.getServiceName();
      String port = request.getPortName().isBlank() ? profile.portName() : request.getPortName();
      String address = request.getAddress().isBlank() ? profile.address() : request.getAddress();

      // The wire field is uint64, but the dispatch timeout is a 32-bit bound.
      // Reject an out-of-range value before resolving the dispatch so a legacy
      // -1 (decoded as an unsigned max) or an oversized value fails as
      // INVALID_ARGUMENT instead of a bridge-side timeout.
      long rawTimeout = request.getTimeoutMs();
      if (rawTimeout < 0 || rawTimeout > 2147483647L) {
        responseObserver.onError(
            Status.INVALID_ARGUMENT
                .withDescription("timeout_ms must be within [0, 2147483647], got " + rawTimeout)
                .asException());
        return;
      }

      long normalizedTimeout = rawTimeout > 0 ? rawTimeout : bridgeConfig.connectionTimeoutMs();

      // Service.Mode.PAYLOAD expects the bare operation element. A payload that is already a SOAP
      // Envelope would be nested in a second envelope on the wire, so parse it and reject that
      // before any operation resolution or wire transmission. The parsed root QName is the input
      // element used to select an operation when the request omits an explicit name.
      String payload = request.getPayload().toStringUtf8();
      Document payloadDoc = SoapEnvelopeHelper.parseResponse(payload);
      if (isSoapEnvelopeRoot(payloadDoc)) {
        responseObserver.onError(
            Status.INVALID_ARGUMENT
                .withDescription("payload is already a SOAP Envelope")
                .asException());
        return;
      }
      QName inputQName = rootQName(payloadDoc);

      CxfClientManager.ResolvedDispatch resolved;
      try {
        resolved =
            clientManager.getResolvedDispatch(
                wsdl,
                address,
                service,
                port,
                request.getOperation(),
                profileName,
                normalizedTimeout,
                inputQName);
      } catch (WsdlOperationCatalog.InvalidOperationException e) {
        // No explicit name with an input that matches zero or multiple operations, or an explicit
        // name that did not resolve. Argument-class error, before any wire transmission.
        LOG.log(Level.WARNING, "Rejected invoke: {0}", e.getMessage());
        responseObserver.onError(
            Status.INVALID_ARGUMENT.withDescription(e.getMessage()).withCause(e).asException());
        return;
      }

      Dispatch<Source> dispatch = resolved.dispatch();

      // The SOAP version is a property of the selected WSDL port binding, never of a request
      // header. Reject a binding we cannot map to an envelope version instead of silently
      // defaulting to SOAP 1.1.
      String bindingId = dispatch.getBinding().getBindingID();
      if (!SOAP11_BINDING_ID.equals(bindingId) && !SOAP12_BINDING_ID.equals(bindingId)) {
        responseObserver.onError(
            Status.INVALID_ARGUMENT
                .withDescription("unsupported WSDL binding ID: " + bindingId)
                .asException());
        return;
      }

      if (resolved.operation().oneWay()) {
        // Input-only operation: send the bare payload exactly once. There is no response Source to
        // serialize or inspect. A local invocation failure propagates through the outer error
        // handling; it is never converted into a success or retried as request-reply.
        dispatch.invokeOneWay(new StreamSource(new StringReader(payload)));
        LOG.log(Level.FINE, "SOAP one-way completed: {0}/{1}", new Object[] {service, port});
        responseObserver.onNext(SoapResponse.getDefaultInstance());
        responseObserver.onCompleted();
        return;
      }

      Source responseSource;
      try {
        responseSource = dispatch.invoke(new StreamSource(new StringReader(payload)));
      } catch (SOAPFaultException sfe) {
        // SOAP fault — surface to caller as a structured fault response.
        var fault = sfe.getFault();
        String code = fault != null && fault.getFaultCode() != null ? fault.getFaultCode() : "";
        String reason =
            fault != null && fault.getFaultString() != null
                ? fault.getFaultString()
                : sfe.getMessage();
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
        // The cause chain (e.g. a WSSecurityException errorCode behind the
        // generic "Security processing failed." fault string) is otherwise
        // lost when the fault is flattened onto the wire — log it so native
        // WS-Security failures stay diagnosable. Never log secrets.
        if (sfe.getCause() != null) {
          LOG.log(Level.WARNING, "SOAP invoke fault cause", sfe.getCause());
        }
        responseObserver.onNext(faultResp);
        responseObserver.onCompleted();
        return;
      }
      // A request-reply operation that produced no Source is a protocol failure: there is no
      // response content to forward and nothing may be fabricated. Never fall back to one-way.
      if (responseSource == null) {
        responseObserver.onError(
            Status.INTERNAL.withDescription(EMPTY_SOAP_RESPONSE_DESCRIPTION).asRuntimeException());
        return;
      }
      // Service.Mode.PAYLOAD: dispatch returns just the SOAP body content (no envelope wrapper).
      // The serialized source is the response payload directly — no envelope extraction needed.
      String responseBody;
      try {
        responseBody = toXmlString(responseSource, responseCapBytes());
      } catch (ResponseCapExceededException e) {
        LOG.log(Level.WARNING, "SOAP invoke aborted: {0}", e.getMessage());
        responseObserver.onError(
            Status.RESOURCE_EXHAUSTED
                .withDescription(
                    "response body "
                        + e.observedBytes
                        + " bytes exceeds CXF_MAX_BODY_BYTES="
                        + e.capBytes)
                .asRuntimeException());
        return;
      }
      // An output-declaring operation whose response serializes to zero bytes is empty content,
      // not an implicit one-way acknowledgment.
      if (responseBody.isEmpty()) {
        responseObserver.onError(
            Status.INTERNAL.withDescription(EMPTY_SOAP_RESPONSE_DESCRIPTION).asRuntimeException());
        return;
      }
      boolean fault = false;

      SoapResponse.Builder builder =
          SoapResponse.newBuilder().setPayload(ByteString.copyFromUtf8(responseBody));

      LOG.log(
          Level.FINE,
          "SOAP invoke completed: {0}/{1}, fault={2}",
          new Object[] {service, port, fault});

      responseObserver.onNext(builder.build());
      responseObserver.onCompleted();
    } catch (CxfClientManager.UnknownOperationException e) {
      // Pre-wire rejection from dispatch creation: the requested operation is absent from the
      // selected WSDL port binding. Argument-class error retaining operation/binding context so
      // callers are not misled into treating it as a missing profile.
      LOG.log(Level.WARNING, "Rejected invoke: {0}", e.getMessage());
      responseObserver.onError(
          Status.INVALID_ARGUMENT.withDescription(e.getMessage()).withCause(e).asException());
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
        // Stream lifecycle only: clear this stream's liveness (by identity) and fail its
        // in-flight requests. The startup-published SOAP listener stays up (Task 2.7).
        serverManager.onConsumerStreamClosed(responseObserver);
      }

      @Override
      public void onCompleted() {
        serverManager.onConsumerStreamClosed(responseObserver);
      }
    };
  }

  @Override
  public void health(HealthRequest request, StreamObserver<HealthResponse> responseObserver) {
    responseObserver.onNext(
        HealthResponse.newBuilder().setHealthy(true).setMessage("SERVING").build());
    responseObserver.onCompleted();
  }

  /**
   * True when the parsed document root is a SOAP 1.1/1.2 {@code Envelope}. The producer rejects
   * such a pre-wrapped payload so it is never nested inside a second envelope on the wire.
   */
  private static boolean isSoapEnvelopeRoot(Document doc) {
    Element root = doc.getDocumentElement();
    if (root == null) {
      return false;
    }
    if (!"Envelope".equals(elementLocalName(root))) {
      return false;
    }
    String namespace = root.getNamespaceURI();
    return SOAP11_NS.equals(namespace) || SOAP12_NS.equals(namespace);
  }

  /** Namespace-qualified root element QName of a parsed bare payload. */
  private static QName rootQName(Document doc) {
    Element root = doc.getDocumentElement();
    if (root == null) {
      return null;
    }
    String namespace = root.getNamespaceURI();
    return new QName(namespace == null ? "" : namespace, elementLocalName(root));
  }

  private static String elementLocalName(Element element) {
    String localName = element.getLocalName();
    return localName == null ? element.getTagName() : localName;
  }

  static String toXmlString(Source source, long cap) throws Exception {
    BoundedOutputStream bounded = new BoundedOutputStream(cap);
    try {
      SecureTransformers.factory()
          .newTransformer()
          .transform(source, new javax.xml.transform.stream.StreamResult(bounded));
    } catch (Exception e) {
      throw unwrapResponseCap(e);
    }
    return bounded.toUtf8String();
  }

  /**
   * The transformer wraps sink failures in {@code TransformerException}s — walk the cause chain and
   * rethrow {@link ResponseCapExceededException} itself so the invoke() catch sees the cap breach
   * instead of a generic transformer failure.
   */
  private static Exception unwrapResponseCap(Exception e) throws Exception {
    if (e instanceof ResponseCapExceededException capEx) {
      throw capEx;
    }
    Throwable cause = e.getCause();
    while (cause != null) {
      if (cause instanceof ResponseCapExceededException capEx) {
        throw capEx;
      }
      if (cause.getCause() == cause) {
        break;
      }
      cause = cause.getCause();
    }
    return e;
  }

  /** Counts serialized bytes and refuses to buffer past the response cap. */
  private static final class BoundedOutputStream extends java.io.OutputStream {
    private final long capBytes;
    private final java.io.ByteArrayOutputStream out = new java.io.ByteArrayOutputStream();

    BoundedOutputStream(long capBytes) {
      this.capBytes = capBytes;
    }

    @Override
    public void write(int b) {
      enforceCap(1);
      out.write(b);
    }

    @Override
    public void write(byte[] b, int off, int len) {
      enforceCap(len);
      out.write(b, off, len);
    }

    private void enforceCap(int incoming) {
      long next = out.size() + incoming;
      if (next > capBytes) {
        throw new ResponseCapExceededException(next, capBytes);
      }
    }

    String toUtf8String() {
      return out.toString(java.nio.charset.StandardCharsets.UTF_8);
    }
  }

  /** The serialized producer response outgrew {@code CXF_MAX_BODY_BYTES}. */
  static final class ResponseCapExceededException extends RuntimeException {
    final long observedBytes;
    final long capBytes;

    ResponseCapExceededException(long observedBytes, long capBytes) {
      super("response body " + observedBytes + " bytes exceeds CXF_MAX_BODY_BYTES=" + capBytes);
      this.observedBytes = observedBytes;
      this.capBytes = capBytes;
    }
  }
}
