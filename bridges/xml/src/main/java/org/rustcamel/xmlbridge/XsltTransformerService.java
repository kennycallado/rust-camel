package org.rustcamel.xmlbridge;

import io.quarkus.grpc.GrpcService;
import io.smallrye.common.annotation.Blocking;
import io.smallrye.mutiny.Uni;
import jakarta.inject.Inject;
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.HexFormat;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import javax.xml.XMLConstants;
import javax.xml.parsers.SAXParserFactory;
import javax.xml.transform.Templates;
import javax.xml.transform.sax.SAXSource;
import javax.xml.transform.stream.StreamResult;
import net.sf.saxon.Configuration;
import net.sf.saxon.TransformerFactoryImpl;
import net.sf.saxon.lib.Feature;
import org.xml.sax.InputSource;
import xml_bridge.BridgeError;
import xml_bridge.CompileStylesheetRequest;
import xml_bridge.CompileStylesheetResponse;
import xml_bridge.MutinyXsltTransformerGrpc;
import xml_bridge.ReleaseStylesheetRequest;
import xml_bridge.ReleaseStylesheetResponse;
import xml_bridge.TransformRequest;
import xml_bridge.TransformResponse;

@GrpcService
public class XsltTransformerService extends MutinyXsltTransformerGrpc.XsltTransformerImplBase {
  private static final String FEATURE_LOAD_EXTERNAL_DTD =
      "http://apache.org/xml/features/nonvalidating/load-external-dtd";
  private static final String FEATURE_EXTERNAL_GENERAL_ENTITIES =
      "http://xml.org/sax/features/external-general-entities";
  private static final String FEATURE_EXTERNAL_PARAMETER_ENTITIES =
      "http://xml.org/sax/features/external-parameter-entities";
  private static final String PROPERTY_ENTITY_EXPANSION_LIMIT =
      "http://www.oracle.com/xml/jaxp/properties/entityExpansionLimit";

  @Inject StylesheetCache stylesheetCache;

  private final Map<String, String> stylesheetHashById = new ConcurrentHashMap<>();

  private static BridgeError error(BridgeError.Kind kind, String message) {
    return BridgeError.newBuilder().setKind(kind).setMessage(message).build();
  }

  @Override
  @Blocking
  public Uni<CompileStylesheetResponse> compileStylesheet(CompileStylesheetRequest req) {
    return Uni.createFrom()
        .item(
            () -> {
              var stylesheetId = req.getStylesheetId();
              var stylesheetBytes = req.getStylesheet().toByteArray();
              var hash = sha256Hex(stylesheetBytes);
              try {
                getOrCompileTemplates(hash, stylesheetBytes);
                stylesheetHashById.put(stylesheetId, hash);
                return CompileStylesheetResponse.newBuilder().setStylesheetId(stylesheetId).build();
              } catch (Exception e) {
                return CompileStylesheetResponse.newBuilder()
                    .setStylesheetId(stylesheetId)
                    .setError(
                        error(
                            BridgeError.Kind.COMPILATION_FAILED,
                            "STYLESHEET_COMPILATION_ERROR: " + e.getMessage()))
                    .build();
              }
            });
  }

