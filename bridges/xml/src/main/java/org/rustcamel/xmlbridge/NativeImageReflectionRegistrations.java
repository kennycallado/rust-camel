package org.rustcamel.xmlbridge;

import io.quarkus.runtime.annotations.RegisterForReflection;

/**
 * Registers third-party classes for GraalVM native-image reflection.
 *
 * <p>These classes are loaded via {@code Class.forName()} at runtime (by Saxon's xmlresolver
 * integration, and by Xerces' internal {@code ObjectFactory} service discovery) and must be
 * explicitly registered so that GraalVM includes them in the native binary.
 *
 * <p>Re-verify the Xerces target list when bumping GraalVM or {@code xercesImpl}: the names come
 * from Xerces' internal {@code ObjectFactory} string lookups and change between versions. Note that
 * Quarkus ignores the application's own {@code reflect-config.json} — registration must stay here
 * via {@code @RegisterForReflection} only.
 */
@RegisterForReflection(
    targets = {
      org.xmlresolver.loaders.XmlLoader.class,
      org.xmlresolver.loaders.CatalogLoaderResolver.class,
      org.xmlresolver.Resolver.class,
      org.xmlresolver.XMLResolverConfiguration.class,
      org.xmlresolver.CatalogManager.class,
      org.xmlresolver.ResolverConfiguration.class,
      // Xerces ObjectFactory fallback targets: loaded reflectively by name from
      // DTDDVFactory/SchemaDVFactory (datatype validators), XMLGrammarPreparser
      // (schema/DTD loaders), DOMParserImpl, CoreDOMImplementationImpl and
      // XIncludeHandler. Without registration the classes are absent from the
      // native image and schema registration fails with
      // "Provider ... not found" (ClassNotFoundException).
      // Deliberate exclusions: DOM ObjectFactory targets (the bridge is
      // SAX-only) and XSD 1.1 ExtendedSchemaDVFactoryImpl (the JAXP factory
      // is XSD 1.0) are intentionally NOT registered.
      org.apache.xerces.impl.dv.dtd.DTDDVFactoryImpl.class,
      org.apache.xerces.impl.dv.dtd.XML11DTDDVFactoryImpl.class,
      org.apache.xerces.impl.dv.xs.SchemaDVFactoryImpl.class,
      org.apache.xerces.impl.xs.XMLSchemaLoader.class,
      org.apache.xerces.impl.dtd.XMLDTDLoader.class,
      org.apache.xerces.parsers.XIncludeAwareParserConfiguration.class,
      org.apache.xerces.dom.PSVIDocumentImpl.class,
      org.apache.xerces.impl.xs.XMLSchemaValidator.class,
      org.apache.xerces.impl.dtd.XML11DTDValidator.class,
      org.apache.xerces.impl.dtd.XMLDTDValidator.class,
      org.apache.xerces.impl.dtd.XML11DTDProcessor.class,
      org.apache.xerces.parsers.XIncludeParserConfiguration.class,
      org.apache.xerces.parsers.XPointerParserConfiguration.class,
    })
public class NativeImageReflectionRegistrations {}
