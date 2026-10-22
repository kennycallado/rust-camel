package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import cxf_bridge.SoapHeader;
import java.nio.charset.StandardCharsets;
import java.util.List;
import javax.xml.namespace.QName;
import javax.xml.parsers.DocumentBuilderFactory;
import javax.xml.transform.dom.DOMSource;
import org.apache.wss4j.dom.WSConstants;
import org.apache.wss4j.dom.WSDataRef;
import org.apache.wss4j.dom.engine.WSSecurityEngineResult;
import org.junit.jupiter.api.Test;
import org.w3c.dom.Document;
import org.w3c.dom.Element;

class SoapHeaderExporterTest {

  private static final String SOAP11_NS = "http://schemas.xmlsoap.org/soap/envelope/";
  private static final String SOAP12_NS = "http://www.w3.org/2003/05/soap-envelope";
  private static final String APP_NS = "http://tempuri.org/commonSchema";
  private static final String WSSE_NS =
      "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd";
  private static final String WSU_NS =
      "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd";

  @Test
  void exportsAllowListedChildWithNamespaceAndXml() throws Exception {
    Document doc =
        SoapEnvelopeHelper.parseResponse(
            """
            <soap:Envelope xmlns:soap="%s"
                           xmlns:rh="%s"
                           xmlns:oth="urn:other">
              <soap:Header>
                <rh:requestHeaderType>first</rh:requestHeaderType>
                <oth:other>second</oth:other>
              </soap:Header>
              <soap:Body><rh:payload/></soap:Body>
            </soap:Envelope>
            """
                .formatted(SOAP11_NS, APP_NS));
    Element first = (Element) doc.getElementsByTagNameNS(APP_NS, "requestHeaderType").item(0);

    List<SoapHeader> exported =
        SoapHeaderExporter.export(doc, List.of(new QName(APP_NS, "requestHeaderType")), List.of());

    assertEquals(1, exported.size());
    SoapHeader header = exported.get(0);
    assertEquals(APP_NS, header.getNs());
    assertEquals("requestHeaderType", header.getLocalName());
    assertEquals(SoapEnvelopeHelper.sourceToString(new DOMSource(first), true), header.getXml());
  }

  @Test
  void exportsInDocumentOrderRegardlessOfAllowListOrder() throws Exception {
    Document doc =
        SoapEnvelopeHelper.parseResponse(
            """
            <soap:Envelope xmlns:soap="%s"
                           xmlns:rh="%s"
                           xmlns:oth="urn:other">
              <soap:Header>
                <rh:requestHeaderType>first</rh:requestHeaderType>
                <oth:other>second</oth:other>
              </soap:Header>
              <soap:Body/>
            </soap:Envelope>
            """
                .formatted(SOAP11_NS, APP_NS));

    List<SoapHeader> exported =
        SoapHeaderExporter.export(
            doc,
            List.of(new QName("urn:other", "other"), new QName(APP_NS, "requestHeaderType")),
            List.of());

    assertEquals(2, exported.size());
    assertEquals("requestHeaderType", exported.get(0).getLocalName());
    assertEquals(APP_NS, exported.get(0).getNs());
    assertEquals("other", exported.get(1).getLocalName());
    assertEquals("urn:other", exported.get(1).getNs());
  }

  @Test
  void unlistedHeaderIsIgnored() throws Exception {
    Document doc =
        SoapEnvelopeHelper.parseResponse(
            """
            <soap:Envelope xmlns:soap="%s" xmlns:rh="%s" xmlns:oth="urn:other">
              <soap:Header>
                <rh:requestHeaderType>first</rh:requestHeaderType>
                <oth:other>second</oth:other>
              </soap:Header>
              <soap:Body/>
            </soap:Envelope>
            """
                .formatted(SOAP11_NS, APP_NS));

    List<SoapHeader> exported =
        SoapHeaderExporter.export(doc, List.of(new QName(APP_NS, "requestHeaderType")), List.of());

    assertEquals(1, exported.size());
    assertEquals("requestHeaderType", exported.get(0).getLocalName());
  }

  @Test
  void missingHeaderReturnsEmpty() throws Exception {
    Document doc =
        SoapEnvelopeHelper.parseResponse(
            """
            <soap:Envelope xmlns:soap="%s"><soap:Body/></soap:Envelope>
            """
                .formatted(SOAP11_NS));

    List<SoapHeader> exported =
        SoapHeaderExporter.export(doc, List.of(new QName(APP_NS, "requestHeaderType")), List.of());

    assertTrue(exported.isEmpty());
  }

