package org.rustcamel.xmlbridge;

import io.quarkus.grpc.GrpcService;
import io.smallrye.common.annotation.Blocking;
import io.smallrye.mutiny.Uni;
import jakarta.inject.Inject;
import java.io.ByteArrayInputStream;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.HexFormat;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import javax.xml.XMLConstants;
import javax.xml.parsers.SAXParserFactory;
import javax.xml.transform.Source;
import javax.xml.transform.sax.SAXSource;
import javax.xml.transform.stream.StreamSource;
import javax.xml.validation.Schema;
import javax.xml.validation.SchemaFactory;
import org.xml.sax.ErrorHandler;
import org.xml.sax.InputSource;
import org.xml.sax.SAXException;
import org.xml.sax.SAXParseException;
import xml_bridge.BridgeError;
import xml_bridge.MutinyXsdValidatorGrpc;
import xml_bridge.RegisterSchemaRequest;
import xml_bridge.RegisterSchemaResponse;
import xml_bridge.UnregisterSchemaRequest;
import xml_bridge.UnregisterSchemaResponse;
import xml_bridge.ValidateRequest;
import xml_bridge.ValidateResponse;
import xml_bridge.ValidateWithRequest;

@GrpcService
public class XsdValidatorService extends MutinyXsdValidatorGrpc.XsdValidatorImplBase {
  private static final String FEATURE_LOAD_EXTERNAL_DTD =
      "http://apache.org/xml/features/nonvalidating/load-external-dtd";
  private static final String FEATURE_EXTERNAL_GENERAL_ENTITIES =
      "http://xml.org/sax/features/external-general-entities";
  private static final String FEATURE_EXTERNAL_PARAMETER_ENTITIES =
      "http://xml.org/sax/features/external-parameter-entities";
  private static final String PROPERTY_ENTITY_EXPANSION_LIMIT =
      "http://www.oracle.com/xml/jaxp/properties/entityExpansionLimit";

  @Inject SchemaCache schemaCache;

  private final Map<String, String> schemaHashById = new ConcurrentHashMap<>();

  private static BridgeError error(BridgeError.Kind kind, String message) {
    return BridgeError.newBuilder().setKind(kind).setMessage(message).build();
  }

  @Override
  @Blocking
  public Uni<RegisterSchemaResponse> registerSchema(RegisterSchemaRequest req) {
    return Uni.createFrom()
        .item(
            () -> {
              var schemaId = req.getSchemaId();
              var schemaBytes = req.getSchema().toByteArray();
              var hash = sha256Hex(schemaBytes);
              try {
                getOrCompileSchema(hash, schemaBytes);
                schemaHashById.put(schemaId, hash);
                return RegisterSchemaResponse.newBuilder().setSchemaId(schemaId).build();
              } catch (SAXException e) {
                return RegisterSchemaResponse.newBuilder()
                    .setSchemaId(schemaId)
                    .setError(
                        error(
                            BridgeError.Kind.COMPILATION_FAILED,
                            "SCHEMA_PARSE_ERROR: " + e.getMessage()))
                    .build();
              } catch (Exception e) {
                return RegisterSchemaResponse.newBuilder()
                    .setSchemaId(schemaId)
                    .setError(
                        error(
                            BridgeError.Kind.INTERNAL, "registerSchema failed: " + e.getMessage()))
                    .build();
              }
            });
  }

  @Override
  @Blocking
  public Uni<ValidateResponse> validate(ValidateRequest req) {
    return Uni.createFrom()
        .item(
            () -> validateInternal(req.getSchema().toByteArray(), req.getDocument().toByteArray()));
  }

  @Override
  @Blocking
  public Uni<ValidateResponse> validateWith(ValidateWithRequest req) {
    return Uni.createFrom()
        .item(
            () -> {
              var hash = schemaHashById.get(req.getSchemaId());
              if (hash == null) {
                return ValidateResponse.newBuilder()
                    .setValid(false)
                    .setError(
                        error(
                            BridgeError.Kind.RESOURCE_NOT_FOUND,
                            "unknown schema_id: " + req.getSchemaId()))
                    .build();
              }

              var schema = schemaCache.get(hash);
              if (schema == null) {
                return ValidateResponse.newBuilder()
                    .setValid(false)
                    .setError(
                        error(
                            BridgeError.Kind.RESOURCE_NOT_FOUND,
                            "schema not loaded for schema_id: " + req.getSchemaId()))
                    .build();
              }

              return validateAgainstSchema(schema, req.getDocument().toByteArray());
            });
  }

