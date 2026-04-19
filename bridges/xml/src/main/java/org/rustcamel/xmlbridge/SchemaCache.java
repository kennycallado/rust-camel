package org.rustcamel.xmlbridge;

import jakarta.enterprise.context.ApplicationScoped;
import java.util.concurrent.ConcurrentHashMap;
import javax.xml.validation.Schema;

@ApplicationScoped
public class SchemaCache {
  private final ConcurrentHashMap<String, Schema> byId = new ConcurrentHashMap<>();

  public Schema get(String id) {
    return byId.get(id);
  }

  public void put(String id, Schema s) {
    byId.put(id, s);
  }

  public boolean remove(String id) {
    return byId.remove(id) != null;
  }

  public int size() {
    return byId.size();
  }
}
