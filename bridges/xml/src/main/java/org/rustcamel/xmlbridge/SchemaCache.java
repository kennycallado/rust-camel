package org.rustcamel.xmlbridge;

import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;
import java.util.ArrayDeque;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import javax.xml.validation.Schema;
import org.eclipse.microprofile.config.inject.ConfigProperty;
import org.jboss.logging.Logger;

/**
 * Compiled-schema store keyed by schema-set digest, never by the untrusted public {@code
 * schema_id}. Two public ids that register the same schema set share one compiled entry, and
 * eviction is reference-counted by the caller: the digest is removed only when no public id still
 * maps to it.
 *
 * <p>The digest covers the root systemId, every resource key and its bytes, and the alias map, so a
 * changed child rotates the entry.
 *
 * <p>The cache is bounded by {@code xml-bridge.schema-cache.max-entries} (default 256). When the
 * total entry count exceeds the bound, eviction removes the oldest-inserted <em>unpinned</em> entry
 * (FIFO over the unpinned population — no reorder on read, so the {@link #get} hot path stays a
 * plain {@code ConcurrentHashMap.get}). Entries held by a live registration are pinned via {@link
 * #pin} and are never evicted: if every entry is pinned the cache overshoots the bound transiently
 * rather than dropping a schema still in use. A non-positive bound is invalid and fails bean
 * creation at startup.
 */
@ApplicationScoped
public class SchemaCache {
  private static final Logger LOG = Logger.getLogger(SchemaCache.class);

  private final ConcurrentHashMap<String, Schema> byDigest = new ConcurrentHashMap<>();

  /** Unpinned digests in insertion order, oldest at the head; eviction candidates. */
  private final ArrayDeque<String> unpinnedOrder = new ArrayDeque<>();

  /** Digests held by a live registration; never evicted. */
  private final Set<String> pinned = ConcurrentHashMap.newKeySet();

  private final int maxEntries;
  private long evictions = 0;

  @Inject
  SchemaCache(
      @ConfigProperty(name = "xml-bridge.schema-cache.max-entries", defaultValue = "256")
          int maxEntries) {
    if (maxEntries <= 0) {
      throw new IllegalArgumentException(
          "xml-bridge.schema-cache.max-entries must be positive, got " + maxEntries);
    }
    this.maxEntries = maxEntries;
  }

  /** Returns the compiled schema for a digest, or {@code null} when absent. */
  public Schema get(String digest) {
    return byDigest.get(digest);
  }

  /**
   * Returns the compiled schema for {@code digest} and pins it in one critical section, or {@code
   * null} when absent. Closes the get-then-pin window a plain {@link #get} plus {@link #pin} pair
   * would leave open to a concurrent eviction.
   */
  public synchronized Schema getAndPin(String digest) {
    var schema = byDigest.get(digest);
    if (schema != null) {
      pinned.add(digest);
      unpinnedOrder.remove(digest);
    }
    return schema;
  }

  /** Stores a compiled schema under its digest, evicting if the bound is exceeded. */
  public synchronized void put(String digest, Schema schema) {
    if (byDigest.putIfAbsent(digest, schema) != null) {
      // Existing key: refresh the value only, never re-enqueue or evict.
      byDigest.put(digest, schema);
      return;
    }
    if (!pinned.contains(digest)) {
      unpinnedOrder.addLast(digest);
    }
    evictUntilBounded(digest);
  }

  /**
   * Inserts and pins {@code digest} in one critical section, so a freshly compiled entry is never
   * visible unpinned and no concurrent insert can evict it before its registration publishes the
   * id-to-digest mapping. When the digest is already cached its value is refreshed (the same digest
   * implies the same schema set) and it stays pinned.
   */
  public synchronized void putAndPin(String digest, Schema schema) {
    if (byDigest.putIfAbsent(digest, schema) != null) {
      byDigest.put(digest, schema); // existing key: refresh value only
    }
    pinned.add(digest);
    unpinnedOrder.remove(digest);
    evictUntilBounded(digest);
  }

  /** Removes a compiled schema by digest, returning whether one was present. */
  public synchronized boolean remove(String digest) {
    boolean present = byDigest.remove(digest) != null;
    unpinnedOrder.remove(digest);
    return present;
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
    if (!pinned.contains(digest)) {
      unpinnedOrder.addLast(digest);
    }
    evictUntilBounded(digest);
    return compiled;
  }

  /** Pins {@code digest} so it can never be evicted; removes it from the eviction queue. */
  public synchronized void pin(String digest) {
    pinned.add(digest);
    unpinnedOrder.remove(digest);
  }

  /**
   * Unpins {@code digest}. If it is still cached it rejoins the tail of the eviction queue, since
   * unpinning is rare and appending preserves oldest-inserted-first order.
   */
  public synchronized void unpin(String digest) {
    // Only a digest that was actually pinned rejoins the eviction queue; otherwise
    // unpinning an unpinned digest would enqueue a duplicate candidate.
    if (pinned.remove(digest) && byDigest.containsKey(digest)) {
      unpinnedOrder.addLast(digest);
    }
  }

  /** Number of evictions performed since construction (test observability). */
  public synchronized long evictions() {
    return evictions;
  }

  /**
   * Whether {@code digest} is currently pinned (test observability; mirrors {@link #evictions}).
   */
  synchronized boolean isPinned(String digest) {
    return pinned.contains(digest);
  }

  /** Number of compiled entries (test observability). */
  public int size() {
    return byDigest.size();
  }

  /**
   * Evicts unpinned entries from the head of the insertion queue until the cache is within its
   * bound. The just-inserted digest is left queued (never dropped) so a subsequent eviction can
   * still reclaim it; if the queue empties or only the just-inserted entry remains, the loop stops
   * and the bound is overshot transiently because no older unpinned candidate exists.
   */
  private void evictUntilBounded(String justInserted) {
    while (byDigest.size() > maxEntries) {
      var head = unpinnedOrder.peekFirst();
      if (head == null || head.equals(justInserted)) {
        break;
      }
      unpinnedOrder.pollFirst();
      if (!byDigest.containsKey(head) || pinned.contains(head)) {
        continue;
      }
      byDigest.remove(head);
      evictions++;
      LOG.infof("schema cache evicted digest %s, size now %d", head, byDigest.size());
    }
  }

  /** A checked compile step supplied to {@link #getOrCompile}. */
  @FunctionalInterface
  public interface SchemaCompiler {
    Schema compile() throws Exception;
  }
}