  @Test
  void duplicateQnameThrows() throws Exception {
    Document doc =
        SoapEnvelopeHelper.parseResponse(
            """
            <soap:Envelope xmlns:soap="%s" xmlns:rh="%s">
              <soap:Header>
                <rh:requestHeaderType>one</rh:requestHeaderType>
                <rh:requestHeaderType>two</rh:requestHeaderType>
              </soap:Header>
              <soap:Body/>
            </soap:Envelope>
            """
                .formatted(SOAP11_NS, APP_NS));
    QName qName = new QName(APP_NS, "requestHeaderType");

    SoapHeaderExporter.SoapHeaderExportException thrown =
        assertThrows(
            SoapHeaderExporter.SoapHeaderExportException.class,
            () -> SoapHeaderExporter.export(doc, List.of(qName), List.of()));

    assertTrue(thrown.getMessage().contains(qName.toString()));
    assertFalse(thrown.getMessage().contains("<rh:"));
    assertFalse(thrown.getMessage().contains("one"));
  }

  @Test
  void reservedNamespaceIsSkippedDefensively() throws Exception {
    Document doc =
        SoapEnvelopeHelper.parseResponse(
            """
            <soap:Envelope xmlns:soap="%s" xmlns:wsse="%s">
              <soap:Header>
                <wsse:Security/>
              </soap:Header>
              <soap:Body/>
            </soap:Envelope>
            """
                .formatted(SOAP11_NS, WSSE_NS));

    List<SoapHeader> exported =
        SoapHeaderExporter.export(doc, List.of(new QName(WSSE_NS, "Security")), List.of());

    assertTrue(exported.isEmpty());
  }

  @Test
  void singleHeaderOverCapThrows() throws Exception {
    Document doc = newEnvelopeWithHeader();
    Element header = directHeader(doc);
    addHeaderWithSerializedLength(
        doc, header, APP_NS, "big", SoapHeaderExporter.MAX_HEADER_BYTES + 1);
    QName qName = new QName(APP_NS, "big");

    SoapHeaderExporter.SoapHeaderExportException thrown =
        assertThrows(
            SoapHeaderExporter.SoapHeaderExportException.class,
            () -> SoapHeaderExporter.export(doc, List.of(qName), List.of()));

    String message = thrown.getMessage();
    assertTrue(message.contains("16384") || message.contains("16,384"));
    assertTrue(message.contains(qName.toString()));
    assertFalse(message.contains("<rh:"));
  }

  @Test
  void multibyteCharactersMeasureBytes() throws Exception {
    Document doc = newEnvelopeWithHeader();
    Element padded = doc.createElementNS(APP_NS, "rh:multibyte");
    directHeader(doc).appendChild(padded);
    String text = "é".repeat(9000);
    padded.setTextContent(text);
    assertTrue(text.length() < SoapHeaderExporter.MAX_HEADER_BYTES);
    assertTrue(serializedBytes(padded) > SoapHeaderExporter.MAX_HEADER_BYTES);

    assertThrows(
        SoapHeaderExporter.SoapHeaderExportException.class,
        () -> SoapHeaderExporter.export(doc, List.of(new QName(APP_NS, "multibyte")), List.of()));
  }

  @Test
  void aggregateOverCapThrows() throws Exception {
    Document doc = newEnvelopeWithHeader();
    Element header = directHeader(doc);
    int perHeader = 14 * 1024;
    List<QName> allowList =
        List.of(
            addHeaderWithSerializedLength(doc, header, APP_NS, "h0", perHeader),
            addHeaderWithSerializedLength(doc, header, APP_NS, "h1", perHeader),
            addHeaderWithSerializedLength(doc, header, APP_NS, "h2", perHeader),
            addHeaderWithSerializedLength(doc, header, APP_NS, "h3", perHeader),
            addHeaderWithSerializedLength(doc, header, APP_NS, "h4", perHeader));

    assertThrows(
        SoapHeaderExporter.SoapHeaderExportException.class,
        () -> SoapHeaderExporter.export(doc, allowList, List.of()));
  }

  @Test
  void exactLimitsAreAccepted() throws Exception {
    Document single = newEnvelopeWithHeader();
    QName singleQName =
        addHeaderWithSerializedLength(
            single, directHeader(single), APP_NS, "single", SoapHeaderExporter.MAX_HEADER_BYTES);
    List<SoapHeader> singleOut = SoapHeaderExporter.export(single, List.of(singleQName), List.of());
    assertEquals(1, singleOut.size());
    assertEquals(
        SoapHeaderExporter.MAX_HEADER_BYTES,
        singleOut.get(0).getXml().getBytes(StandardCharsets.UTF_8).length);

    Document total = newEnvelopeWithHeader();
    Element totalHeader = directHeader(total);
    int quarter = SoapHeaderExporter.MAX_TOTAL_BYTES / 4;
    List<QName> allowList =
        List.of(
            addHeaderWithSerializedLength(total, totalHeader, APP_NS, "q0", quarter),
            addHeaderWithSerializedLength(total, totalHeader, APP_NS, "q1", quarter),
            addHeaderWithSerializedLength(total, totalHeader, APP_NS, "q2", quarter),
            addHeaderWithSerializedLength(total, totalHeader, APP_NS, "q3", quarter));

    List<SoapHeader> totalOut = SoapHeaderExporter.export(total, allowList, List.of());
    assertEquals(4, totalOut.size());
    int sum =
        totalOut.stream().mapToInt(h -> h.getXml().getBytes(StandardCharsets.UTF_8).length).sum();
    assertEquals(SoapHeaderExporter.MAX_TOTAL_BYTES, sum);
  }

