package org.rustcamel.xmlbridge;

import io.quarkus.grpc.GrpcService;
import io.smallrye.common.annotation.Blocking;
import io.smallrye.mutiny.Uni;
import jakarta.inject.Inject;
import java.io.ByteArrayInputStream;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutionException;
import javax.xml.XMLConstants;
import javax.xml.transform.Source;
import javax.xml.transform.sax.SAXSource;
import javax.xml.validation.Schema;
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
  /**
   * Hard bound on any diagnostic text echoed back over the bridge. Untrusted resource keys or
   * parser messages can be arbitrarily long; only a bounded category plus an optional truncated
   * snippet ever leaves the bridge.
   */
  private static final int MAX_ERROR_TEXT = 512;

  private static final int MAX_SNIPPET_TEXT = 64;

  @Inject SchemaCache schemaCache;

  private final Map<String, String> schemaHashById = new ConcurrentHashMap<>();

  /**
   * Compiles currently in progress, keyed by digest, so concurrent registrations of the same schema
   * set share one compile. A leader inserts its future before compiling and removes it afterwards;
   * joiners await the leader's result instead of compiling a second time.
   */
  private final ConcurrentHashMap<String, CompletableFuture<Schema>> inFlightCompiles =
      new ConcurrentHashMap<>();

  /**
   * Real service monitor for compound registry reads and writes. This is a plain object rather than
   * the injected {@link SchemaCache}: when {@code SchemaCache} is a CDI client proxy, synchronizing
   * on it locks the proxy, not the bean, so it cannot guard the {@code schemaHashById} / cache
   * pair. {@code validateWith} captures both the digest and the compiled schema under this lock,
   * then validates outside it against an immutable {@link Schema} strong reference.
   *
   * <p>The lock order is {@code registryLock} → {@link SchemaCache} monitor, one direction only:
   * the cache's {@code putAndPin}/{@code getAndPin} critical sections never acquire {@code
   * registryLock}, and a schema compile holds neither monitor. The order is therefore acyclic and
   * deadlock-free, and a registration no longer blocks {@code snapshotFor} for the duration of a
   * compile.
   *
   * <p>Publication re-asserts the pin with {@code putAndPin} inside this monitor, together with the
   * id-to-digest map swap. The two-level pin (the compile path already pins before waiting for this
   * monitor, and publication pins again while holding it) makes pin and map swap atomic with
   * respect to a racing replacement's eviction: a racing {@code unpin}/{@code remove} either runs
   * before publication, in which case publication re-inserts the retained {@link Schema}, or runs
   * after, in which case the map already references the digest and {@code containsValue} keeps it.
   */
  private final Object registryLock = new Object();

  private static BridgeError error(BridgeError.Kind kind, String message) {
    return error(kind, message, message);
  }

  private static BridgeError error(BridgeError.Kind kind, String message, String detail) {
    return BridgeError.newBuilder()
        .setKind(kind)
        .setMessage(bound(message))
        .setDetail(bound(detail))
        .build();
  }

  /** Collapses control whitespace and truncates to the diagnostic bound. */
  private static String bound(String text) {
    if (text == null) {
      return "";
    }
    var collapsed = text.replace('\r', ' ').replace('\n', ' ').replace('\t', ' ').strip();
    if (collapsed.length() <= MAX_ERROR_TEXT) {
      return collapsed;
    }
    return collapsed.substring(0, MAX_ERROR_TEXT - 3) + "...";
  }

  /**
   * The throwable's message followed by the distinct messages of its causes, so a wrapped Xerces
   * diagnostic (and the offending pattern it quotes) is not lost behind a null or generic outer
   * message. The result is bounded by {@link #bound} when it reaches the wire.
   */
  static String withCauses(Throwable t) {
    var text = new StringBuilder();
    var depth = 0;
    for (var cause = t; cause != null && depth < 8; cause = cause.getCause(), depth++) {
      var message = cause.getMessage();
      // Substring "already covered" check, not equality: a cause whose text is
      // entirely present in an accumulated message adds nothing. A short cause
      // that happens to be a substring of unrelated text would be dropped; if
      // that ever happens, switch to exact equality.
      if (message == null || message.isBlank() || text.indexOf(message) >= 0) {
        continue;
      }
      if (!text.isEmpty()) {
        text.append(" | ");
      }
      text.append(message);
    }
    return text.isEmpty() ? t.getClass().getSimpleName() : text.toString();
  }

  /** A bounded, safe stand-in for an untrusted key in a diagnostic. */
  static String snippet(String text) {
    if (text == null) {
      return "";
    }
    var collapsed = text.replace('\r', ' ').replace('\n', ' ').replace('\t', ' ').strip();
    if (collapsed.length() <= MAX_SNIPPET_TEXT) {
      return collapsed;
    }
    return collapsed.substring(0, MAX_SNIPPET_TEXT - 3) + "...";
  }

  @Override
  @Blocking
  public Uni<RegisterSchemaResponse> registerSchema(RegisterSchemaRequest req) {
    return Uni.createFrom()
        .item(
            () -> {
              var schemaId = req.getSchemaId();
              try {
                registerSchemaSet(schemaId, req);
                return RegisterSchemaResponse.newBuilder().setSchemaId(schemaId).build();
              } catch (SchemaSetCompiler.SchemaSetException e) {
                return registerError(schemaId, e.kind(), e.getMessage(), e.detail());
              } catch (SchemaSetCompiler.ResourceRefusedException e) {
                return registerError(
                    schemaId,
                    BridgeError.Kind.COMPILATION_FAILED,
                    "SCHEMA_PARSE_ERROR: " + e.getMessage(),
                    null);
              } catch (SAXException e) {
                return registerError(
                    schemaId,
                    BridgeError.Kind.COMPILATION_FAILED,
                    "SCHEMA_PARSE_ERROR: " + refusalMessage(e),
                    null);
              } catch (Exception e) {
                var refusal = refusalCause(e);
                if (refusal != null) {
                  return registerError(
                      schemaId,
                      BridgeError.Kind.COMPILATION_FAILED,
                      "SCHEMA_PARSE_ERROR: " + refusal.getMessage(),
                      null);
                }
                return registerError(
                    schemaId,
                    BridgeError.Kind.INTERNAL,
                    "registerSchema failed: " + withCauses(e),
                    null);
              }
            });
  }

  private static RegisterSchemaResponse registerError(
      String schemaId, BridgeError.Kind kind, String message, String detail) {
    return RegisterSchemaResponse.newBuilder()
        .setSchemaId(schemaId)
        .setError(detail == null ? error(kind, message) : error(kind, message, detail))
        .build();
  }

  /**
   * Registers a decoded request. Empty {@code resources} keeps the legacy single-blob contract
   * keyed by {@code SHA-256(schema)}; a transmitted set is decoded, capped, validated, digest-keyed
   * by the full schema set, and served by an in-memory resolver only. The compile happens outside
   * every monitor via single-flight. The publish section holds {@link #registryLock} and re-asserts
   * the pin with {@code putAndPin} together with the mapping swap and unreferenced eviction, so a
   * racing replacement cannot strip the pin before this id is visible.
   */
  private void registerSchemaSet(String schemaId, RegisterSchemaRequest req) throws Exception {
    // A decoded set is any request carrying transmitted resources or aliases;
    // only a request that carries neither keeps the legacy single-blob contract
    // keyed by SHA-256(schema). This way an alias-map cap violation is still
    // rejected when the (untrusted) request omits resources.
    if (req.getResourcesCount() == 0 && req.getAliasesCount() == 0) {
      var schemaBytes = req.getSchema().toByteArray();
      var digest = SchemaSetCompiler.sha256Hex(schemaBytes);
      var schema = getOrCompilePinned(digest, () -> compileLegacySchema(schemaBytes));
      synchronized (registryLock) {
        // Re-assert pin and presence atomically with the map swap: a racing
        // replacement's evictIfUnreferenced either already ran (and this
        // re-inserts the retained Schema) or sees the new mapping and keeps it.
        schemaCache.putAndPin(digest, schema);
        var previous = schemaHashById.put(schemaId, digest);
        evictIfUnreferenced(previous, digest);
      }
      return;
    }

    var set = SchemaSetCompiler.decodeSuppliedSet(req);
    SchemaSetCompiler.preparseForDoctype(set.resources());
    var digest =
        SchemaSetCompiler.schemaSetDigest(set.rootSystemId(), set.resources(), set.aliases());
    var schema = getOrCompilePinned(digest, () -> compileSuppliedSet(set));
    synchronized (registryLock) {
      // Same two-level pin as the legacy branch: the compile path pinned for
      // bound-eviction safety, publication re-pins atomically with the swap.
      schemaCache.putAndPin(digest, schema);
      var previous = schemaHashById.put(schemaId, digest);
      evictIfUnreferenced(previous, digest);
    }
  }

  /** Test seam: compiles a decoded supplied set. */
  Schema compileSuppliedSet(SchemaSetCompiler.SuppliedSet set) throws Exception {
    return SchemaSetCompiler.compileSupplied(set);
  }

  /** Test seam: compiles a legacy single-blob schema. */
  Schema compileLegacySchema(byte[] schemaBytes) throws Exception {
    return SchemaSetCompiler.compileLegacy(schemaBytes);
  }

  /**
   * Returns the compiled schema for {@code digest}, compiling it outside every monitor when absent.
   * The cache insert and pin happen in one {@link SchemaCache} critical section ({@code
   * putAndPin}), so a freshly compiled entry is never evictable before publication; the caller
   * re-pins the returned {@link Schema} under {@link #registryLock} at publication. A failed
   * compile leaves no pin, no map change, and no cache entry. Concurrent same-digest registrations
   * compile once: the first caller becomes the leader and joiners await its future. After winning
   * leadership the leader rechecks the cache, because a flight that completed and was removed from
   * the map between this caller's initial miss and its {@code putIfAbsent} must not be recompiled.
   */
  private Schema getOrCompilePinned(String digest, SchemaCache.SchemaCompiler compiler)
      throws Exception {
    var cached = schemaCache.getAndPin(digest);
    if (cached != null) {
      return cached;
    }
    var flight = new CompletableFuture<Schema>();
    var leader = inFlightCompiles.putIfAbsent(digest, flight);
    if (leader != null) {
      var joined = joinFlight(leader);
      schemaCache.putAndPin(digest, joined);
      return joined;
    }
    try {
      var cachedAfterLeadership = schemaCache.getAndPin(digest);
      if (cachedAfterLeadership != null) {
        flight.complete(cachedAfterLeadership);
        return cachedAfterLeadership;
      }
      var compiled = compiler.compile();
      schemaCache.putAndPin(digest, compiled);
      flight.complete(compiled);
      return compiled;
    } catch (Exception | Error t) {
      flight.completeExceptionally(t);
      throw t;
    } finally {
      inFlightCompiles.remove(digest, flight);
    }
  }

  /**
   * Waits for the leader's compile. Joiners re-pin the joined value with {@code putAndPin}, which
   * covers the rare evicted-between-completion-and-join window by re-inserting it.
   */
  private static Schema joinFlight(CompletableFuture<Schema> leader) throws Exception {
    try {
      return leader.get();
    } catch (ExecutionException e) {
      var cause = e.getCause();
      if (cause instanceof Exception ex) {
        throw ex;
      }
      throw (Error) cause;
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      throw new IllegalStateException("interrupted while waiting for schema compile", e);
    }
  }

  /**
   * Atomically captures the public id's digest and its compiled schema. A concurrent replacement
   * can then evict the old digest without racing this read; the returned {@link Schema} stays valid
   * because validation only ever holds this immutable strong reference. Returns {@code null} for an
   * unknown id, and a snapshot with a {@code null} schema when the mapping exists but the compiled
   * entry is missing.
   */
  SchemaSnapshot snapshotFor(String schemaId) {
    synchronized (registryLock) {
      var digest = schemaHashById.get(schemaId);
      if (digest == null) {
        return null;
      }
      return new SchemaSnapshot(digest, schemaCache.get(digest));
    }
  }

  /** The real registry monitor, exposed package-private so a test can coordinate a replacement. */
  Object registryLock() {
    return registryLock;
  }

  /** One atomic view of the registry: the mapped digest and its compiled schema. */
  record SchemaSnapshot(String digest, Schema schema) {}

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
              var snapshot = snapshotFor(req.getSchemaId());
              if (snapshot == null) {
                return ValidateResponse.newBuilder()
                    .setValid(false)
                    .setError(
                        error(
                            BridgeError.Kind.RESOURCE_NOT_FOUND,
                            "unknown schema_id: " + req.getSchemaId()))
                    .build();
              }
              if (snapshot.schema() == null) {
                return ValidateResponse.newBuilder()
                    .setValid(false)
                    .setError(
                        error(
                            BridgeError.Kind.RESOURCE_NOT_FOUND,
                            "schema not loaded for schema_id: " + req.getSchemaId()))
                    .build();
              }

              // Released the registry lock above: the immutable Schema is safe to
              // use even if a concurrent replacement evicts the digest now.
              return validateAgainstSchema(snapshot.schema(), req.getDocument().toByteArray());
            });
  }

  @Override
  @Blocking
  public Uni<UnregisterSchemaResponse> unregisterSchema(UnregisterSchemaRequest req) {
    return Uni.createFrom()
        .item(
            () -> {
              synchronized (registryLock) {
                var digest = schemaHashById.remove(req.getSchemaId());
                if (digest == null) {
                  return UnregisterSchemaResponse.newBuilder().setReleased(false).build();
                }
                // Only drop the compiled entry once no other public id still
                // references the same digest. Unpin first to release pinned-set
                // membership; the guard inside unpin makes the call idempotent
                // and prevents duplicate eviction-queue entries.
                if (!schemaHashById.containsValue(digest)) {
                  schemaCache.unpin(digest);
                  schemaCache.remove(digest);
                }
                return UnregisterSchemaResponse.newBuilder().setReleased(true).build();
              }
            });
  }

  private void evictIfUnreferenced(String previous, String current) {
    if (previous != null && !previous.equals(current) && !schemaHashById.containsValue(previous)) {
      schemaCache.unpin(previous);
      schemaCache.remove(previous);
    }
  }

  private ValidateResponse validateInternal(byte[] schemaBytes, byte[] documentBytes) {
    try {
      var hash = SchemaSetCompiler.sha256Hex(schemaBytes);
      var schema = getOrCompileSchema(hash, schemaBytes);
      return validateAgainstSchema(schema, documentBytes);
    } catch (SAXException e) {
      return ValidateResponse.newBuilder()
          .setValid(false)
          .setError(
              error(BridgeError.Kind.COMPILATION_FAILED, "SCHEMA_PARSE_ERROR: " + withCauses(e)))
          .build();
    } catch (Exception e) {
      return ValidateResponse.newBuilder()
          .setValid(false)
          .setError(error(BridgeError.Kind.INTERNAL, "validate failed: " + withCauses(e)))
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
    synchronized (registryLock) {
      var existing = schemaCache.get(hash);
      if (existing != null) {
        return existing;
      }
      var compiled = SchemaSetCompiler.compileLegacy(schemaBytes);
      schemaCache.put(hash, compiled);
      return compiled;
    }
  }

  private static Source secureSaxSource(byte[] xmlBytes) throws Exception {
    return new SAXSource(
        SchemaSetCompiler.newHardenedReader(), new InputSource(new ByteArrayInputStream(xmlBytes)));
  }

  private static SchemaSetCompiler.ResourceRefusedException refusalCause(Throwable t) {
    for (Throwable cause = t; cause != null; cause = cause.getCause()) {
      if (cause instanceof SchemaSetCompiler.ResourceRefusedException refusal) {
        return refusal;
      }
    }
    return null;
  }

  private static String refusalMessage(SAXException e) {
    var refusal = refusalCause(e);
    return refusal != null && refusal.getMessage() != null ? refusal.getMessage() : withCauses(e);
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
