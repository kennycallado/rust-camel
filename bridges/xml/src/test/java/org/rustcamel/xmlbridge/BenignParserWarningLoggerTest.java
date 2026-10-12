package org.rustcamel.xmlbridge;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.PrintStream;
import javax.xml.XMLConstants;
import javax.xml.transform.sax.SAXSource;
import javax.xml.transform.stream.StreamResult;
import net.sf.saxon.Configuration;
import net.sf.saxon.TransformerFactoryImpl;
import net.sf.saxon.lib.Feature;
import net.sf.saxon.lib.Logger;
import org.junit.jupiter.api.Test;
import org.xml.sax.InputSource;

class BenignParserWarningLoggerTest {
  private static final String XERCES =
      "Selected XML parser org.apache.xerces.jaxp.SAXParserImpl$JAXPSAXParser";

  @Test
  void dropsBenignPropertyAndFeatureWarnings() {
    assertTrue(
        BenignParserWarningLogger.isBenign(
            XERCES
                + " does not recognize the property"
                + " http://javax.xml.XMLConstants/property/accessExternalDTD"));
    assertTrue(
        BenignParserWarningLogger.isBenign(
            XERCES + " does not recognize the property http://xml.org/sax/features/validation"));
    // Saxon's feature-loop branch uses "feature"; cover that wording too.
    assertTrue(
        BenignParserWarningLogger.isBenign(
            XERCES + " does not recognize the feature http://xml.org/sax/features/validation"));
  }

  @Test
  void keepsOtherWarnings() {
    assertFalse(
        BenignParserWarningLogger.isBenign(
            XERCES + " does not recognize the property http://example.org/other"));
    assertFalse(BenignParserWarningLogger.isBenign("some unrelated warning"));
    assertFalse(BenignParserWarningLogger.isBenign(null));
  }

  @Test
  void transformEmitsNoBenignWarningsButForwardsOthers() throws Exception {
    var err = new ByteArrayOutputStream();
    var logger = new BenignParserWarningLogger();
    logger.setPrintStream(new PrintStream(err, true));

    var config = new Configuration();
    config.setBooleanProperty(Feature.DTD_VALIDATION, false);
    config.setLogger(logger);
    var factory = new TransformerFactoryImpl(config);
    factory.setAttribute(XMLConstants.ACCESS_EXTERNAL_DTD, "");

    var spf = new org.apache.xerces.jaxp.SAXParserFactoryImpl();
    spf.setNamespaceAware(true);
    var xsl =
        "<xsl:stylesheet version='1.0' xmlns:xsl='http://www.w3.org/1999/XSL/Transform'>"
            + "<xsl:template match='/'><o/></xsl:template></xsl:stylesheet>";
    var templates =
        factory.newTemplates(
            new SAXSource(
                spf.newSAXParser().getXMLReader(),
                new InputSource(new ByteArrayInputStream(xsl.getBytes()))));
    templates
        .newTransformer()
        .transform(
            new SAXSource(
                spf.newSAXParser().getXMLReader(),
                new InputSource(new ByteArrayInputStream("<a/>".getBytes()))),
            new StreamResult(new ByteArrayOutputStream()));

    assertFalse(err.toString().contains("does not recognize"), err.toString());

    logger.println("unrelated warning", Logger.WARNING);
    assertTrue(err.toString().contains("unrelated warning"));
  }
}
