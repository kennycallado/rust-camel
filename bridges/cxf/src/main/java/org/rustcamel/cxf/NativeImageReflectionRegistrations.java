package org.rustcamel.cxf;

import io.quarkus.runtime.annotations.RegisterForReflection;

/**
 * Registers classes for GraalVM native-image reflection.
 *
 * <p>Uses {@code classNames} (String-based) instead of {@code targets} (class-literal) to avoid
 * build-time class loading that can fail native-image builds.
 *
 * <p>This annotation is the ONLY live reflection mechanism in this bridge: Quarkus regenerates
 * {@code META-INF/native-image/reflect-config.json} at packaging time from these {@code
 * classNames}, so any static config file placed at that path is silently ignored (its unique
 * entries must live here instead).
 *
 * <p>Every entry here must be a class that exists on the runtime classpath. Nonexistent classes are
 * silently no-ops (Quarkus writes them to the generated config, but native-image never resolves
 * them), which hides typos. After editing this list, force a CLEAN native rebuild: Gradle's
 * incremental cache can serve a stale compiled class to the Quarkus augmentation step, so a
 * warm-cache rebuild may register the OLD list even though {@code compileJava} reports "executed".
 */
@RegisterForReflection(
    classNames = {
      // --- Bridge classes ---
      "org.rustcamel.cxf.PortAnnouncer",
      "org.rustcamel.cxf.BridgeConfig",
      "org.rustcamel.cxf.CxfBridgeService",
      "org.rustcamel.cxf.CxfClientManager",
      "org.rustcamel.cxf.CxfServerManager",
      "org.rustcamel.cxf.SoapEnvelopeHelper",

      // --- gRPC generated service/proto classes ---
      "cxf_bridge.CxfBridgeGrpc",
      "cxf_bridge.CxfBridgeGrpc$CxfBridgeImplBase",
      "cxf_bridge.SoapRequest",
      "cxf_bridge.SoapRequestOrBuilder",
      "cxf_bridge.SoapResponse",
      "cxf_bridge.SoapResponseOrBuilder",
      "cxf_bridge.ConsumerRequest",
      "cxf_bridge.ConsumerRequestOrBuilder",
      "cxf_bridge.ConsumerResponse",
      "cxf_bridge.ConsumerResponseOrBuilder",
      "cxf_bridge.HealthRequest",
      "cxf_bridge.HealthRequestOrBuilder",
      "cxf_bridge.HealthResponse",
      "cxf_bridge.HealthResponseOrBuilder",

      // --- CXF JAX-WS SPI (ServiceLoader bypass for native-image) ---
      "org.apache.cxf.jaxws.spi.ProviderImpl",
      "jakarta.xml.ws.spi.Provider",

      // --- CXF Bus / lifecycle ---
      "org.apache.cxf.bus.CXFBusFactory",
      "org.apache.cxf.bus.extension.ExtensionManagerBus",
      "org.apache.cxf.bus.ManagedBus",
      "org.apache.cxf.Bus",
      "org.apache.cxf.BusFactory",

      // --- CXF bus-extensions.txt (cxf-core) ---
      "org.apache.cxf.bus.managers.ClientLifeCycleManagerImpl",
      "org.apache.cxf.bus.managers.CXFBusLifeCycleManager",
      "org.apache.cxf.bus.managers.EndpointResolverRegistryImpl",
      "org.apache.cxf.bus.managers.HeaderManagerImpl",
      "org.apache.cxf.bus.managers.PhaseManagerImpl",
      "org.apache.cxf.bus.managers.ServerLifeCycleManagerImpl",
      "org.apache.cxf.bus.managers.ServerRegistryImpl",
      "org.apache.cxf.bus.managers.WorkQueueManagerImpl",
      "org.apache.cxf.bus.resource.ResourceManagerImpl",
      "org.apache.cxf.catalog.OASISCatalogManager",
      "org.apache.cxf.common.spi.ClassLoaderProxyService",
      "org.apache.cxf.common.util.ASMHelperImpl",
      "org.apache.cxf.common.logging.Slf4jLogger",
      "org.apache.cxf.io.DelayedCachedOutputStreamCleaner",
      "org.apache.cxf.service.factory.FactoryBeanListenerManager",

      // --- CXF bus-extensions.txt (cxf-rt-*) ---
      // AbstractBindingFactory.setBus is invoked reflectively on the producer/
      // Dispatch path (bus-extensions.txt wiring). Without allDeclaredMethods
      // reflection the native image throws MissingReflectionRegistrationError.
      "org.apache.cxf.binding.AbstractBindingFactory",
      "org.apache.cxf.binding.soap.SoapBindingFactory",
      "org.apache.cxf.binding.soap.SoapTransportFactory",
      "org.apache.cxf.binding.xml.wsdl11.XMLWSDLExtensionLoader",
      "org.apache.cxf.binding.xml.XMLBindingFactory",
      "org.apache.cxf.jaxb.FactoryClassProxyService",
      "org.apache.cxf.jaxb.WrapperHelperProxyService",
      "org.apache.cxf.jaxws.spi.WrapperClassCreatorProxyService",
      "org.apache.cxf.jaxws.spi.WrapperClassNamingConvention$DefaultWrapperClassNamingConvention",
      "org.apache.cxf.transport.http.HTTPTransportFactory",
      "org.apache.cxf.transport.http.HTTPWSDLExtensionLoader",
      "org.apache.cxf.transport.http.policy.HTTPClientAssertionBuilder",
      "org.apache.cxf.transport.http.policy.HTTPServerAssertionBuilder",
      "org.apache.cxf.transport.http.policy.NoOpPolicyInterceptorProvider",
      "org.apache.cxf.ws.addressing.impl.AddressingFeatureApplier",
      "org.apache.cxf.ws.addressing.impl.AddressingWSDLExtensionLoader",
      "org.apache.cxf.ws.addressing.impl.MAPAggregatorImplLoader",
      "org.apache.cxf.endpoint.dynamic.ExceptionClassCreatorProxyService",
      "org.apache.cxf.jaxws.context.WebServiceContextResourceResolver",
      "org.apache.cxf.ws.addressing.policy.AddressingAssertionBuilder",
      "org.apache.cxf.ws.addressing.policy.AddressingPolicyInterceptorProvider",
      "org.apache.cxf.ws.addressing.policy.UsingAddressingAssertionBuilder",
      "org.apache.cxf.wsdl11.WSDLManagerImpl",
      "org.apache.cxf.wsdl.ExtensionClassCreatorProxyService",
      "org.apache.cxf.ws.policy.AssertionBuilderRegistryImpl",
      "org.apache.cxf.ws.policy.attachment.external.DomainExpressionBuilderRegistry",
      "org.apache.cxf.ws.policy.attachment.external.EndpointReferenceDomainExpressionBuilder",
      "org.apache.cxf.ws.policy.attachment.external.URIDomainExpressionBuilder",
      "org.apache.cxf.ws.policy.attachment.ServiceModelPolicyProvider",
      "org.apache.cxf.ws.policy.attachment.wsdl11.Wsdl11AttachmentPolicyProvider",
      "org.apache.cxf.ws.policy.mtom.MTOMAssertionBuilder",
      "org.apache.cxf.ws.policy.mtom.MTOMPolicyInterceptorProvider",
      "org.apache.cxf.ws.policy.PolicyAnnotationListener",
      "org.apache.cxf.ws.policy.PolicyBuilderImpl",
      "org.apache.cxf.ws.policy.PolicyDataEngineImpl",
      "org.apache.cxf.ws.policy.PolicyEngineImpl",
      "org.apache.cxf.ws.policy.PolicyInterceptorProviderRegistryImpl",
      "org.apache.cxf.ws.security.cache.CacheCleanupListener",

      // --- WSDL4J factory loaded by javax.wsdl.WSDLFactory via Class.forName ---
      "com.ibm.wsdl.factory.WSDLFactoryImpl",
      "com.ibm.wsdl.xml.WSDLReaderImpl",
      "com.ibm.wsdl.xml.WSDLWriterImpl",
      "com.ibm.wsdl.extensions.http.HTTPAddressImpl",
      "com.ibm.wsdl.extensions.http.HTTPBindingImpl",
      "com.ibm.wsdl.extensions.http.HTTPOperationImpl",
      "com.ibm.wsdl.extensions.http.HTTPUrlEncodedImpl",
      "com.ibm.wsdl.extensions.http.HTTPUrlReplacementImpl",
      "com.ibm.wsdl.extensions.mime.MIMEContentImpl",
      "com.ibm.wsdl.extensions.mime.MIMEMimeXmlImpl",
      "com.ibm.wsdl.extensions.mime.MIMEMultipartRelatedImpl",
      "com.ibm.wsdl.extensions.mime.MIMEPartImpl",
      "com.ibm.wsdl.extensions.schema.SchemaImpl",
      "com.ibm.wsdl.extensions.schema.SchemaImportImpl",
      "com.ibm.wsdl.extensions.schema.SchemaReferenceImpl",
      "com.ibm.wsdl.extensions.soap.SOAPAddressImpl",
      "com.ibm.wsdl.extensions.soap.SOAPBindingImpl",
      "com.ibm.wsdl.extensions.soap.SOAPBodyImpl",
      "com.ibm.wsdl.extensions.soap.SOAPFaultImpl",
      "com.ibm.wsdl.extensions.soap.SOAPHeaderImpl",
      "com.ibm.wsdl.extensions.soap.SOAPHeaderFaultImpl",
      "com.ibm.wsdl.extensions.soap.SOAPOperationImpl",
      "com.ibm.wsdl.extensions.soap12.SOAP12AddressImpl",
      "com.ibm.wsdl.extensions.soap12.SOAP12BindingImpl",
      "com.ibm.wsdl.extensions.soap12.SOAP12BodyImpl",
      "com.ibm.wsdl.extensions.soap12.SOAP12FaultImpl",
      "com.ibm.wsdl.extensions.soap12.SOAP12HeaderImpl",
      "com.ibm.wsdl.extensions.soap12.SOAP12HeaderFaultImpl",
      "com.ibm.wsdl.extensions.soap12.SOAP12OperationImpl",

      // --- CXF core/frontend/binding ---
      "org.apache.cxf.jaxws.DispatchImpl",
      "org.apache.cxf.jaxws.ServiceImpl",
      "org.apache.cxf.endpoint.ClientImpl",

      // --- Jakarta Activation SPI providers ---
      "org.eclipse.angus.activation.MailcapRegistryProviderImpl",
      "org.eclipse.angus.activation.MimeTypeRegistryProviderImpl",

      // --- CXF SOAP WSDL extension impls (ExtensionRegistry proxies) ---
      "org.apache.cxf.binding.soap.wsdl.extensions.SoapBinding",
      "org.apache.cxf.binding.soap.wsdl.extensions.SoapAddress",
      "org.apache.cxf.binding.soap.wsdl.extensions.SoapBody",
      "org.apache.cxf.binding.soap.wsdl.extensions.SoapFault",
      "org.apache.cxf.binding.soap.wsdl.extensions.SoapHeader",
      "org.apache.cxf.binding.soap.wsdl.extensions.SoapHeaderFault",
      "org.apache.cxf.binding.soap.wsdl.extensions.SoapOperation",
      "javax.wsdl.extensions.ExtensibilityElement",

      // --- WS-Security / WSS4J ---
      "org.apache.cxf.ws.security.wss4j.WSS4JInInterceptor",
      "org.apache.cxf.ws.security.wss4j.WSS4JOutInterceptor",
      "org.apache.wss4j.common.ConfigurationConstants",
      "org.apache.wss4j.common.crypto.Crypto",
      "org.apache.wss4j.common.crypto.CryptoFactory",
      "org.apache.wss4j.common.ext.WSPasswordCallback",
      "org.apache.wss4j.dom.WSConstants",
      "org.apache.wss4j.dom.engine.WSSConfig",
      "org.apache.wss4j.dom.engine.WSSecurityEngine",
      "org.apache.wss4j.dom.handler.RequestData",
      "org.apache.wss4j.dom.handler.WSHandlerConstants",
      "org.apache.wss4j.dom.message.WSSecHeader",
      "org.apache.wss4j.dom.message.WSSecSignature",
      "org.apache.wss4j.common.crypto.Merlin",
      "org.apache.wss4j.dom.processor.SignatureProcessor",
      "org.apache.wss4j.dom.processor.TimestampProcessor",
      "org.apache.wss4j.dom.processor.EncryptedKeyProcessor",
      "org.apache.wss4j.dom.processor.ReferenceListProcessor",
      // BST_DIRECT_REFERENCE signatures carry a wsse:BinarySecurityToken, which
      // WSS4J resolves and instantiates reflectively during inbound verification.
      // Without this entry the native image fails with
      // "Unable to load class org.apache.wss4j.dom.processor.BinarySecurityTokenProcessor".
      "org.apache.wss4j.dom.processor.BinarySecurityTokenProcessor",
      "org.apache.wss4j.dom.message.WSSecEncrypt",
      "org.apache.wss4j.dom.message.WSSecBase",
      "org.apache.wss4j.dom.validate.SignatureTrustValidator",
      // --- SAAJ impl (jakarta.xml.soap factories for the WSS4J DOM path) ---
      "com.sun.xml.messaging.saaj.soap.SAAJMetaFactoryImpl",
      "com.sun.xml.messaging.saaj.soap.MessageFactoryImpl",
      "com.sun.xml.messaging.saaj.soap.SOAPFactoryImpl",
      "com.sun.xml.messaging.saaj.soap.ver1_1.SOAPMessageFactory1_1Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_2.SOAPMessageFactory1_2Impl",
      // Exact impls named by the saaj-impl 3.0.6 META-INF/services files
      // (jakarta.xml.soap.SOAPFactory / SOAPConnectionFactory) — the service
      // resources above are read reflectively at runtime in the native image.
      "com.sun.xml.messaging.saaj.soap.ver1_1.SOAPFactory1_1Impl",
      "com.sun.xml.messaging.saaj.soap.ver1_2.SOAPFactory1_2Impl",
      "com.sun.xml.messaging.saaj.client.p2p.HttpSOAPConnectionFactory",
      // --- xmlsec 4.0.4 sign/verify implementations + wss4j token classes ---
      // Belt-and-braces for the WSS4J DOM signing path (WSSecSignature.build
      // + XMLSignature.sign). The Santuario registries hold class literals, so
      // these should be reachable by static analysis; registering the
      // constructors removes any residual reflective-construction risk and is
      // a silent no-op for names the image already covers.
      "org.apache.xml.security.algorithms.implementations.SignatureBaseRSA$SignatureRSASHA256",
      "org.apache.xml.security.algorithms.implementations.SignatureBaseRSA$SignatureRSASHA1",
      "org.apache.xml.security.algorithms.implementations.SignatureBaseRSA$SignatureRSASHA384",
      "org.apache.xml.security.algorithms.implementations.SignatureBaseRSA$SignatureRSASHA512",
      "org.apache.xml.security.transforms.implementations.TransformC14N",
      "org.apache.xml.security.transforms.implementations.TransformC14NWithComments",
      "org.apache.xml.security.transforms.implementations.TransformC14NExclusive",
      "org.apache.xml.security.transforms.implementations.TransformC14NExclusiveWithComments",
      "org.apache.xml.security.transforms.implementations.TransformC14N11",
      "org.apache.xml.security.transforms.implementations.TransformC14N11_WithComments",
      "org.apache.xml.security.transforms.implementations.TransformEnvelopedSignature",
      "org.apache.xml.security.transforms.implementations.TransformBase64Decode",
      "org.apache.xml.security.transforms.implementations.TransformXPath",
      "org.apache.xml.security.c14n.implementations.Canonicalizer20010315",
      "org.apache.xml.security.c14n.implementations.Canonicalizer20010315OmitComments",
      "org.apache.xml.security.c14n.implementations.Canonicalizer20010315WithComments",
      "org.apache.xml.security.c14n.implementations.Canonicalizer20010315Excl",
      "org.apache.xml.security.c14n.implementations.Canonicalizer20010315ExclOmitComments",
      "org.apache.xml.security.c14n.implementations.Canonicalizer20010315ExclWithComments",
      "org.apache.xml.security.c14n.implementations.Canonicalizer11_OmitComments",
      "org.apache.xml.security.c14n.implementations.Canonicalizer11_WithComments",
      // WSS4J 4.x signs with the "physical" canonicalizer internally
      // (http://santuario.apache.org/c14n/physical). xmlsec resolves
      // canonicalizers reflectively (Canonicalizer.register(String, String) +
      // no-arg constructor) — without this entry the native image fails with
      // "Unknown canonicalizer ... CanonicalizerPhysical.<init>()" and the
      // outbound signature collapses to the generic "Security processing
      // failed." SOAP fault.
      "org.apache.xml.security.c14n.implementations.CanonicalizerPhysical",
      "org.apache.wss4j.dom.transform.STRTransform",
      "org.apache.wss4j.common.token.BinarySecurity",
      "org.apache.wss4j.common.token.SecurityTokenReference",
      "org.apache.wss4j.common.token.Reference",
      // --- wss4j action/processor/validator objects ---
      // WSSConfig.getAction/getProcessor/getValidator instantiate these with
      // getDeclaredConstructor().newInstance() from name-keyed maps; without
      // constructor reflection the native image fails inside doSenderAction
      // right after the SAAJ header creation works. SAML classes are
      // deliberately excluded (OpenSAML types would break the native build
      // when OpenSAML is absent).
      "org.apache.wss4j.dom.action.SignatureAction",
      "org.apache.wss4j.dom.action.TimestampAction",
      "org.apache.wss4j.dom.action.EncryptionAction",
      "org.apache.wss4j.dom.action.UsernameTokenAction",
      "org.apache.wss4j.dom.action.UsernameTokenSignedAction",
      "org.apache.wss4j.dom.action.SignatureConfirmationAction",
      "org.apache.wss4j.dom.action.SignatureDerivedAction",
      "org.apache.wss4j.dom.action.EncryptionDerivedAction",
      "org.apache.wss4j.dom.action.CustomTokenAction",
      "org.apache.wss4j.dom.processor.EncryptedDataProcessor",
      "org.apache.wss4j.dom.processor.DerivedKeyTokenProcessor",
      "org.apache.wss4j.dom.processor.SecurityContextTokenProcessor",
      "org.apache.wss4j.dom.processor.SignatureConfirmationProcessor",
      "org.apache.wss4j.dom.processor.UsernameTokenProcessor",
      "org.apache.wss4j.dom.validate.TimestampValidator",
      "org.apache.wss4j.dom.validate.UsernameTokenValidator",
      "org.apache.wss4j.dom.transform.AttachmentCiphertextTransform",
      "org.apache.wss4j.dom.resolvers.ResolverAttachment",
      "org.rustcamel.cxf.WssSecurityProcessor"
    })
public class NativeImageReflectionRegistrations {}
