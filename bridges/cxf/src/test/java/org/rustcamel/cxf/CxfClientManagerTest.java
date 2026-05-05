package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.mockito.Mock;
import org.mockito.MockitoAnnotations;

@DisplayName("CxfClientManager")
class CxfClientManagerTest {

  @Mock BridgeConfig bridgeConfig;
  @Mock SecurityProfileStore profileStore;

  CxfClientManager manager;

  @BeforeEach
  void setUp() {
    MockitoAnnotations.openMocks(this);
    when(bridgeConfig.connectionTimeoutMs()).thenReturn(30000);

    manager = new CxfClientManager();
    manager.bridgeConfig = bridgeConfig;
    manager.profileStore = profileStore;
  }

  @Test
  @DisplayName("cacheSize returns 0 initially")
  void cacheSizeReturnsZeroInitially() {
    assertEquals(0, manager.cacheSize());
  }

  @Test
  @DisplayName("getDispatch with unknown profile throws")
  void getDispatchWithUnknownProfileThrows() {
    when(profileStore.getProfile("unknown"))
        .thenThrow(new IllegalArgumentException("Unknown security profile: unknown"));

    assertThrows(
        IllegalArgumentException.class,
        () ->
            manager.getDispatch(
                "/fake.wsdl", "http://localhost:8080", "Svc", "Port", "op", "unknown"));
  }

  @Test
  @DisplayName("profiles is the injected profileStore")
  void profileStoreIsInjected() {
    // Verify the manager references the mock profileStore
    assertNotNull(manager.profileStore);
    assertSame(profileStore, manager.profileStore);
  }

  @Test
  @DisplayName("bridgeConfig is the injected config")
  void bridgeConfigIsInjected() {
    assertNotNull(manager.bridgeConfig);
    assertSame(bridgeConfig, manager.bridgeConfig);
  }
}
