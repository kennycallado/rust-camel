package org.rustcamel.jms;

import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;
import java.util.Map;
import java.util.logging.Level;
import java.util.logging.Logger;
import javax.jms.BytesMessage;
import javax.jms.Connection;
import javax.jms.Destination;
import javax.jms.JMSException;
import javax.jms.Message;
import javax.jms.MessageProducer;
import javax.jms.Session;
import javax.jms.TextMessage;

@ApplicationScoped
public class JmsProducer {
    @Inject JmsClientFactory factory;
    private static final Logger LOG = Logger.getLogger(JmsProducer.class.getName());

    public String send(String destination, byte[] body, Map<String, String> headers, String contentType) throws JMSException {
        try (Connection conn = factory.createConnection();
             Session session = conn.createSession(false, Session.AUTO_ACKNOWLEDGE)) {
            conn.start();
            Destination dest = parseDestination(session, destination);
            MessageProducer producer = session.createProducer(dest);

            Message msg;
            if (contentType != null && contentType.startsWith("text/")) {
                TextMessage tm = session.createTextMessage(new String(body, java.nio.charset.StandardCharsets.UTF_8));
                msg = tm;
            } else {
                BytesMessage bm = session.createBytesMessage();
                bm.writeBytes(body);
                msg = bm;
            }

            if (contentType != null && !contentType.isEmpty()) {
                msg.setStringProperty("Content-Type", contentType);
            }
            for (var entry : headers.entrySet()) {
                String key = entry.getKey();
                if (key.startsWith("JMS") && !key.startsWith("JMSX")) continue;
                try {
                    msg.setStringProperty(key, entry.getValue());
                } catch (Exception e) {
                    if (LOG.isLoggable(Level.FINE)) {
                        LOG.fine("Failed to set JMS property '" + key + "': " + e.getMessage());
                    }
                }
            }
            producer.send(msg);
            return msg.getJMSMessageID();
        }
    }

    static Destination parseDestination(Session session, String dest) throws JMSException {
        if (dest.startsWith("queue:")) return session.createQueue(dest.substring(6));
        if (dest.startsWith("topic:")) return session.createTopic(dest.substring(6));
        return session.createQueue(dest);
    }
}
