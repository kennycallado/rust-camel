package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.charset.StandardCharsets;
import javax.xml.XMLConstants;
import javax.xml.transform.TransformerFactory;
import javax.xml.transform.dom.DOMSource;
import org.junit.jupiter.api.Test;
import org.w3c.dom.Document;

class SecureTransformersTest {
  private static final String EXPECTED =
      "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>"
          + "<soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\">"
          + "<soap:Body><ping/></soap:Body></soap:Envelope>";

  @Test
  void factoryReportsHardenedAttributes() {
    TransformerFactory factory = SecureTransformers.factory();

    assertTrue(factory.getFeature(XMLConstants.FEATURE_SECURE_PROCESSING));
    assertEquals("", factory.getAttribute(XMLConstants.ACCESS_EXTERNAL_DTD));
    assertEquals("", factory.getAttribute(XMLConstants.ACCESS_EXTERNAL_STYLESHEET));
  }

  @Test
  void serializationUnchangedAtAllThreeSites() throws Exception {
    Document document =
        SoapEnvelopeHelper.parseResponse(
            "<soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\">"
                + "<soap:Body><ping/></soap:Body></soap:Envelope>");
    DOMSource source = new DOMSource(document);

    assertEquals(EXPECTED, SoapEnvelopeHelper.sourceToString(source, false));
    assertEquals(
        EXPECTED,
        new String(
            SoapEnvelopeHelper.sourceToBytes(new DOMSource(document)), StandardCharsets.UTF_8));
    assertEquals(EXPECTED, CxfBridgeService.toXmlString(new DOMSource(document)));
  }
}
