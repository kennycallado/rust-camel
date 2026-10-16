package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Pins the settlement contract of rc-47gr5: {@code acknowledge()} returns only after the broker
 * applied the ack, so the journal COMMIT implies a broker-side ack. Both provider defaults are
 * asynchronous and would let a crash after COMMIT redeliver with the reservation already cleared.
 */
@DisplayName("JmsClientFactory synchronous acknowledgement")
class JmsClientFactorySyncAckTest {

  @Test
  void activemq_classic_sends_acks_synchronously() {
    var cf = new JmsClientFactory().buildActiveMqFactory("tcp://localhost:61616", null, null);
    assertFalse(cf.isSendAcksAsync());
  }

  @Test
  void artemis_blocks_on_acknowledge() throws Exception {
    var cf = JmsClientFactory.buildArtemisFactory("tcp://localhost:61616", null, null);
    try {
      assertTrue(cf.isBlockOnAcknowledge());
    } finally {
      cf.close();
    }
  }
}
