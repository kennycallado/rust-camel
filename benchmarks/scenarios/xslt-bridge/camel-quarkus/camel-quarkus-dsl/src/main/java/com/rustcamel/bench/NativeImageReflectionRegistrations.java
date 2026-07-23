package com.rustcamel.bench;

import io.quarkus.runtime.annotations.RegisterForReflection;

/**
 * Registers third-party classes for GraalVM native-image reflection.
 *
 * <p>Two distinct registration needs are covered here:
 *
 * <ol>
 *   <li><b>xmlresolver</b> classes, loaded via {@code Class.forName()} at
 *       runtime by Saxon's catalog integration (mirrors {@code bridges/xml/
 *       .../NativeImageReflectionRegistrations.java}).</li>
 *   <li><b>External Xerces parser configuration cascade.</b> When
 *       {@code SAXParserFactory.newInstance().newSAXParser()} builds an
 *       {@code org.apache.xerces.parsers.SAXParser}, Xerces'
 *       {@code ObjectFactory.newInstance()} resolves its parser
 *       configuration by <em>name</em> via {@code Class.forName(
 *       "org.apache.xerces.parsers.XIncludeAwareParserConfiguration",
 *       true, contextClassLoader)}. Under GraalVM native-image, a class
 *       reachable in the image heap is still NOT resolvable by
 *       {@code Class.forName} unless it is registered for reflection.
 *       The prior {@code reflect-config.json} string entries were
 *       insufficient because the registration must include the
 *       constructor Xerces invokes reflectively AND the transitive
 *       configuration graph it wires up. Referencing the real classes
 *       from the {@code xerces:xercesImpl:2.12.2} dependency (which is on
 *       the compile classpath) via {@code @RegisterForReflection} gives
 *       Quarkus the strong, constructor-inclusive registration that raw
 *       JSON did not. This is the mechanism the production bridge relies
 *       on implicitly through GraalVM CE's reachability analysis; on the
 *       Mandrel toolchain the registration must be explicit.</li>
 * </ol>
 */
@RegisterForReflection(
    targets = {
      // --- xmlresolver (Saxon catalog integration) ---
      org.xmlresolver.loaders.XmlLoader.class,
      org.xmlresolver.loaders.CatalogLoaderResolver.class,
      org.xmlresolver.Resolver.class,
      org.xmlresolver.XMLResolverConfiguration.class,
      org.xmlresolver.CatalogManager.class,
      org.xmlresolver.ResolverConfiguration.class,
      // --- external Xerces parser configuration cascade ---
      // ObjectFactory.newInstance() resolves these by name via
      // Class.forName; each must be reflection-registered (with its
      // no-arg / SymbolTable constructor) for the lookup to succeed.
      org.apache.xerces.jaxp.SAXParserFactoryImpl.class,
      org.apache.xerces.parsers.SAXParser.class,
      org.apache.xerces.parsers.XIncludeAwareParserConfiguration.class,
      org.apache.xerces.parsers.XML11Configuration.class,
      org.apache.xerces.impl.dv.dtd.DTDDVFactoryImpl.class,
      org.apache.xerces.impl.dv.dtd.XML11DTDDVFactoryImpl.class,
    })
public class NativeImageReflectionRegistrations {}
