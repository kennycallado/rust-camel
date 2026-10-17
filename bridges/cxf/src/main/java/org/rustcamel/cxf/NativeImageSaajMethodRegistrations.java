package org.rustcamel.cxf;

import io.quarkus.runtime.annotations.RegisterForReflection;

/**
 * Registers the SAAJ SOAP/DOM classes (including their methods and constructors) for GraalVM
 * native-image reflection.
 *
 * <p>The failure this guards: WSS4J's {@code WSSecurityUtil.findWsseSecurityHeaderBlock} creates a
 * missing {@code <Header>} via {@code SOAPDocumentImpl.importNode}, which calls {@code
 * createSoapNode(node.getClass(), imported)}. That helper's element branch does {@code
 * clazz.getConstructor(SOAPDocumentImpl.class, Element.class).newInstance(...)} on the CONCRETE
 * SAAJ wrapper class (e.g. {@code ver1_1.Header1_1Impl}). Without constructor reflection metadata
 * for the {@code ver1_1}/{@code ver1_2} classes the lookup throws {@code NoSuchMethodException},
 * SAAJ wraps it in {@code IllegalStateException}, and WSS4J flattens everything to {@code
 * WSSecurityException(INVALID_SECURITY)} ("An error was discovered processing the
 * &lt;wsse:Security&gt; header") and CXF to the generic "Security processing failed." SOAP fault —
 * breaking all outbound WS-Security signing before the request is sent. The abstract {@code impl.*}
 * superclasses do NOT help: constructors are not inherited by {@code getConstructor}.
 */
@RegisterForReflection(
    classNames = {
      // --- SAAJ document / part / message surface ---
      "com.sun.xml.messaging.saaj.soap.SOAPDocumentImpl",
      "com.sun.xml.messaging.saaj.soap.SOAPPartImpl",
      "com.sun.xml.messaging.saaj.soap.SOAPDocument",
      "com.sun.xml.messaging.saaj.soap.SOAPDocumentFragment",
      "com.sun.xml.messaging.saaj.soap.MessageImpl",
      // --- abstract element wrappers (plain elements like wsse:Security) ---
      "com.sun.xml.messaging.saaj.soap.impl.ElementImpl",
      "com.sun.xml.messaging.saaj.soap.impl.EnvelopeImpl",
      "com.sun.xml.messaging.saaj.soap.impl.HeaderImpl",
      "com.sun.xml.messaging.saaj.soap.impl.HeaderElementImpl",
      "com.sun.xml.messaging.saaj.soap.impl.BodyImpl",
      "com.sun.xml.messaging.saaj.soap.impl.BodyElementImpl",
      "com.sun.xml.messaging.saaj.soap.impl.DetailImpl",
      "com.sun.xml.messaging.saaj.soap.impl.DetailEntryImpl",
      "com.sun.xml.messaging.saaj.soap.impl.FaultImpl",
      "com.sun.xml.messaging.saaj.soap.impl.FaultElementImpl",
      "com.sun.xml.messaging.saaj.soap.impl.AttrImpl",
      "com.sun.xml.messaging.saaj.soap.impl.CDATAImpl",
      "com.sun.xml.messaging.saaj.soap.impl.TextImpl",
      "com.sun.xml.messaging.saaj.soap.impl.SOAPTextImpl",
      "com.sun.xml.messaging.saaj.soap.impl.SOAPCommentImpl",
      // --- concrete SOAP 1.1 wrappers (the classes importNode/createSoapNode
      //     actually reflect on; public (SOAPDocumentImpl, Element) ctors) ---
      "com.sun.xml.messaging.saaj.soap.ver1_1.Envelope1_1Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_1.Header1_1Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_1.HeaderElement1_1Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_1.Body1_1Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_1.BodyElement1_1Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_1.Fault1_1Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_1.FaultElement1_1Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_1.Detail1_1Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_1.DetailEntry1_1Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_1.SOAPPart1_1Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_1.Message1_1Impl",
      // --- concrete SOAP 1.2 wrappers ---
      "com.sun.xml.messaging.saaj.soap.ver1_2.Envelope1_2Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_2.Header1_2Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_2.HeaderElement1_2Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_2.Body1_2Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_2.BodyElement1_2Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_2.Fault1_2Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_2.FaultElement1_2Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_2.Detail1_2Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_2.DetailEntry1_2Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_2.SOAPPart1_2Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_2.Message1_2Impl"
    })
public class NativeImageSaajMethodRegistrations {}
