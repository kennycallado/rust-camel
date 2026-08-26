package org.rustcamel.cxf;

import javax.xml.XMLConstants;
import javax.xml.transform.TransformerConfigurationException;
import javax.xml.transform.TransformerFactory;

final class SecureTransformers {
  private SecureTransformers() {}

  static TransformerFactory factory() {
    TransformerFactory factory = TransformerFactory.newInstance();
    try {
      factory.setFeature(XMLConstants.FEATURE_SECURE_PROCESSING, true);
      factory.setAttribute(XMLConstants.ACCESS_EXTERNAL_DTD, "");
      factory.setAttribute(XMLConstants.ACCESS_EXTERNAL_STYLESHEET, "");
    } catch (TransformerConfigurationException e) {
      throw new IllegalStateException("Unable to harden TransformerFactory", e);
    }
    return factory;
  }
}
