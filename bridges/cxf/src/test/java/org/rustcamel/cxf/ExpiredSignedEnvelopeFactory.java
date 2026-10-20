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
import org.apache.wss4j.dom.message.WSSecTimestamp;
import org.w3c.dom.Document;

/**
 * Builds SOAP 1.1 envelopes signed by caller-supplied test material. Test-only: no production code
 * path uses it.
 */
final class ExpiredSignedEnvelopeFactory {

  private ExpiredSignedEnvelopeFactory() {}

  /**
   * Signs {@code soapXml} over the Body Content (plus Timestamp when requested). The BinarySecurity
   * Token direct reference carries the leaf certificate alone, or the whole key-entry chain as an
   * X509PKIPathv1 token when {@code includeCertificateChain} is true.
   */
  static String signSoap11(
      String soapXml,
      Path keystore,
      String alias,
      String signatureAlgorithm,
      String digestAlgorithm,
      boolean includeTimestamp,
      boolean includeCertificateChain)
      throws Exception {
    Document doc = SoapEnvelopeHelper.parseResponse(soapXml);
    WSSecHeader secHeader = new WSSecHeader(doc);
    secHeader.insertSecurityHeader();

    if (includeTimestamp) {
      new WSSecTimestamp(secHeader).build();
    }

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
    parts.add(new WSEncryptionPart("Body", doc.getDocumentElement().getNamespaceURI(), "Content"));
    if (includeTimestamp) {
      parts.add(new WSEncryptionPart("Timestamp", WSConstants.WSU_NS, ""));
    }

    sign.setKeyIdentifierType(WSConstants.BST_DIRECT_REFERENCE);
    sign.setUseSingleCertificate(!includeCertificateChain);

    sign.prepare(crypto);
    // The BST is materialized only after prepare(); a direct BST reference cannot resolve before
    // the element is in the security header. With useSingleCertificate(false) WSS4J emits the
    // full certificate chain as an X509PKIPathv1 token.
    sign.prependBSTElementToHeader();
    sign.computeSignature(sign.addReferencesToSign(sign.getParts()));

    return SoapEnvelopeHelper.sourceToString(new DOMSource(doc));
  }
}
