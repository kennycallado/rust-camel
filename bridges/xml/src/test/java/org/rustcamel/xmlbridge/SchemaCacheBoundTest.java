package org.rustcamel.xmlbridge;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
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
}
