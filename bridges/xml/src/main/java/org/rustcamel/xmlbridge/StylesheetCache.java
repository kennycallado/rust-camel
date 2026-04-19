package org.rustcamel.xmlbridge;

import jakarta.enterprise.context.ApplicationScoped;
import java.util.concurrent.ConcurrentHashMap;
import javax.xml.transform.Templates;

@ApplicationScoped
public class StylesheetCache {
  private final ConcurrentHashMap<String, Templates> byId = new ConcurrentHashMap<>();

  public Templates get(String id) {
    return byId.get(id);
  }

  public void put(String id, Templates x) {
    byId.put(id, x);
  }

  public boolean remove(String id) {
    return byId.remove(id) != null;
  }
}
