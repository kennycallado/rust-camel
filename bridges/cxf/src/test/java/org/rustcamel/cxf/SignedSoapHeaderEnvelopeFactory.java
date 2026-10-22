package org.rustcamel.cxf;

import java.nio.file.Path;
import java.util.List;
import javax.xml.transform.dom.DOMSource;
import org.apache.wss4j.common.WSEncryptionPart;
import org.apache.wss4j.common.crypto.Crypto;
import org.apache.wss4j.common.crypto.CryptoFactory;
import org.apache.wss4j.dom.WSConstants;
import org.apache.wss4j.dom.message.WSSecHeader;
import org.apache.wss4j.dom.message.WSSecSignature;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.NodeList;

/**
 * Builds SOAP envelopes signed over the Body and, optionally, one caller-supplied application
 * header. Test-only: no production code path uses it.
 *
 * <p>The header is inserted as the first child of the envelope's own SOAP Header (creating it when
 * absent) with {@code wsu:Id="hdr-1"}. When {@code signHeader} is true the signature also covers
 * that element through an ID-based {@link WSEncryptionPart}; when false the header travels
 * unsigned, which is the shape of the committed Rust fixture.
 */
final class SignedSoapHeaderEnvelopeFactory {

  private static final String WSU_NS =
      "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd";
  private static final String XMLNS_NS = "http://www.w3.org/2000/xmlns/";
  private static final String HEADER_ID = "hdr-1";

  private SignedSoapHeaderEnvelopeFactory() {}

  /**
   * Signs {@code soapXml} over the Body Content and, when {@code signHeader} is true, over the
   * application header identified by {@code hdr-1}. The BinarySecurityToken direct reference
   * carries the leaf certificate, mirroring {@link ExpiredSignedEnvelopeFactory}.
   */
  static String signSoap11(
      String soapXml,
      Path keystore,
      String alias,
      String signatureAlgorithm,
      String digestAlgorithm,
      String headerXml,
      boolean signHeader)
      throws Exception {
    Document doc = SoapEnvelopeHelper.parseResponse(soapXml);
    Element root = doc.getDocumentElement();
    Element header = directHeader(root);
    if (header == null) {
      String namespace = root.getNamespaceURI();
      String prefix = root.getPrefix();
      String qualifiedName = prefix == null || prefix.isEmpty() ? "Header" : prefix + ":Header";
      header = doc.createElementNS(namespace, qualifiedName);
      Element body = SoapEnvelopeHelper.directBodyElement(doc);
      if (body != null) {
        root.insertBefore(header, body);
      } else {
        root.appendChild(header);
      }
    }

    Document headerDoc = SoapEnvelopeHelper.parseResponse(headerXml);
    Element applicationHeader = (Element) doc.importNode(headerDoc.getDocumentElement(), true);
    // Declare the utility prefix on the header itself so the wsu:Id attribute serializes with a
    // bound namespace even before WSS4J writes its own Security header.
    applicationHeader.setAttributeNS(XMLNS_NS, "xmlns:wsu", WSU_NS);
    applicationHeader.setAttributeNS(WSU_NS, "wsu:Id", HEADER_ID);
    header.insertBefore(applicationHeader, header.getFirstChild());

    WSSecHeader secHeader = new WSSecHeader(doc);
    secHeader.insertSecurityHeader();

    Crypto crypto =
        CryptoFactory.getInstance(
            SecurityProfile.createCryptoProperties(
                keystore.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD));

    WSSecSignature sign = new WSSecSignature(secHeader);
    sign.setUserInfo(alias, TestKeystoreHelper.KEYSTORE_PASSWORD);
    sign.setSignatureAlgorithm(signatureAlgorithm);
    sign.setDigestAlgo(digestAlgorithm);
    sign.setSigCanonicalization(WSConstants.C14N_EXCL_OMIT_COMMENTS);

    List<WSEncryptionPart> parts = sign.getParts();
    parts.add(new WSEncryptionPart("Body", root.getNamespaceURI(), "Content"));
    if (signHeader) {
      // WSS4J resolves an ID-based part in Element mode through the document's wsu:Id.
      parts.add(new WSEncryptionPart(HEADER_ID, "Element"));
    }

    sign.setKeyIdentifierType(WSConstants.BST_DIRECT_REFERENCE);
    sign.setUseSingleCertificate(true);

    sign.prepare(crypto);
    // The BST is materialized only after prepare(); a direct BST reference cannot resolve before
    // the element is in the security header.
    sign.prependBSTElementToHeader();
    sign.computeSignature(sign.addReferencesToSign(sign.getParts()));

    return SoapEnvelopeHelper.sourceToString(new DOMSource(doc));
  }

  /** The first direct SOAP {@code Header} child of the envelope root, in its namespace. */
  private static Element directHeader(Element root) {
    String namespace = root.getNamespaceURI();
    NodeList children = root.getChildNodes();
    for (int i = 0; i < children.getLength(); i++) {
      if (children.item(i) instanceof Element candidate
          && "Header".equals(localName(candidate))
          && namespace != null
          && namespace.equals(candidate.getNamespaceURI())) {
        return candidate;
      }
    }
    return null;
  }

  private static String localName(Element element) {
    String localName = element.getLocalName();
    return localName == null ? element.getTagName() : localName;
  }
}
