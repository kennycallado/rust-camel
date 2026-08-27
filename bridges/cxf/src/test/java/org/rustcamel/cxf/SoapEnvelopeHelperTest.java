package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.StringReader;
import javax.xml.parsers.DocumentBuilder;
import javax.xml.parsers.DocumentBuilderFactory;
import javax.xml.parsers.ParserConfigurationException;
import javax.xml.transform.Source;
import javax.xml.transform.stream.StreamSource;
import org.junit.jupiter.api.Test;

class SoapEnvelopeHelperTest {

  @Test
  void configureSecureThrowsIllegalStateWhenFeatureUnsupported() {
    ParserConfigurationException original = new ParserConfigurationException("boom");
    DocumentBuilderFactory stub =
        new DocumentBuilderFactory() {
          @Override
          public void setFeature(String name, boolean value) throws ParserConfigurationException {
            throw original;
          }

          @Override
          public Object getAttribute(String name) {
            throw new UnsupportedOperationException();
          }

          @Override
          public void setAttribute(String name, Object value) {
            throw new UnsupportedOperationException();
          }

          @Override
          public DocumentBuilder newDocumentBuilder() {
            throw new UnsupportedOperationException();
          }

          @Override
          public boolean isNamespaceAware() {
            throw new UnsupportedOperationException();
          }

          @Override
          public boolean isValidating() {
            throw new UnsupportedOperationException();
          }

          @Override
          public boolean isXIncludeAware() {
            throw new UnsupportedOperationException();
          }

          @Override
          public boolean getFeature(String name) {
            throw new UnsupportedOperationException();
          }

          @Override
          public void setNamespaceAware(boolean awareness) {
            throw new UnsupportedOperationException();
          }

          @Override
          public void setValidating(boolean validating) {
            throw new UnsupportedOperationException();
          }

          @Override
          public void setXIncludeAware(boolean state) {
            throw new UnsupportedOperationException();
          }
        };

    IllegalStateException thrown =
        assertThrows(IllegalStateException.class, () -> SoapEnvelopeHelper.configureSecure(stub));

    assertSame(original, thrown.getCause());
  }

  @Test
  void secureDbfHasAllHardeningFeaturesEnabled() throws ParserConfigurationException {
    DocumentBuilderFactory dbf =
        SoapEnvelopeHelper.configureSecure(DocumentBuilderFactory.newInstance());

    assertTrue(dbf.getFeature("http://apache.org/xml/features/disallow-doctype-decl"));
    assertFalse(dbf.getFeature("http://xml.org/sax/features/external-general-entities"));
    assertFalse(dbf.getFeature("http://xml.org/sax/features/external-parameter-entities"));
    assertFalse(dbf.getFeature("http://apache.org/xml/features/nonvalidating/load-external-dtd"));
    assertFalse(dbf.isXIncludeAware());
    assertTrue(dbf.isNamespaceAware());
  }

  @Test
  void testWrapSoap11() throws Exception {
    String xmlBody = "<m:ping xmlns:m=\"urn:test\"><m:id>1</m:id></m:ping>";
    String envelope = SoapEnvelopeHelper.wrapInEnvelope(xmlBody, "1.1");
    assertTrue(envelope.contains("http://schemas.xmlsoap.org/soap/envelope/"));
  }

  @Test
  void testWrapSoap12() throws Exception {
    String xmlBody = "<m:ping xmlns:m=\"urn:test\"><m:id>1</m:id></m:ping>";
    String envelope = SoapEnvelopeHelper.wrapInEnvelope(xmlBody, "1.2");
    assertTrue(envelope.contains("http://www.w3.org/2003/05/soap-envelope"));
  }

  @Test
  void testWrapIncludesHeader() throws Exception {
    String xmlBody = "<m:ping xmlns:m=\"urn:test\"><m:id>1</m:id></m:ping>";
    String envelope = SoapEnvelopeHelper.wrapInEnvelope(xmlBody, "1.1");
    assertTrue(envelope.contains("Header/>"));
  }

  @Test
  void testExtractBody() throws Exception {
    String envelope =
        "<soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<soap:Header/><soap:Body><m:ping xmlns:m=\"urn:test\"><m:id>1</m:id></m:ping></soap:Body></soap:Envelope>";
    String extracted = SoapEnvelopeHelper.extractBody(new StreamSource(new StringReader(envelope)));
    assertTrue(extracted.contains("<m:ping"));
    assertTrue(extracted.contains("<m:id>1</m:id>"));
  }

  @Test
  void testIsFaultTrue() throws Exception {
    String faultEnvelope =
        "<soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<soap:Body><soap:Fault xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\"><faultcode>soap:Server</faultcode><faultstring>Internal Error</faultstring></soap:Fault></soap:Body></soap:Envelope>";
    Source source = new StreamSource(new StringReader(faultEnvelope));
    assertTrue(SoapEnvelopeHelper.isFault(source));
  }

  @Test
  void testIsFaultFalse() throws Exception {
    String normalEnvelope =
        "<soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<soap:Header/><soap:Body><m:ping xmlns:m=\"urn:test\"><m:id>1</m:id></m:ping></soap:Body></soap:Envelope>";
    assertFalse(SoapEnvelopeHelper.isFault(new StreamSource(new StringReader(normalEnvelope))));
  }

  @Test
  void testExtractFaultCodeSoap11() throws Exception {
    String faultEnvelope =
        "<soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<soap:Body><soap:Fault xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\"><faultcode>soap:Server</faultcode><faultstring>Internal Error</faultstring></soap:Fault></soap:Body></soap:Envelope>";
    assertEquals(
        "soap:Server",
        SoapEnvelopeHelper.extractFaultCode(new StreamSource(new StringReader(faultEnvelope))));
  }

  @Test
  void testExtractFaultStringSoap11() throws Exception {
    String faultEnvelope =
        "<soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\">"
            + "<soap:Body><soap:Fault xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\"><faultcode>soap:Server</faultcode><faultstring>Internal Error</faultstring></soap:Fault></soap:Body></soap:Envelope>";
    assertEquals(
        "Internal Error",
        SoapEnvelopeHelper.extractFaultString(new StreamSource(new StringReader(faultEnvelope))));
  }

  @Test
  void testExtractFaultCodeSoap12() throws Exception {
    String faultEnvelope =
        "<soap:Envelope xmlns:soap=\"http://www.w3.org/2003/05/soap-envelope\">"
            + "<soap:Body><soap:Fault xmlns:soap=\"http://www.w3.org/2003/05/soap-envelope\"><Code><Value>soap:Sender</Value></Code><Reason><Text>Bad request</Text></Reason></soap:Fault></soap:Body></soap:Envelope>";
    assertEquals(
        "soap:Sender",
        SoapEnvelopeHelper.extractFaultCode(new StreamSource(new StringReader(faultEnvelope))));
  }

  @Test
  void testExtractFaultStringSoap12() throws Exception {
    String faultEnvelope =
        "<soap:Envelope xmlns:soap=\"http://www.w3.org/2003/05/soap-envelope\">"
            + "<soap:Body><soap:Fault xmlns:soap=\"http://www.w3.org/2003/05/soap-envelope\"><Code><Value>soap:Sender</Value></Code><Reason><Text>Bad request</Text></Reason></soap:Fault></soap:Body></soap:Envelope>";
    assertEquals(
        "Bad request",
        SoapEnvelopeHelper.extractFaultString(new StreamSource(new StringReader(faultEnvelope))));
  }
}
