package org.rustcamel.xmlbridge;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNotSame;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.charset.StandardCharsets;
import org.junit.jupiter.api.Test;

/**
 * Bounded, pin-aware eviction behaviour of {@link SchemaCache}. Plain JUnit: the cache is
 * constructed directly, no Quarkus context is booted. Cached values are genuine {@code Schema}
 * instances compiled from minimal in-memory XSDs.
 */
class SchemaCacheBoundTest {

  /** A compiler that produces a real, distinct schema per namespace. */
  private static SchemaCache.SchemaCompiler compilerFor(String namespace) {
    var xsd =
        ("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"
                + "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\""
                + " targetNamespace=\"urn:"
                + namespace
                + "\" xmlns:tns=\"urn:"
                + namespace
                + "\" elementFormDefault=\"qualified\">"
                + "<xs:element name=\"root\" type=\"tns:rootType\"/>"
                + "<xs:complexType name=\"rootType\"><xs:sequence>"
                + "<xs:element name=\"value\" type=\"xs:string\"/>"
                + "</xs:sequence></xs:complexType>"
                + "</xs:schema>")
            .getBytes(StandardCharsets.UTF_8);
    return () -> SchemaSetCompiler.compileLegacy(xsd);
  }

  @Test
  void inlineInsertsBeyondBoundStayBounded() throws Exception {
    var cache = new SchemaCache(4);
    for (int i = 0; i < 6; i++) {
      cache.getOrCompile("digest-" + i, compilerFor("ns" + i));
    }
    assertTrue(cache.size() <= 4, "cache must stay at or below its bound, was " + cache.size());
    assertTrue(
        cache.evictions() >= 2, "at least two inserts must have evicted, was " + cache.evictions());
  }

  @Test
  void pinnedDigestSurvivesEvictionPressure() throws Exception {
    var cache = new SchemaCache(2);
    cache.pin("A");
    cache.getOrCompile("A", compilerFor("a"));
    cache.getOrCompile("B", compilerFor("b"));
    cache.getOrCompile("C", compilerFor("c"));
    cache.getOrCompile("D", compilerFor("d"));
    assertNotNull(cache.get("A"), "pinned digest A must never be evicted");
    assertTrue(cache.size() <= 2, "cache must stay at or below its bound, was " + cache.size());
  }

  @Test
  void unpinnedAfterUnpinBecomesEvictable() throws Exception {
    var cache = new SchemaCache(2);
    cache.pin("A");
    cache.getOrCompile("A", compilerFor("a"));
    cache.getOrCompile("B", compilerFor("b"));
    cache.unpin("A");
    cache.getOrCompile("C", compilerFor("c"));
    cache.getOrCompile("D", compilerFor("d"));
    assertNull(cache.get("A"), "unpinned digest A must become evictable");
    assertTrue(cache.size() <= 2, "cache must stay at or below its bound, was " + cache.size());
  }

  @Test
  void inlineInsertDoesNotDowngradePin() throws Exception {
    var cache = new SchemaCache(2);
    cache.pin("A");
    cache.getOrCompile("A", compilerFor("a"));
    // Second call hits the existing entry and short-circuits (it never reaches the
    // enqueue branch): this asserts the hit path preserves the pinned entry.
    cache.getOrCompile("A", compilerFor("a"));
    cache.getOrCompile("B", compilerFor("b"));
    cache.getOrCompile("C", compilerFor("c"));
    assertNotNull(cache.get("A"), "a repeated inline insert must not downgrade the pin on A");
  }

  @Test
  void justInsertedHeadBreakKeepsQueueIntact() throws Exception {
    var cache = new SchemaCache(1);
    cache.pin("A");
    cache.getOrCompile("A", compilerFor("a"));
    cache.getOrCompile("B", compilerFor("b"));
    cache.getOrCompile("C", compilerFor("c"));
    cache.unpin("A");
    cache.getOrCompile("D", compilerFor("d"));
    assertEquals(1, cache.size(), "cache must return to its bound once a pinned entry unpins");
    assertNull(cache.get("B"), "B must be evicted");
    assertNull(cache.get("A"), "A must be evicted after unpin");
  }

