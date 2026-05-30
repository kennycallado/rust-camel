use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[allow(non_snake_case)]
pub struct EventRepresentation {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub time: Option<u64>,
    #[serde(default)]
    #[serde(rename = "type")]
    pub event_type: Option<String>,
    #[serde(default)]
    pub realmId: Option<String>,
    #[serde(default)]
    pub clientId: Option<String>,
    #[serde(default)]
    pub userId: Option<String>,
    #[serde(default)]
    pub sessionId: Option<String>,
    #[serde(default)]
    pub ipAddress: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub details: Option<std::collections::HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[allow(non_snake_case)]
pub struct AdminEventRepresentation {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub time: Option<u64>,
    #[serde(default)]
    pub realmId: Option<String>,
    #[serde(default)]
    pub authDetails: Option<AuthDetailsRepresentation>,
    #[serde(default)]
    pub operationType: Option<String>,
    #[serde(default)]
    pub resourceType: Option<String>,
    #[serde(default)]
    pub resourcePath: Option<String>,
    #[serde(default)]
    pub representation: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub details: Option<std::collections::HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[allow(non_snake_case)]
pub struct AuthDetailsRepresentation {
    #[serde(default)]
    pub realmId: Option<String>,
    #[serde(default)]
    pub clientId: Option<String>,
    #[serde(default)]
    pub userId: Option<String>,
    #[serde(default)]
    pub ipAddress: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_user_event() {
        let json = serde_json::json!({
            "id": "event-uuid-1",
            "time": 1700000000000_u64,
            "type": "LOGIN",
            "realmId": "test-realm",
            "clientId": "my-client",
            "userId": "user-123",
            "sessionId": "session-abc",
            "ipAddress": "192.168.1.1",
            "details": { "custom": "value" }
        });
        let event: EventRepresentation = serde_json::from_value(json).unwrap();
        assert_eq!(event.id.as_deref(), Some("event-uuid-1"));
        assert_eq!(event.time, Some(1700000000000));
        assert_eq!(event.event_type.as_deref(), Some("LOGIN"));
        assert_eq!(event.userId.as_deref(), Some("user-123"));
        assert_eq!(event.details.unwrap().get("custom").unwrap(), "value");
    }

    #[test]
    fn deserialize_user_event_minimal() {
        let json = serde_json::json!({});
        let event: EventRepresentation = serde_json::from_value(json).unwrap();
        assert!(event.id.is_none());
        assert!(event.time.is_none());
    }

    #[test]
    fn deserialize_admin_event() {
        let json = serde_json::json!({
            "id": "admin-event-1",
            "time": 1700000000001_u64,
            "realmId": "test-realm",
            "authDetails": {
                "realmId": "master",
                "clientId": "admin-cli",
                "userId": "admin-user",
                "ipAddress": "10.0.0.1"
            },
            "operationType": "CREATE",
            "resourceType": "USER",
            "resourcePath": "users/user-456"
        });
        let event: AdminEventRepresentation = serde_json::from_value(json).unwrap();
        assert_eq!(event.id.as_deref(), Some("admin-event-1"));
        assert_eq!(event.operationType.as_deref(), Some("CREATE"));
        assert_eq!(event.resourceType.as_deref(), Some("USER"));
        let auth = event.authDetails.unwrap();
        assert_eq!(auth.userId.as_deref(), Some("admin-user"));
    }

    #[test]
    fn deserialize_admin_event_minimal() {
        let json = serde_json::json!({});
        let event: AdminEventRepresentation = serde_json::from_value(json).unwrap();
        assert!(event.id.is_none());
        assert!(event.authDetails.is_none());
    }

    #[test]
    fn deserialize_event_array() {
        let json = serde_json::json!([
            { "id": "e1", "type": "LOGIN", "time": 100 },
            { "id": "e2", "type": "LOGOUT", "time": 200 }
        ]);
        let events: Vec<EventRepresentation> = serde_json::from_value(json).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type.as_deref(), Some("LOGIN"));
        assert_eq!(events[1].event_type.as_deref(), Some("LOGOUT"));
    }
}
