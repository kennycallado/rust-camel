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
pub const CAMEL_LLM_TOOLS: &str = "CamelLlmTools";
pub const CAMEL_LLM_TOOL_CHOICE: &str = "CamelLlmToolChoice";
pub const CAMEL_LLM_TOOL_CALLS: &str = "CamelLlmToolCalls";
pub const CAMEL_LLM_ESTIMATED_COST_USD: &str = "CamelLlmEstimatedCostUsd";
pub const CAMEL_LLM_MESSAGES: &str = "CamelLlmMessages";
pub const CAMEL_LLM_TEXT: &str = "CamelLlmText";

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
        assert_eq!(CAMEL_LLM_TOOLS, "CamelLlmTools");
        assert_eq!(CAMEL_LLM_TOOL_CHOICE, "CamelLlmToolChoice");
        assert_eq!(CAMEL_LLM_TOOL_CALLS, "CamelLlmToolCalls");
        assert_eq!(CAMEL_LLM_ESTIMATED_COST_USD, "CamelLlmEstimatedCostUsd");
        assert_eq!(CAMEL_LLM_MESSAGES, "CamelLlmMessages");
        assert_eq!(CAMEL_LLM_TEXT, "CamelLlmText");
    }
}