  @Test
  void signedTrueOnlyForProtectedElement() throws Exception {
    Document doc = newEnvelopeWithHeader();
    Element header = directHeader(doc);
    Element signed = appendHeaderElement(doc, header, "signed");
    Element other = appendHeaderElement(doc, header, "other");
    WSDataRef signedRef = new WSDataRef();
    signedRef.setProtectedElement(signed);
    WSDataRef siblingRef = new WSDataRef();
    siblingRef.setProtectedElement(other);

    List<SoapHeader> exported =
        SoapHeaderExporter.export(
            doc,
            List.of(new QName(APP_NS, "signed"), new QName(APP_NS, "other")),
            List.of(signResult(signedRef)));

    assertEquals(2, exported.size());
    assertTrue(exported.get(0).getSigned());
    assertFalse(exported.get(1).getSigned());

    List<SoapHeader> siblingOnly =
        SoapHeaderExporter.export(
            doc, List.of(new QName(APP_NS, "signed")), List.of(signResult(siblingRef)));

    assertEquals(1, siblingOnly.size());
    assertFalse(siblingOnly.get(0).getSigned());
  }

  @Test
  void idOnlyReferenceDoesNotSetSigned() throws Exception {
    Document doc = newEnvelopeWithHeader();
    Element header = directHeader(doc);
    Element element = appendHeaderElement(doc, header, "identified");
    element.setAttributeNS(WSU_NS, "wsu:Id", "hdr-1");
    WSDataRef ref = new WSDataRef();
    ref.setWsuId("hdr-1");
    List<WSSecurityEngineResult> results = List.of(signResult(ref));

    List<SoapHeader> exported =
        SoapHeaderExporter.export(doc, List.of(new QName(APP_NS, "identified")), results);

    assertEquals(1, exported.size());
    assertFalse(exported.get(0).getSigned());
  }

  @Test
  void signedFalseForEncryptedOnlyResult() throws Exception {
    Document doc = newEnvelopeWithHeader();
    Element header = directHeader(doc);
    Element element = appendHeaderElement(doc, header, "encrypted");
    WSDataRef ref = new WSDataRef();
    ref.setProtectedElement(element);
    WSSecurityEngineResult encr = new WSSecurityEngineResult(WSConstants.ENCR);
    encr.put(WSSecurityEngineResult.TAG_DATA_REF_URIS, List.of(ref));

    List<SoapHeader> exported =
        SoapHeaderExporter.export(doc, List.of(new QName(APP_NS, "encrypted")), List.of(encr));

    assertEquals(1, exported.size());
    assertFalse(exported.get(0).getSigned());
  }

  private static WSSecurityEngineResult signResult(WSDataRef... refs) {
    WSSecurityEngineResult result = new WSSecurityEngineResult(WSConstants.SIGN);
    result.put(WSSecurityEngineResult.TAG_DATA_REF_URIS, List.of(refs));
    return result;
  }

  private static Document newEnvelopeWithHeader() throws Exception {
    DocumentBuilderFactory dbf = DocumentBuilderFactory.newInstance();
    dbf.setNamespaceAware(true);
    Document doc = dbf.newDocumentBuilder().newDocument();
    Element envelope = doc.createElementNS(SOAP11_NS, "soap:Envelope");
    doc.appendChild(envelope);
    envelope.appendChild(doc.createElementNS(SOAP11_NS, "soap:Header"));
    envelope.appendChild(doc.createElementNS(SOAP11_NS, "soap:Body"));
    return doc;
  }

  private static Element directHeader(Document doc) {
    return (Element) doc.getDocumentElement().getElementsByTagNameNS(SOAP11_NS, "Header").item(0);
  }

  private static Element appendHeaderElement(Document doc, Element header, String localName) {
    Element element = doc.createElementNS(APP_NS, "rh:" + localName);
    header.appendChild(element);
    return element;
  }

  private static QName addHeaderWithSerializedLength(
      Document doc, Element header, String ns, String localName, int targetBytes) throws Exception {
    Element element = doc.createElementNS(ns, "rh:" + localName);
    header.appendChild(element);
    element.setTextContent("x");
    int oneByteLength = serializedBytes(element);
    int padding = targetBytes - oneByteLength + 1;
    if (padding < 1) {
      throw new IllegalArgumentException(
          "target " + targetBytes + " below minimum for " + localName);
    }
    element.setTextContent("x".repeat(padding));
    int actual = serializedBytes(element);
    if (actual != targetBytes) {
      throw new IllegalStateException(
          "expected " + targetBytes + " bytes for " + localName + " but got " + actual);
    }
    return new QName(ns, localName);
  }

  private static int serializedBytes(Element element) throws Exception {
    return SoapEnvelopeHelper.sourceToString(new DOMSource(element), true)
        .getBytes(StandardCharsets.UTF_8)
        .length;
  }
}