  @Test
  void nonPositiveBoundRejected() {
    var zero = assertThrows(IllegalArgumentException.class, () -> new SchemaCache(0));
    assertTrue(
        zero.getMessage().contains("xml-bridge.schema-cache.max-entries"),
        "startup failure must name the property, was: " + zero.getMessage());

    var negative = assertThrows(IllegalArgumentException.class, () -> new SchemaCache(-1));
    assertTrue(
        negative.getMessage().contains("xml-bridge.schema-cache.max-entries"),
        "startup failure must name the property, was: " + negative.getMessage());
  }

  @Test
  void evictionCounterIncreasesMonotonically() throws Exception {
    var cache = new SchemaCache(1);
    cache.getOrCompile("X", compilerFor("x"));
    long before = cache.evictions();
    cache.getOrCompile("Y", compilerFor("y"));
    assertTrue(cache.evictions() > before, "an eviction must raise the counter");
    assertEquals(1, cache.size());
  }

  @Test
  void removeDropsDigestFromEvictionQueue() throws Exception {
    var cache = new SchemaCache(4);
    cache.getOrCompile("X", compilerFor("x"));
    cache.remove("X");
    cache.getOrCompile("Y", compilerFor("y"));
    cache.getOrCompile("Z", compilerFor("z"));
    cache.getOrCompile("W", compilerFor("w"));
    assertEquals(3, cache.size(), "a removed digest must not distort later eviction");
  }

  @Test
  void putAndPinInsertsPinnedAndBounded() throws Exception {
    var cache = new SchemaCache(2);
    cache.put("A", compilerFor("a").compile());
    cache.put("B", compilerFor("b").compile());
    cache.putAndPin("C", compilerFor("c").compile());
    assertEquals(2, cache.size(), "cache must stay at its bound, was " + cache.size());
    assertEquals(1, cache.evictions(), "the oldest unpinned entry must be evicted");
    assertNotNull(cache.get("C"), "the pinned insert must be cached");
    assertTrue(cache.isPinned("C"), "putAndPin must pin the inserted digest");
    assertNull(cache.get("A"), "the unpinned FIFO head must be the eviction victim");
  }

  @Test
  void putAndPinAllPinnedOvershoots() throws Exception {
    var cache = new SchemaCache(1);
    cache.putAndPin("A", compilerFor("a").compile());
    cache.putAndPin("B", compilerFor("b").compile());
    assertNotNull(cache.get("A"), "pinned entry A must survive");
    assertNotNull(cache.get("B"), "pinned entry B must survive");
    assertEquals(2, cache.size(), "all-pinned cache must overshoot transiently");
  }

  @Test
  void putAndPinRefreshesExistingAndKeepsPin() throws Exception {
    var cache = new SchemaCache(2);
    cache.putAndPin("A", compilerFor("a").compile());
    var s1 = cache.get("A");
    cache.putAndPin("A", compilerFor("a").compile());
    assertNotSame(s1, cache.get("A"), "an existing digest must be value-refreshed");
    assertEquals(1, cache.size(), "a refresh must not duplicate the entry");
    assertTrue(cache.isPinned("A"), "a refresh must keep the digest pinned");
    cache.put("B", compilerFor("b").compile());
    cache.put("C", compilerFor("c").compile());
    assertNotNull(cache.get("A"), "pin must survive eviction pressure");
    assertTrue(cache.size() <= 2, "cache must stay at or below its bound, was " + cache.size());
    assertNull(cache.get("B"), "the unpinned FIFO victim must be B");
  }

  @Test
  void getAndPinPinsExisting() throws Exception {
    var cache = new SchemaCache(2);
    cache.putAndPin("A", compilerFor("a").compile());
    cache.unpin("A");
    var s = cache.getAndPin("A");
    assertNotNull(s, "getAndPin must return the cached schema");
    assertTrue(cache.isPinned("A"), "getAndPin must pin the digest");
    cache.getOrCompile("B", compilerFor("b"));
    cache.getOrCompile("C", compilerFor("c"));
    assertNotNull(cache.get("A"), "the pin taken by getAndPin must protect A");
    assertTrue(cache.size() <= 2, "cache must stay at or below its bound, was " + cache.size());
  }

  @Test
  void getAndPinReturnsNullWhenAbsent() {
    var cache = new SchemaCache(2);
    assertNull(cache.getAndPin("missing"), "getAndPin must return null for an absent digest");
    assertTrue(!cache.isPinned("missing"), "getAndPin must not pin an absent digest");
  }
}
