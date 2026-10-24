package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.times;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import java.net.StandardSocketOptions;
import java.util.List;
import javax.net.ssl.SNIHostName;
import javax.net.ssl.SSLParameters;
import javax.net.ssl.SSLSocket;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.mockito.ArgumentCaptor;

/**
 * Fluent-chain hardening on {@link ServerNamePreservingSSLSocket} (task 5.1): {@code setOption}
 * must return the wrapper, not the delegate, so a chained {@code setSSLParameters} still runs the
 * SNI/endpoint-identification correction.
 */
@DisplayName("ServerNamePreservingSSLSocket fluent chaining")
class ServerNamePreservingSSLSocketTest {

  @Test
  void setOptionReturnsWrapperSoChainedSetSslParametersKeepsSniCorrection() throws Exception {
    SSLSocket delegate = mock(SSLSocket.class);
    when(delegate.getSSLParameters()).thenAnswer(invocation -> new SSLParameters());

    ServerNamePreservingSSLSocket wrapper =
        new ServerNamePreservingSSLSocket(delegate, "broker.identity");

    SSLSocket chained = wrapper.setOption(StandardSocketOptions.SO_KEEPALIVE, true);
    assertSame(wrapper, chained, "setOption must stay on the wrapper");

    chained.setSSLParameters(new SSLParameters());

    ArgumentCaptor<SSLParameters> captor = ArgumentCaptor.forClass(SSLParameters.class);
    verify(delegate, times(3)).setSSLParameters(captor.capture());
    SSLParameters last = captor.getValue();
    assertEquals(List.of(new SNIHostName("broker.identity")), last.getServerNames());
    assertEquals("HTTPS", last.getEndpointIdentificationAlgorithm());
  }
}
