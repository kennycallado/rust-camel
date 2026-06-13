//! Header constants for the LLM component.

pub const CAMEL_LLM_PROVIDER: &str = "CamelLlmProvider";
pub const CAMEL_LLM_MODEL: &str = "CamelLlmModel";
pub const CAMEL_LLM_TEMPERATURE: &str = "CamelLlmTemperature";
pub const CAMEL_LLM_MAX_TOKENS: &str = "CamelLlmMaxTokens";
pub const CAMEL_LLM_SYSTEM_PROMPT: &str = "CamelLlmSystemPrompt";
pub const CAMEL_LLM_STREAM: &str = "CamelLlmStream";
pub const CAMEL_LLM_TOKENS_IN: &str = "CamelLlmTokensIn";
pub const CAMEL_LLM_TOKENS_OUT: &str = "CamelLlmTokensOut";
pub const CAMEL_LLM_FINISH_REASON: &str = "CamelLlmFinishReason";
pub const CAMEL_LLM_USAGE_AVAILABLE: &str = "CamelLlmUsageAvailable";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_constants_are_prefixed() {
        assert_eq!(CAMEL_LLM_PROVIDER, "CamelLlmProvider");
        assert_eq!(CAMEL_LLM_MODEL, "CamelLlmModel");
        assert_eq!(CAMEL_LLM_TEMPERATURE, "CamelLlmTemperature");
        assert_eq!(CAMEL_LLM_MAX_TOKENS, "CamelLlmMaxTokens");
        assert_eq!(CAMEL_LLM_SYSTEM_PROMPT, "CamelLlmSystemPrompt");
        assert_eq!(CAMEL_LLM_STREAM, "CamelLlmStream");
        assert_eq!(CAMEL_LLM_TOKENS_IN, "CamelLlmTokensIn");
        assert_eq!(CAMEL_LLM_TOKENS_OUT, "CamelLlmTokensOut");
        assert_eq!(CAMEL_LLM_FINISH_REASON, "CamelLlmFinishReason");
        assert_eq!(CAMEL_LLM_USAGE_AVAILABLE, "CamelLlmUsageAvailable");
    }
}
