use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age: Option<u32>,
    pub role: String,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUserRequest {
    pub name: String,
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age: Option<u32>,
    #[serde(default = "default_role")]
    pub role: String,
}

fn default_role() -> String {
    "user".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateUserRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserListResponse {
    pub users: Vec<User>,
    pub total: usize,
    pub page: u32,
    pub per_page: u32,
}

impl CreateUserRequest {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("name is required and cannot be empty".to_string());
        }

        if self.name.len() > 100 {
            return Err("name must be 100 characters or less".to_string());
        }

        if self.email.trim().is_empty() {
            return Err("email is required and cannot be empty".to_string());
        }

        if !self.is_valid_email(&self.email) {
            return Err("email format is invalid".to_string());
        }

        if let Some(age) = self.age {
            if age > 150 {
                return Err("age must be 150 or less".to_string());
            }
        }

        let valid_roles = ["user", "admin", "moderator"];
        if !valid_roles.contains(&self.role.as_str()) {
            return Err(format!("role must be one of: {}", valid_roles.join(", ")));
        }

        Ok(())
    }

    fn is_valid_email(&self, email: &str) -> bool {
        email.contains('@') && email.contains('.')
    }
}

impl UpdateUserRequest {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(name) = &self.name {
            if name.trim().is_empty() {
                return Err("name cannot be empty".to_string());
            }
            if name.len() > 100 {
                return Err("name must be 100 characters or less".to_string());
            }
        }

        if let Some(email) = &self.email {
            if email.trim().is_empty() {
                return Err("email cannot be empty".to_string());
            }
            if !email.contains('@') || !email.contains('.') {
                return Err("email format is invalid".to_string());
            }
        }

        if let Some(age) = self.age {
            if age > 150 {
                return Err("age must be 150 or less".to_string());
            }
        }

        if let Some(role) = &self.role {
            let valid_roles = ["user", "admin", "moderator"];
            if !valid_roles.contains(&role.as_str()) {
                return Err(format!("role must be one of: {}", valid_roles.join(", ")));
            }
        }

        Ok(())
    }

    pub fn has_updates(&self) -> bool {
        self.name.is_some() || self.email.is_some() || self.age.is_some() || self.role.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_create_user_request() {
        let req = CreateUserRequest {
            name: "Alice".to_string(),
            email: "alice@example.com".to_string(),
            age: Some(30),
            role: "user".to_string(),
        };
        assert!(req.validate().is_ok());
    }

    #[test]
    fn test_invalid_name_empty() {
        let req = CreateUserRequest {
            name: "  ".to_string(),
            email: "test@example.com".to_string(),
            age: None,
            role: "user".to_string(),
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn test_invalid_email() {
        let req = CreateUserRequest {
            name: "Bob".to_string(),
            email: "invalid-email".to_string(),
            age: None,
            role: "user".to_string(),
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn test_invalid_role() {
        let req = CreateUserRequest {
            name: "Charlie".to_string(),
            email: "charlie@example.com".to_string(),
            age: None,
            role: "superuser".to_string(),
        };
        assert!(req.validate().is_err());
    }

    #[test]
    fn test_update_request_has_updates() {
        let req = UpdateUserRequest {
            name: Some("New Name".to_string()),
            email: None,
            age: None,
            role: None,
        };
        assert!(req.has_updates());
    }

    #[test]
    fn test_update_request_no_updates() {
        let req = UpdateUserRequest {
            name: None,
            email: None,
            age: None,
            role: None,
        };
        assert!(!req.has_updates());
    }
}