  @Override
  public Uni<UnregisterSchemaResponse> unregisterSchema(UnregisterSchemaRequest req) {
    return Uni.createFrom()
        .item(
            () -> {
              var hash = schemaHashById.remove(req.getSchemaId());
              var released = hash != null && schemaCache.remove(hash);
              return UnregisterSchemaResponse.newBuilder().setReleased(released).build();
            });
  }

  private ValidateResponse validateInternal(byte[] schemaBytes, byte[] documentBytes) {
    try {
      var hash = sha256Hex(schemaBytes);
      var schema = getOrCompileSchema(hash, schemaBytes);
      return validateAgainstSchema(schema, documentBytes);
    } catch (SAXException e) {
      return ValidateResponse.newBuilder()
          .setValid(false)
          .setError(
              error(BridgeError.Kind.COMPILATION_FAILED, "SCHEMA_PARSE_ERROR: " + e.getMessage()))
          .build();
    } catch (Exception e) {
      return ValidateResponse.newBuilder()
          .setValid(false)
          .setError(error(BridgeError.Kind.INTERNAL, "validate failed: " + e.getMessage()))
          .build();
    }
  }

  private ValidateResponse validateAgainstSchema(Schema schema, byte[] documentBytes) {
    try {
      var validator = schema.newValidator();
      // These properties exist in JAXP 1.5+ but may not be recognized by all impls
      try {
        validator.setProperty(XMLConstants.ACCESS_EXTERNAL_DTD, "");
      } catch (SAXException ignored) {
      }
      try {
        validator.setProperty(XMLConstants.ACCESS_EXTERNAL_SCHEMA, "");
      } catch (SAXException ignored) {
      }
      var collector = new ValidationIssueCollector();
      validator.setErrorHandler(collector);
      validator.validate(secureSaxSource(documentBytes));
      if (collector.hasErrors()) {
        return ValidateResponse.newBuilder()
            .setValid(false)
            .addAllErrors(collector.asProtoErrors())
            .setError(error(BridgeError.Kind.VALIDATION_FAILED, collector.asBridgeMessage()))
            .build();
      }
      return ValidateResponse.newBuilder().setValid(true).build();
    } catch (SAXException e) {
      return ValidateResponse.newBuilder()
          .setValid(false)
          .addErrors(
              xml_bridge.ValidationError.newBuilder()
                  .setMessage(e.getMessage() == null ? "validation failed" : e.getMessage())
                  .setSeverity("ERROR")
                  .build())
          .setError(
              error(BridgeError.Kind.VALIDATION_FAILED, "VALIDATION_ERROR: " + e.getMessage()))
          .build();
    } catch (Exception e) {
      return ValidateResponse.newBuilder()
          .setValid(false)
          .setError(error(BridgeError.Kind.SECURITY_VIOLATION, "SECURITY_ERROR: " + e.getMessage()))
          .build();
    }
  }

  private Schema getOrCompileSchema(String hash, byte[] schemaBytes) throws Exception {
    var cached = schemaCache.get(hash);
    if (cached != null) {
      return cached;
    }

    synchronized (schemaCache) {
      var existing = schemaCache.get(hash);
      if (existing != null) {
        return existing;
      }

      // Use JAXP discovery — Quarkus puts xerces on the classpath.
      // Do NOT specify a classloader: Quarkus augmentation handles JAXP SPI properly.
      var factory = SchemaFactory.newInstance(XMLConstants.W3C_XML_SCHEMA_NS_URI);
      factory.setFeature(XMLConstants.FEATURE_SECURE_PROCESSING, true);
      // Block external DTD/schema access (JAXP 1.5+)
      trySetProperty(factory, XMLConstants.ACCESS_EXTERNAL_DTD, "");
      trySetProperty(factory, XMLConstants.ACCESS_EXTERNAL_SCHEMA, "");

      var source = new StreamSource(new ByteArrayInputStream(schemaBytes));
      var compiled = factory.newSchema(source);
      schemaCache.put(hash, compiled);
      return compiled;
    }
  }

