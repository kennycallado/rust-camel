use camel_api::{Exchange, Message, Value};
use rumqttc::{Publish, QoS};

/// Header: MQTT topic the message was received on.
pub const CAMEL_MQTT_TOPIC: &str = "CamelMqttTopic";
/// Header: MQTT QoS level ("0", "1", or "2").
pub const CAMEL_MQTT_QOS: &str = "CamelMqttQos";
/// Header: whether the message was retained.
pub const CAMEL_MQTT_RETAINED: &str = "CamelMqttRetained";
/// Header: whether this is a duplicate delivery.
pub const CAMEL_MQTT_DUPLICATE: &str = "CamelMqttDuplicate";
/// Header: MQTT packet ID (absent for QoS 0).
pub const CAMEL_MQTT_PACKET_ID: &str = "CamelMqttPacketId";
/// Header: client ID of the subscribing connection.
pub const CAMEL_MQTT_CLIENT_ID: &str = "CamelMqttClientId";
/// Producer outbound header: override retain flag (URI default used if absent).
pub const CAMEL_MQTT_RETAIN: &str = "CamelMqttRetain";

/// Build an [`Exchange`] from an incoming MQTT [`Publish`] packet.
///
/// Sets standard MQTT headers on the message. The `CamelMqttPacketId` header is
/// omitted for QoS 0 messages where the broker never assigns a packet ID.
pub fn build_exchange(publish: &Publish, client_id: &str) -> Exchange {
    let mut msg = Message::new(publish.payload.clone());
    msg.set_header(
        CAMEL_MQTT_TOPIC,
        Value::String(String::from_utf8_lossy(publish.topic.as_ref()).into_owned()),
    );
    msg.set_header(
        CAMEL_MQTT_QOS,
        Value::String(qos_to_str(publish.qos).to_string()),
    );
    msg.set_header(
        CAMEL_MQTT_RETAINED,
        Value::String(publish.retain.to_string()),
    );
    msg.set_header(CAMEL_MQTT_DUPLICATE, Value::String(publish.dup.to_string()));
    msg.set_header(CAMEL_MQTT_CLIENT_ID, Value::String(client_id.to_string()));
    // pkid is only meaningful for QoS 1 and 2
    if publish.qos != QoS::AtMostOnce {
        msg.set_header(
            CAMEL_MQTT_PACKET_ID,
            Value::String(publish.pkid.to_string()),
        );
    }
    Exchange::new(msg)
}

fn qos_to_str(qos: QoS) -> &'static str {
    match qos {
        QoS::AtMostOnce => "0",
        QoS::AtLeastOnce => "1",
        QoS::ExactlyOnce => "2",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::Value;
    use rumqttc::{Publish, QoS};

    fn make_publish(qos: QoS, pkid: u16) -> Publish {
        Publish {
            dup: false,
            qos,
            retain: false,
            topic: bytes::Bytes::from("sensors/temp"),
            pkid,
            payload: bytes::Bytes::from("22.5"),
        }
    }

    #[test]
    fn build_exchange_sets_required_headers() {
        let publish = make_publish(QoS::AtLeastOnce, 42);
        let exchange = build_exchange(&publish, "camel-route-a1b2c3");
        let h = &exchange.input.headers;
        assert_eq!(
            h.get(CAMEL_MQTT_TOPIC),
            Some(&Value::String("sensors/temp".into()))
        );
        assert_eq!(h.get(CAMEL_MQTT_QOS), Some(&Value::String("1".into())));
        assert_eq!(
            h.get(CAMEL_MQTT_RETAINED),
            Some(&Value::String("false".into()))
        );
        assert_eq!(
            h.get(CAMEL_MQTT_DUPLICATE),
            Some(&Value::String("false".into()))
        );
        assert_eq!(
            h.get(CAMEL_MQTT_PACKET_ID),
            Some(&Value::String("42".into()))
        );
        assert_eq!(
            h.get(CAMEL_MQTT_CLIENT_ID),
            Some(&Value::String("camel-route-a1b2c3".into()))
        );
    }

    #[test]
    fn build_exchange_no_packet_id_for_qos0() {
        let publish = make_publish(QoS::AtMostOnce, 0);
        let exchange = build_exchange(&publish, "camel-route-a1b2c3");
        assert!(exchange.input.headers.get(CAMEL_MQTT_PACKET_ID).is_none());
    }
}
