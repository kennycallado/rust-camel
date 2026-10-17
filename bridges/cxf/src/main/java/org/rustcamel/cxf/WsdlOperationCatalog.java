package org.rustcamel.cxf;

import jakarta.xml.ws.Dispatch;
import jakarta.xml.ws.Service;
import java.io.File;
import java.util.ArrayList;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Objects;
import java.util.Set;
import javax.xml.namespace.QName;
import javax.xml.transform.Source;
import org.apache.cxf.binding.soap.model.SoapOperationInfo;
import org.apache.cxf.jaxws.DispatchImpl;
import org.apache.cxf.service.model.BindingInfo;
import org.apache.cxf.service.model.BindingMessageInfo;
import org.apache.cxf.service.model.BindingOperationInfo;
import org.apache.cxf.service.model.MessagePartInfo;

/**
 * Immutable snapshot of the operations declared by a selected WSDL binding.
 *
 * <p>The catalog copies the binding metadata it needs at construction time; it never retains the
 * mutable CXF model collections. Operation shape is the document/literal wrapper element QName from
 * the input message part, not the operation local name and not an unwrapped scalar parameter name.
 */
final class WsdlOperationCatalog {

  private final List<Operation> operations;

  private WsdlOperationCatalog(List<Operation> operations) {
    this.operations = List.copyOf(operations);
  }

  /**
   * Creates a temporary PAYLOAD dispatch for the configured WSDL/service/port. The catalog loader
   * only observes the selected binding metadata; it never invokes the dispatch.
   */
  @FunctionalInterface
  interface TemporaryDispatchFactory {
    Dispatch<Source> create(String wsdl, String service, String port) throws Exception;
  }

  /**
   * Loads the operation catalog for the configured WSDL/service/port without issuing any remote
   * invocation: a temporary PAYLOAD CXF dispatch is created purely to read the selected binding's
   * metadata and is closed on both success and failure. Configuration paths only — no exchange data
   * selects the WSDL, service, or port.
   */
  static WsdlOperationCatalog load(String wsdl, String service, String port) throws Exception {
    return load(wsdl, service, port, WsdlOperationCatalog::createTemporaryDispatch);
  }

  static WsdlOperationCatalog load(
      String wsdl, String service, String port, TemporaryDispatchFactory factory) throws Exception {
    Objects.requireNonNull(factory, "factory");
    Dispatch<Source> temporary = null;
    try {
      temporary = factory.create(wsdl, service, port);
      return fromBinding(selectedBinding(temporary));
    } finally {
      closeTemporary(temporary);
    }
  }

  private static Dispatch<Source> createTemporaryDispatch(String wsdl, String service, String port)
      throws Exception {
    QName serviceQName = service.startsWith("{") ? QName.valueOf(service) : new QName(service);
    QName portQName = port.startsWith("{") ? QName.valueOf(port) : new QName(port);
    Service jaxwsService = Service.create(new File(wsdl).toURI().toURL(), serviceQName);
    return jaxwsService.createDispatch(portQName, Source.class, Service.Mode.PAYLOAD);
  }

  private static BindingInfo selectedBinding(Dispatch<Source> dispatch) {
    if (!(dispatch instanceof DispatchImpl<Source> cxfDispatch)) {
      throw new IllegalStateException("Operation resolution requires a CXF dispatch");
    }
    return cxfDispatch.getClient().getEndpoint().getEndpointInfo().getBinding();
  }

  private static void closeTemporary(Dispatch<Source> dispatch) {
    if (dispatch instanceof DispatchImpl<Source> cxfDispatch) {
      try {
        cxfDispatch.close();
      } catch (Exception e) {
        // Closing the temporary metadata dispatch must not mask the load outcome.
      }
    }
  }

