package org.rustcamel.cxf;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import javax.xml.parsers.DocumentBuilder;
import javax.xml.parsers.DocumentBuilderFactory;
import javax.xml.parsers.ParserConfigurationException;
import javax.xml.transform.OutputKeys;
import javax.xml.transform.Source;
import javax.xml.transform.Transformer;
import javax.xml.transform.TransformerFactory;
import javax.xml.transform.dom.DOMSource;
import javax.xml.transform.stream.StreamResult;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.Node;
import org.w3c.dom.NodeList;

public final class SoapEnvelopeHelper {
  private static final String SOAP_NS_11 = "http://schemas.xmlsoap.org/soap/envelope/";
  private static final String SOAP_NS_12 = "http://www.w3.org/2003/05/soap-envelope";
  private static final ThreadLocal<DocumentBuilderFactory> SECURE_DBF =
      ThreadLocal.withInitial(
          () -> {
            DocumentBuilderFactory dbf = DocumentBuilderFactory.newInstance();
            dbf.setNamespaceAware(true);
            try {
              dbf.setFeature("http://apache.org/xml/features/disallow-doctype-decl", true);
              dbf.setFeature("http://xml.org/sax/features/external-general-entities", false);
              dbf.setFeature("http://xml.org/sax/features/external-parameter-entities", false);
              dbf.setFeature(
                  "http://apache.org/xml/features/nonvalidating/load-external-dtd", false);
              dbf.setXIncludeAware(false);
            } catch (ParserConfigurationException ignored) {
            }
            return dbf;
          });

  private SoapEnvelopeHelper() {}

  /** Parses a response XML string into a DOM Document for reuse across extraction methods. */
  public static Document parseResponse(String xml) throws Exception {
    DocumentBuilderFactory dbf = SECURE_DBF.get();
    DocumentBuilder db = dbf.newDocumentBuilder();
    return db.parse(
        new ByteArrayInputStream(xml.getBytes(java.nio.charset.StandardCharsets.UTF_8)));
  }

  public static String wrapInEnvelope(String xmlBody, String soapVersion) throws Exception {
    String ns = "1.2".equals(soapVersion) ? SOAP_NS_12 : SOAP_NS_11;

    DocumentBuilderFactory dbf = SECURE_DBF.get();
    DocumentBuilder db = dbf.newDocumentBuilder();

    Document envelopeDoc = db.newDocument();
    Element envelope = envelopeDoc.createElementNS(ns, "soapenv:Envelope");
    Element header = envelopeDoc.createElementNS(ns, "soapenv:Header");
    Element body = envelopeDoc.createElementNS(ns, "soapenv:Body");
    envelopeDoc.appendChild(envelope);
    envelope.appendChild(header);
    envelope.appendChild(body);

    Document payloadDoc =
        db.parse(
            new ByteArrayInputStream(xmlBody.getBytes(java.nio.charset.StandardCharsets.UTF_8)));
    Node imported = envelopeDoc.importNode(payloadDoc.getDocumentElement(), true);
    body.appendChild(imported);

    return sourceToString(new DOMSource(envelopeDoc), false);
  }

  public static String extractBody(Source source) throws Exception {
    Document doc = parseSource(source);
    Element body = findElement(doc, "Body");
    if (body == null) {
      return "";
    }
    Element firstElement = firstElementChild(body);
    if (firstElement == null) {
      return "";
    }
    return sourceToString(new DOMSource(firstElement), true);
  }

  public static boolean isFault(Source source) throws Exception {
    return isFault(parseSource(source));
  }

  public static String extractFaultCode(Source source) throws Exception {
    return extractFaultCode(parseSource(source));
  }

  public static String extractFaultString(Source source) throws Exception {
    return extractFaultString(parseSource(source));
  }

  public static String extractBody(Document doc) throws Exception {
    Element body = findElement(doc, "Body");
    if (body == null) {
      return "";
    }
    Element firstElement = firstElementChild(body);
    if (firstElement == null) {
      return "";
    }
    return sourceToString(new DOMSource(firstElement), true);
  }

  public static boolean isFault(Document doc) {
    return findElement(doc, "Fault") != null;
  }

  public static String extractFaultCode(Document doc) {
    Element fault = findElement(doc, "Fault");
    if (fault == null) {
      return "";
    }

    Element code11 = findChildByLocalName(fault, "faultcode");
    if (code11 != null) {
      return code11.getTextContent();
    }

    Element code12 = findChildByLocalName(fault, "Code");
    if (code12 != null) {
      Element value = findChildByLocalName(code12, "Value");
      if (value != null) {
        return value.getTextContent();
      }
    }

    return "";
  }

  public static String extractFaultString(Document doc) {
    Element fault = findElement(doc, "Fault");
    if (fault == null) {
      return "";
    }

    Element str11 = findChildByLocalName(fault, "faultstring");
    if (str11 != null) {
      return str11.getTextContent();
    }

    Element reason12 = findChildByLocalName(fault, "Reason");
    if (reason12 != null) {
      Element text = findChildByLocalName(reason12, "Text");
      if (text != null) {
        return text.getTextContent();
      }
    }

    return "";
  }

  /** Converts a Source to String omitting the XML declaration. */
  public static String sourceToString(Source source) throws Exception {
    return sourceToString(source, true);
  }

  private static Document parseSource(Source source) throws Exception {
    byte[] bytes = sourceToBytes(source);
    DocumentBuilderFactory dbf = SECURE_DBF.get();
    DocumentBuilder db = dbf.newDocumentBuilder();
    return db.parse(new ByteArrayInputStream(bytes));
  }

  private static byte[] sourceToBytes(Source source) throws Exception {
    ByteArrayOutputStream out = new ByteArrayOutputStream();
    Transformer transformer = TransformerFactory.newInstance().newTransformer();
    transformer.transform(source, new StreamResult(out));
    return out.toByteArray();
  }

  public static String sourceToString(Source source, boolean omitDecl) throws Exception {
    ByteArrayOutputStream out = new ByteArrayOutputStream();
    Transformer transformer = TransformerFactory.newInstance().newTransformer();
    transformer.setOutputProperty(OutputKeys.OMIT_XML_DECLARATION, omitDecl ? "yes" : "no");
    transformer.setOutputProperty(OutputKeys.INDENT, "no");
    transformer.transform(source, new StreamResult(out));
    return out.toString(java.nio.charset.StandardCharsets.UTF_8);
  }

  private static Element findElement(Document doc, String localName) {
    NodeList list11 = doc.getElementsByTagNameNS(SOAP_NS_11, localName);
    if (list11.getLength() > 0 && list11.item(0) instanceof Element e) {
      return e;
    }
    NodeList list12 = doc.getElementsByTagNameNS(SOAP_NS_12, localName);
    if (list12.getLength() > 0 && list12.item(0) instanceof Element e) {
      return e;
    }
    return null;
  }

  private static Element firstElementChild(Element parent) {
    NodeList children = parent.getChildNodes();
    for (int i = 0; i < children.getLength(); i++) {
      Node node = children.item(i);
      if (node instanceof Element element) {
        return element;
      }
    }
    return null;
  }

  private static Element findChildByLocalName(Element parent, String localName) {
    NodeList children = parent.getChildNodes();
    for (int i = 0; i < children.getLength(); i++) {
      Node node = children.item(i);
      if (node instanceof Element element) {
        String ln = element.getLocalName();
        if (ln == null) {
          ln = element.getTagName();
        }
        if (localName.equals(ln)) {
          return element;
        }
      }
    }
    return null;
  }
}