  @Override
  @Blocking
  public Uni<TransformResponse> transform(TransformRequest req) {
    return Uni.createFrom()
        .item(
            () -> {
              var hash = stylesheetHashById.get(req.getStylesheetId());
              if (hash == null) {
                return TransformResponse.newBuilder()
                    .setError(
                        error(
                            BridgeError.Kind.RESOURCE_NOT_FOUND,
                            "unknown stylesheet_id: " + req.getStylesheetId()))
                    .build();
              }

              var templates = stylesheetCache.get(hash);
              if (templates == null) {
                return TransformResponse.newBuilder()
                    .setError(
                        error(
                            BridgeError.Kind.RESOURCE_NOT_FOUND,
                            "stylesheet not loaded for stylesheet_id: " + req.getStylesheetId()))
                    .build();
              }

              try {
                var transformer = templates.newTransformer();
                transformer.setURIResolver(
                    (href, base) -> {
                      throw new javax.xml.transform.TransformerException(
                          "External resource access denied: " + href);
                    });
                req.getParametersMap().forEach(transformer::setParameter);
                if (!req.getOutputMethod().isEmpty()) {
                  transformer.setOutputProperty(
                      javax.xml.transform.OutputKeys.METHOD, req.getOutputMethod());
                }

                var out = new ByteArrayOutputStream();
                transformer.transform(
                    secureSaxSource(req.getDocument().toByteArray()), new StreamResult(out));
                return TransformResponse.newBuilder()
                    .setResult(com.google.protobuf.ByteString.copyFrom(out.toByteArray()))
                    .build();
              } catch (Exception e) {
                return TransformResponse.newBuilder()
                    .setError(
                        error(
                            BridgeError.Kind.SECURITY_VIOLATION,
                            "TRANSFORM_ERROR: " + e.getMessage()))
                    .build();
              }
            });
  }

  @Override
  public Uni<ReleaseStylesheetResponse> releaseStylesheet(ReleaseStylesheetRequest req) {
    return Uni.createFrom()
        .item(
            () -> {
              var hash = stylesheetHashById.remove(req.getStylesheetId());
              if (hash != null) {
                stylesheetCache.remove(hash);
              }
              return ReleaseStylesheetResponse.newBuilder().build();
            });
  }

  private Templates getOrCompileTemplates(String hash, byte[] stylesheetBytes) throws Exception {
    var cached = stylesheetCache.get(hash);
    if (cached != null) {
      return cached;
    }

    synchronized (stylesheetCache) {
      var existing = stylesheetCache.get(hash);
      if (existing != null) {
        return existing;
      }
      var config = new Configuration();
      config.setBooleanProperty(Feature.ALLOW_EXTERNAL_FUNCTIONS, false);
      config.setBooleanProperty(Feature.DTD_VALIDATION, false);
      var factory = new TransformerFactoryImpl(config);

      factory.setFeature(XMLConstants.FEATURE_SECURE_PROCESSING, true);
      factory.setAttribute(XMLConstants.ACCESS_EXTERNAL_DTD, "");
      factory.setAttribute(XMLConstants.ACCESS_EXTERNAL_STYLESHEET, "");
      factory.setURIResolver(
          (href, base) -> {
            throw new javax.xml.transform.TransformerException(
                "External URI access denied: " + href);
          });

      var templates = factory.newTemplates(secureSaxSource(stylesheetBytes));
      stylesheetCache.put(hash, templates);
      return templates;
    }
  }

  private static SAXSource secureSaxSource(byte[] xmlBytes) throws Exception {
    var spf = SAXParserFactory.newInstance();
    spf.setNamespaceAware(true);
    spf.setFeature(XMLConstants.FEATURE_SECURE_PROCESSING, true);
    spf.setFeature(FEATURE_LOAD_EXTERNAL_DTD, false);
    spf.setFeature(FEATURE_EXTERNAL_GENERAL_ENTITIES, false);
    spf.setFeature(FEATURE_EXTERNAL_PARAMETER_ENTITIES, false);
    spf.setFeature("http://apache.org/xml/features/disallow-doctype-decl", true); // I-4
    var reader = spf.newSAXParser().getXMLReader();
    reader.setProperty(PROPERTY_ENTITY_EXPANSION_LIMIT, 100); // I-1
    reader.setEntityResolver(
        (publicId, systemId) -> new InputSource(new ByteArrayInputStream(new byte[0])));
    return new SAXSource(reader, new InputSource(new ByteArrayInputStream(xmlBytes)));
  }

  private static String sha256Hex(byte[] bytes) {
    try {
      var digest = MessageDigest.getInstance("SHA-256");
      return HexFormat.of().formatHex(digest.digest(bytes));
    } catch (NoSuchAlgorithmException e) {
      throw new IllegalStateException("SHA-256 unavailable", e);
    }
  }
}