  /** Copies the selected binding's operation metadata into an immutable catalog. */
  static WsdlOperationCatalog fromBinding(BindingInfo binding) {
    Objects.requireNonNull(binding, "binding");
    List<Operation> operations = new ArrayList<>();
    for (BindingOperationInfo bindingOperation : binding.getOperations()) {
      BindingMessageInfo input = bindingOperation.getInput();
      boolean oneWay = input != null && bindingOperation.getOutput() == null;
      Set<QName> inputQNames = new LinkedHashSet<>();
      if (input != null && input.getMessageInfo() != null) {
        for (MessagePartInfo part : input.getMessageInfo().getMessageParts()) {
          QName element = part.getElementQName();
          if (element != null) {
            inputQNames.add(element);
          }
        }
      }
      SoapOperationInfo soapOperation = bindingOperation.getExtensor(SoapOperationInfo.class);
      String action = soapOperation == null ? null : soapOperation.getAction();
      operations.add(new Operation(bindingOperation.getName(), inputQNames, action, oneWay));
    }
    return new WsdlOperationCatalog(operations);
  }

  /**
   * Resolves an explicit producer operation name, or, when the name is absent or blank, the input
   * payload root QName. Unknown, ambiguous, or missing selections throw {@link
   * InvalidOperationException}.
   */
  Operation resolveProducer(String operation, QName inputQName) {
    if (operation != null && !operation.isBlank()) {
      List<Operation> matches = new ArrayList<>();
      for (Operation candidate : operations) {
        if (candidate.name().getLocalPart().equals(operation)) {
          matches.add(candidate);
        }
      }
      if (matches.size() != 1) {
        throw new InvalidOperationException(
            "Producer operation '"
                + operation
                + "' matched "
                + matches.size()
                + " operations in the selected binding; exactly one is required");
      }
      return matches.get(0);
    }
    if (inputQName == null) {
      throw new InvalidOperationException(
          "No producer operation name and no input QName were supplied");
    }
    return resolveUniqueInput(inputQName);
  }

  /**
   * Resolves a consumer input by unique qualified body QName, then requires a supplied SOAPAction
   * to agree with the declared action. A {@code null} action means no action was supplied.
   */
  Operation resolveConsumer(QName inputQName, String soapAction) {
    Operation resolved = resolveUniqueInput(inputQName);
    String supplied = normalizeSoapAction(soapAction);
    if (supplied != null && !supplied.equals(resolved.soapAction())) {
      throw new InvalidOperationException(
          "SOAPAction '"
              + soapAction
              + "' does not agree with declared action '"
              + resolved.soapAction()
              + "' for input "
              + inputQName);
    }
    return resolved;
  }

  private Operation resolveUniqueInput(QName inputQName) {
    if (inputQName == null) {
      throw new InvalidOperationException("An input QName is required to resolve an operation");
    }
    List<Operation> matches = new ArrayList<>();
    for (Operation candidate : operations) {
      if (candidate.inputQNames().contains(inputQName)) {
        matches.add(candidate);
      }
    }
    if (matches.size() != 1) {
      throw new InvalidOperationException(
          "Input QName "
              + inputQName
              + " matched "
              + matches.size()
              + " operations in the selected binding; exactly one is required");
    }
    return matches.get(0);
  }

  /**
   * Normalizes a SOAPAction value. {@code null} is preserved as absence; otherwise the value is
   * trimmed and a single matching outer quote pair is removed.
   */
  static String normalizeSoapAction(String raw) {
    if (raw == null) {
      return null;
    }
    String trimmed = raw.trim();
    if (trimmed.length() >= 2) {
      char first = trimmed.charAt(0);
      char last = trimmed.charAt(trimmed.length() - 1);
      if (first == last && (first == '"' || first == '\'')) {
        return trimmed.substring(1, trimmed.length() - 1);
      }
    }
    return trimmed;
  }

  /** Immutable operation metadata; a missing declared action is stored as the empty string. */
  record Operation(QName name, Set<QName> inputQNames, String soapAction, boolean oneWay) {
    Operation {
      Objects.requireNonNull(name, "name");
      inputQNames = Set.copyOf(Objects.requireNonNull(inputQNames, "inputQNames"));
      String normalized = normalizeSoapAction(soapAction);
      soapAction = normalized == null ? "" : normalized;
    }
  }

  /** Signals an unknown, ambiguous, missing, or conflicting operation selection. */
  static final class InvalidOperationException extends IllegalArgumentException {
    InvalidOperationException(String message) {
      super(message);
    }
  }
}
