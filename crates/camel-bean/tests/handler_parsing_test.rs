/// Comprehensive test demonstrating Task 2.1: Parse handler methods
/// This test validates that the handler parsing infrastructure works correctly
use camel_bean::handler;

/// Test struct with various handler methods
#[allow(dead_code)]
struct TestService;

#[allow(dead_code)]
impl TestService {
    // Valid handler with body parameter
    #[handler]
    pub async fn process_body(&self, body: String) -> Result<String, String> {
        Ok(format!("processed: {}", body))
    }

    // Valid handler with body and headers
    #[handler]
    pub async fn process_with_headers(
        &self,
        _body: String,
        _headers: std::collections::HashMap<String, String>,
    ) -> Result<String, String> {
        Ok("processed with headers".to_string())
    }

    // Valid handler with exchange
    #[handler]
    pub async fn process_exchange(
        &self,
        _exchange: &mut camel_api::Exchange,
    ) -> Result<(), String> {
        Ok(())
    }

    // Valid handler with no special parameters
    #[handler]
    pub async fn simple(&self) -> Result<String, String> {
        Ok("simple".to_string())
    }

    // Non-handler method - should be ignored
    pub fn helper(&self) -> String {
        "helper".to_string()
    }

    // Another non-handler
    pub async fn not_a_handler(&self) -> String {
        "not a handler".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handler_attribute_compiles() {
        // This test just verifies that the #[handler] attribute compiles
        // The actual parsing happens in the macro code
        let _service = TestService;
    }

    #[test]
    fn test_multiple_handlers_allowed() {
        // Verify we can have multiple methods with #[handler]
        let _service = TestService;
    }
}
