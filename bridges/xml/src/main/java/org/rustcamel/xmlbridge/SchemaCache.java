package org.rustcamel.xmlbridge;

import jakarta.enterprise.context.ApplicationScoped;
import java.util.concurrent.ConcurrentHashMap;
import javax.xml.validation.Schema;

/**
 * Compiled-schema store keyed by schema-set digest, never by the untrusted public {@code
 * schema_id}. Two public ids that register the same schema set share one compiled entry, and
 * eviction is reference-counted by the caller: the digest is removed only when no public id still
 * maps to it.
 *
 * <p>The digest covers the root systemId, every resource key and its bytes, and the alias map, so a
 * changed child rotates the entry.
 */
@ApplicationScoped
public class SchemaCache {
  private final ConcurrentHashMap<String, Schema> byDigest = new ConcurrentHashMap<>();

  /** Returns the compiled schema for a digest, or {@code null} when absent. */
  public Schema get(String digest) {
    return byDigest.get(digest);
  }

  /** Stores a compiled schema under its digest. */
  public void put(String digest, Schema schema) {
    byDigest.put(digest, schema);
  }

  /** Removes a compiled schema by digest, returning whether one was present. */
  public boolean remove(String digest) {
    return byDigest.remove(digest) != null;
  }

  /**
   * Compiles and stores the schema for {@code digest} exactly once under the cache monitor. The
   * loader runs while the monitor is held so a compile failure leaves the cache untouched and
   * concurrent callers never compile the same digest twice.
   */
  public synchronized Schema getOrCompile(String digest, SchemaCompiler compiler) throws Exception {
    var existing = byDigest.get(digest);
    if (existing != null) {
      return existing;
    }
    var compiled = compiler.compile();
    byDigest.put(digest, compiled);
    return compiled;
  }

  /** Number of compiled entries (test observability). */
  public int size() {
    return byDigest.size();
  }

  /** A checked compile step supplied to {@link #getOrCompile}. */
  @FunctionalInterface
  public interface SchemaCompiler {
    Schema compile() throws Exception;
  }
}
