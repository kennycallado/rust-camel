package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.StringReader;
import javax.xml.transform.Source;
import javax.xml.transform.stream.StreamSource;
import org.junit.jupiter.api.Test;

class SoapEnvelopeHelperTest {

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
