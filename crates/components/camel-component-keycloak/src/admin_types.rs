use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeycloakUser {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(rename = "firstName", skip_serializing_if = "Option::is_none")]
    pub first_name: Option<String>,
    #[serde(rename = "lastName", skip_serializing_if = "Option::is_none")]
    pub last_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credentials: Option<Vec<KeycloakCredential>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct KeycloakCredential {
    #[serde(rename = "type")]
    pub cred_type: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporary: Option<bool>,
}

impl std::fmt::Debug for KeycloakCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeycloakCredential")
            .field("type", &self.cred_type)
            .field("value", &"REDACTED")
            .field("temporary", &self.temporary)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeycloakRole {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct KeycloakClient {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

impl std::fmt::Debug for KeycloakClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let secret_display = if self.secret.is_some() {
            "REDACTED"
        } else {
            "None"
        };
        f.debug_struct("KeycloakClient")
            .field("id", &self.id)
            .field("client_id", &self.client_id)
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("secret", &secret_display)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeycloakRealm {
    pub realm: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keycloak_user_serialize_minimal() {
        let user = KeycloakUser {
            id: None,
            username: Some("testuser".into()),
            email: None,
            enabled: None,
            first_name: None,
            last_name: None,
            credentials: None,
        };
        let json = serde_json::to_string(&user).unwrap();
        assert_eq!(json, "{\"username\":\"testuser\"}");
    }

    #[test]
    fn keycloak_user_deserialize_full() {
        let json = serde_json::json!({
            "id": "user-uuid-123",
            "username": "testuser",
            "email": "test@example.com",
            "enabled": true,
            "firstName": "Test",
            "lastName": "User",
            "credentials": [{
                "type": "password",
                "value": "secret123",
                "temporary": false
            }]
        });
        let user: KeycloakUser = serde_json::from_value(json).unwrap();
        assert_eq!(user.id.as_deref(), Some("user-uuid-123"));
        assert_eq!(user.username.as_deref(), Some("testuser"));
        assert_eq!(user.email.as_deref(), Some("test@example.com"));
        assert_eq!(user.enabled, Some(true));
        assert_eq!(user.first_name.as_deref(), Some("Test"));
        assert_eq!(user.last_name.as_deref(), Some("User"));
        let creds = user.credentials.unwrap();
        assert_eq!(creds.len(), 1);
        assert_eq!(creds[0].cred_type, "password");
        assert_eq!(creds[0].value, "secret123");
        assert_eq!(creds[0].temporary, Some(false));
    }

    #[test]
    fn keycloak_role_serialize() {
        let role = KeycloakRole {
            id: None,
            name: "admin".into(),
            description: None,
        };
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "{\"name\":\"admin\"}");
    }

    #[test]
    fn keycloak_client_serialize() {
        let client = KeycloakClient {
            id: None,
            client_id: Some("my-app".into()),
            name: None,
            enabled: None,
            secret: Some("abc-123".into()),
        };
        let json = serde_json::to_string(&client).unwrap();
        assert_eq!(json, "{\"client_id\":\"my-app\",\"secret\":\"abc-123\"}");
    }
}
