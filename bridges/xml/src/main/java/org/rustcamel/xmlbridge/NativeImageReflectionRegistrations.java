package org.rustcamel.xmlbridge;

import io.quarkus.runtime.annotations.RegisterForReflection;

/**
 * Registers third-party classes for GraalVM native-image reflection.
 *
 * <p>These classes are loaded via {@code Class.forName()} at runtime (by Saxon's xmlresolver
 * integration) and must be explicitly registered so that GraalVM includes them in the native
 * binary.
 */
@RegisterForReflection(
    targets = {
      org.xmlresolver.loaders.XmlLoader.class,
      org.xmlresolver.loaders.CatalogLoaderResolver.class,
      org.xmlresolver.Resolver.class,
      org.xmlresolver.XMLResolverConfiguration.class,
      org.xmlresolver.CatalogManager.class,
      org.xmlresolver.ResolverConfiguration.class,
    })
public class NativeImageReflectionRegistrations {}
