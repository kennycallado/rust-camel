package org.rustcamel.cxf;

import io.quarkus.runtime.annotations.RegisterForReflection;

/**
 * Registers classes for GraalVM native-image reflection.
 *
 * <p>Uses {@code classNames} (String-based) instead of {@code targets} (class-literal) to avoid
 * build-time class loading that can fail native-image builds.
 */
@RegisterForReflection(
    classNames = {
      // --- Bridge classes ---
      "org.rustcamel.cxf.PortAnnouncer",
      "org.rustcamel.cxf.BridgeConfig",
      "org.rustcamel.cxf.CxfBridgeService",
      "org.rustcamel.cxf.CxfClientManager",
      "org.rustcamel.cxf.CxfServerManager",
      "org.rustcamel.cxf.SecurityConfig",
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
      "org.apache.wss4j.common.crypto.Merlin",
      "org.apache.wss4j.common.crypto.CryptoFactory",
      "org.apache.wss4j.dom.engine.WSSConfig",
      "org.apache.wss4j.dom.handler.WSHandlerConstants",
      "org.apache.wss4j.common.ext.WSPasswordCallback"
    })
public class NativeImageReflectionRegistrations {}
