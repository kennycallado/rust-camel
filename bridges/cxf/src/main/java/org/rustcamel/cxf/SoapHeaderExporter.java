package org.rustcamel.cxf;

import cxf_bridge.SoapHeader;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import javax.xml.namespace.QName;
import javax.xml.transform.dom.DOMSource;
import org.apache.wss4j.dom.WSConstants;
import org.apache.wss4j.dom.WSDataRef;
import org.apache.wss4j.dom.engine.WSSecurityEngineResult;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.Node;
import org.w3c.dom.NodeList;

/**
 * Extracts allow-listed application SOAP headers from an already verified envelope. Header values
 * are serialized and bounded; reserved WS-Security and SOAP namespaces are never exported.
 */
final class SoapHeaderExporter {
  static final int MAX_HEADERS = 16;
  static final int MAX_HEADER_BYTES = 16 * 1024;
  static final int MAX_TOTAL_BYTES = 64 * 1024;

  static final Set<String> RESERVED_NAMESPACES =
      Set.of(
          "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd",
          "http://docs.oasis-open.org/wss/oasis-wss-wssecurity-secext-1.1.xsd",
          "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd",
          "http://www.w3.org/2000/09/xmldsig#",
          "http://www.w3.org/2001/04/xmlenc#",
          "http://schemas.xmlsoap.org/soap/envelope/",
          "http://www.w3.org/2003/05/soap-envelope");

  private static final String SOAP_NS_11 = "http://schemas.xmlsoap.org/soap/envelope/";
  private static final String SOAP_NS_12 = "http://www.w3.org/2003/05/soap-envelope";

  private SoapHeaderExporter() {}

  /**
   * True when {@code namespace} is a WS-Security or SOAP envelope namespace that is never exported.
   */
  static boolean isReservedNamespace(String namespace) {
    return RESERVED_NAMESPACES.contains(namespace);
  }

  static final class SoapHeaderExportException extends Exception {
    SoapHeaderExportException(String detail) {
      super(detail);
    }
  }

  static List<SoapHeader> export(
      Document doc, List<QName> allowList, List<WSSecurityEngineResult> results) throws Exception {
    if (allowList == null || allowList.isEmpty()) {
      return List.of();
    }
    Element header = directHeaderElement(doc);
    if (header == null) {
      return List.of();
    }

    List<SoapHeader> exported = new ArrayList<>();
    Set<QName> seen = new HashSet<>();
    int totalBytes = 0;
    NodeList children = header.getChildNodes();
    for (int i = 0; i < children.getLength(); i++) {
      if (!(children.item(i) instanceof Element element)) {
        continue;
      }
      String namespaceUri = element.getNamespaceURI();
      if (namespaceUri == null) {
        namespaceUri = "";
      }
      String localName = localName(element);
      QName qName = new QName(namespaceUri, localName);
      if (!allowList.contains(qName)) {
        continue;
      }
      if (isReservedNamespace(namespaceUri)) {
        continue;
      }
      if (!seen.add(qName)) {
        throw new SoapHeaderExportException("duplicate allow-listed header: " + qName);
      }

      String xml = SoapEnvelopeHelper.sourceToString(new DOMSource(element), true);
      int bytes = xml.getBytes(StandardCharsets.UTF_8).length;
      if (bytes > MAX_HEADER_BYTES) {
        throw new SoapHeaderExportException(
            "header "
                + qName
                + " exceeds single-header cap of "
                + MAX_HEADER_BYTES
                + " bytes: "
                + bytes);
      }
      totalBytes += bytes;
      if (totalBytes > MAX_TOTAL_BYTES) {
        throw new SoapHeaderExportException(
            "header "
                + qName
                + " exceeds total cap of "
                + MAX_TOTAL_BYTES
                + " bytes: "
                + totalBytes);
      }

      exported.add(
          SoapHeader.newBuilder()
              .setNs(namespaceUri)
              .setLocalName(localName)
              .setXml(xml)
              .setSigned(coveredBySignResult(element, results))
              .build());
    }
    return exported.isEmpty() ? List.of() : List.copyOf(exported);
  }

  private static Element directHeaderElement(Document doc) {
    if (doc == null) {
      return null;
    }
    Element root = doc.getDocumentElement();
    if (root == null) {
      return null;
    }
    String rootNamespace = root.getNamespaceURI();
    if (!SOAP_NS_11.equals(rootNamespace) && !SOAP_NS_12.equals(rootNamespace)) {
      return null;
    }
    if (!"Envelope".equals(localName(root))) {
      return null;
    }
    NodeList children = root.getChildNodes();
    for (int i = 0; i < children.getLength(); i++) {
      Node node = children.item(i);
      if (node instanceof Element candidate
          && "Header".equals(localName(candidate))
          && rootNamespace.equals(candidate.getNamespaceURI())) {
        return candidate;
      }
    }
    return null;
  }

  private static String localName(Element element) {
    String localName = element.getLocalName();
    return localName == null ? element.getTagName() : localName;
  }

  private static boolean coveredBySignResult(
      Element element, List<WSSecurityEngineResult> results) {
    if (results == null || results.isEmpty()) {
      return false;
    }
    for (WSSecurityEngineResult result : results) {
      if (result == null) {
        continue;
      }
      Object action = result.get(WSSecurityEngineResult.TAG_ACTION);
      if (!(action instanceof Integer actionCode) || actionCode != WSConstants.SIGN) {
        continue;
      }
      Object refsObj = result.get(WSSecurityEngineResult.TAG_DATA_REF_URIS);
      if (!(refsObj instanceof List<?> refs)) {
        continue;
      }
      for (Object refObj : refs) {
        if (!(refObj instanceof WSDataRef ref)) {
          continue;
        }
        Element protectedElement = ref.getProtectedElement();
        if (protectedElement != null && protectedElement.isSameNode(element)) {
          return true;
        }
      }
    }
    return false;
  }
}
