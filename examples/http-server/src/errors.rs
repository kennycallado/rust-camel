use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<u16>,
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            error: message.into(),
            details: None,
            code: Some(400),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            error: message.into(),
            details: None,
            code: Some(404),
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            error: message.into(),
            details: None,
            code: Some(409),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            error: message.into(),
            details: None,
            code: Some(500),
        }
    }

    #[allow(dead_code)]
    pub fn with_details(mut self, details: Vec<String>) -> Self {
        self.details = Some(details);
        self
    }
}

#[allow(dead_code)]
pub fn validation_error(message: impl Into<String>) -> ApiError {
    ApiError::bad_request(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_error_serialization() {
        let error = ApiError::not_found("User not found");
        let json = serde_json::to_string(&error).unwrap();
        assert!(json.contains("User not found"));
        assert!(json.contains("\"code\":404"));
    }

    #[test]
    fn test_api_error_with_details() {
        let error = ApiError::bad_request("Validation failed")
            .with_details(vec!["Name is required".to_string()]);
        let json = serde_json::to_string(&error).unwrap();
        assert!(json.contains("details"));
    }
}