  /** Set a SchemaFactory property, ignoring unsupported-property exceptions. */
  private static void trySetProperty(SchemaFactory f, String name, String value) {
    try {
      f.setProperty(name, value);
    } catch (org.xml.sax.SAXNotRecognizedException | org.xml.sax.SAXNotSupportedException ignored) {
      // property not supported by this implementation — skip
    }
  }

  private static Source secureSaxSource(byte[] xmlBytes) throws Exception {
    // Use default JAXP factory — Quarkus class loading finds the right impl.
    var spf = SAXParserFactory.newInstance();
    spf.setNamespaceAware(true);
    spf.setFeature(XMLConstants.FEATURE_SECURE_PROCESSING, true);
    // Block external entity loading (standard JAXP)
    trySetFeature(spf, FEATURE_EXTERNAL_GENERAL_ENTITIES, false);
    trySetFeature(spf, FEATURE_EXTERNAL_PARAMETER_ENTITIES, false);
    trySetFeature(spf, FEATURE_LOAD_EXTERNAL_DTD, false);
    // Disallow DOCTYPE entirely (Xerces-specific but effective when available)
    trySetFeature(spf, "http://apache.org/xml/features/disallow-doctype-decl", true);
    var reader = spf.newSAXParser().getXMLReader();
    // Hard entity-expansion limit (Xerces-specific, skip if unsupported)
    try {
      reader.setProperty(PROPERTY_ENTITY_EXPANSION_LIMIT, 100);
    } catch (SAXException ignored) {
    }
    reader.setEntityResolver(
        (publicId, systemId) -> new InputSource(new ByteArrayInputStream(new byte[0])));
    return new SAXSource(reader, new InputSource(new ByteArrayInputStream(xmlBytes)));
  }

  private static void trySetFeature(SAXParserFactory spf, String name, boolean value) {
    try {
      spf.setFeature(name, value);
    } catch (Exception ignored) {
    }
  }

  private static String sha256Hex(byte[] bytes) {
    try {
      var digest = MessageDigest.getInstance("SHA-256");
      return HexFormat.of().formatHex(digest.digest(bytes));
    } catch (NoSuchAlgorithmException e) {
      throw new IllegalStateException("SHA-256 unavailable", e);
    }
  }

  private static final class ValidationIssueCollector implements ErrorHandler {
    private final List<SAXParseException> errors = new ArrayList<>();
    private final List<SAXParseException> warnings = new ArrayList<>();

    @Override
    public void warning(SAXParseException exception) {
      warnings.add(exception); // warnings don't count as validation failures
    }

    @Override
    public void error(SAXParseException exception) {
      errors.add(exception);
    }

    @Override
    public void fatalError(SAXParseException exception) {
      errors.add(exception);
    }

    boolean hasErrors() {
      return !errors.isEmpty();
    }

    List<xml_bridge.ValidationError> asProtoErrors() {
      var out = new ArrayList<xml_bridge.ValidationError>(errors.size());
      for (var issue : errors) {
        out.add(
            xml_bridge.ValidationError.newBuilder()
                .setMessage(issue.getMessage() == null ? "validation failed" : issue.getMessage())
                .setLine(Math.max(issue.getLineNumber(), 0))
                .setColumn(Math.max(issue.getColumnNumber(), 0))
                .setSeverity("ERROR")
                .build());
      }
      return out;
    }

    String asBridgeMessage() {
      if (errors.isEmpty()) {
        return "VALIDATION_ERROR: unknown";
      }
      var first = errors.get(0);
      var msg = first.getMessage() == null ? "validation failed" : first.getMessage();
      return "VALIDATION_ERROR: " + msg;
    }
  }
}
