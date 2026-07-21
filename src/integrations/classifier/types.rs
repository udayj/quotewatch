use thiserror::Error;

pub type ClassifierResult<T> = std::result::Result<T, ClassifierError>;

#[derive(Debug, Error)]
pub enum ClassifierError {
    #[error("missing environment variable {0}")]
    MissingEnv(&'static str),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("HTTP retry error: {0}")]
    HttpRetry(String),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("response format error: {0}")]
    ResponseFormat(String),

    #[error("schema validation error: {0}")]
    Schema(String),
}

impl From<crate::integrations::http::HttpError> for ClassifierError {
    fn from(error: crate::integrations::http::HttpError) -> Self {
        Self::HttpRetry(error.to_string())
    }
}

pub(super) mod fireworks {
    use serde::{Deserialize, Serialize};
    use serde_json::Value;

    #[derive(Debug, Serialize)]
    pub struct ChatRequest {
        pub model: String,
        pub temperature: f32,
        pub max_tokens: u32,
        pub reasoning_effort: String,
        pub messages: Vec<ChatMessage>,
        pub tools: Vec<Value>,
        pub tool_choice: Value,
    }

    #[derive(Debug, Serialize)]
    pub struct ChatMessage {
        pub role: String,
        pub content: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct ChatResponse {
        pub choices: Vec<ChatChoice>,
        pub usage: Option<ChatUsage>,
    }

    #[derive(Debug, Deserialize)]
    pub struct ChatChoice {
        pub message: ChatChoiceMessage,
    }

    #[derive(Debug, Deserialize)]
    pub struct ChatChoiceMessage {
        pub content: Option<String>,
        pub tool_calls: Option<Vec<ToolCall>>,
    }

    #[derive(Debug, Deserialize)]
    pub struct ToolCall {
        #[serde(rename = "type")]
        pub kind: String,
        pub function: ToolCallFunction,
    }

    #[derive(Debug, Deserialize)]
    pub struct ToolCallFunction {
        pub name: String,
        pub arguments: Value,
    }

    #[derive(Debug, Deserialize)]
    pub struct ChatUsage {
        pub prompt_tokens: Option<i64>,
        pub completion_tokens: Option<i64>,
        pub prompt_tokens_details: Option<PromptTokensDetails>,
    }

    #[derive(Debug, Deserialize)]
    pub struct PromptTokensDetails {
        pub cached_tokens: Option<i64>,
    }
}

pub(super) mod claude {
    use serde::{Deserialize, Serialize};
    use serde_json::Value;

    #[derive(Debug, Serialize)]
    pub struct ClaudeRequest {
        pub model: String,
        pub max_tokens: u32,
        pub temperature: f32,
        pub system: Vec<ClaudeSystemBlock>,
        pub messages: Vec<ClaudeMessage>,
        pub tools: Vec<ClaudeTool>,
        pub tool_choice: Value,
    }

    #[derive(Debug, Serialize)]
    pub struct ClaudeSystemBlock {
        #[serde(rename = "type")]
        pub kind: String,
        pub text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub cache_control: Option<Value>,
    }

    #[derive(Debug, Serialize)]
    pub struct ClaudeMessage {
        pub role: String,
        pub content: String,
    }

    #[derive(Debug, Serialize)]
    pub struct ClaudeTool {
        pub name: String,
        pub description: String,
        pub input_schema: Value,
    }

    #[derive(Debug, Deserialize)]
    pub struct ClaudeResponse {
        pub content: Vec<ClaudeContentBlock>,
        pub usage: Option<ClaudeUsage>,
    }

    #[derive(Debug, Deserialize)]
    pub struct ClaudeContentBlock {
        #[serde(rename = "type")]
        pub kind: String,
        pub name: Option<String>,
        pub input: Option<Value>,
    }

    #[derive(Debug, Deserialize)]
    pub struct ClaudeUsage {
        pub input_tokens: Option<i64>,
        pub cache_creation_input_tokens: Option<i64>,
        pub cache_read_input_tokens: Option<i64>,
        pub output_tokens: Option<i64>,
    }
}
