package org.rustcamel.jms;

import javax.jms.JMSException;

/**
 * A forwarded property write was rejected by the broker or its JMS client.
 *
 * <p>The message names the offending properties only. It never carries a property value, provider
 * exception text, credential material, or host/connection detail, because all of those may leak
 * message content or broker topology into caller-visible errors.
 */
public class InvalidPropertyException extends JMSException {

  private static final long serialVersionUID = 1L;

  public InvalidPropertyException(String propertyNames) {
    super("Invalid or rejected JMS property names: " + propertyNames);
  }
}
