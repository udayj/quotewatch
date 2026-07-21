use super::common::{
    CLASSIFICATION_TOOL, Pricing, classification_batch_from_value, classification_prompt_from_env,
    empty_result, env_f64, validate_batch,
};
use super::guards::apply_deterministic_guards;
use super::types::{
    ClassifierError, ClassifierResult,
    claude::{
        ClaudeMessage, ClaudeRequest, ClaudeResponse, ClaudeSystemBlock, ClaudeTool, ClaudeUsage,
    },
};
use crate::domain::types::{ClassificationBatch, ClassificationResult, LlmUsage, ThreadDigest};
use crate::integrations::http::RetryableHttpClient;
use serde_json::{Value, json};

const DEFAULT_CLAUDE_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct ClaudeClassifier {
    client: RetryableHttpClient,
    api_key: String,
    model: String,
    base_url: String,
    classification_prompt: String,
    pricing: Pricing,
}

impl ClaudeClassifier {
    // Builds the Claude classifier from environment configuration.
    // Required: ANTHROPIC_API_KEY. Optional values choose model, endpoint, and pricing.
    pub fn from_env() -> ClassifierResult<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| ClassifierError::MissingEnv("ANTHROPIC_API_KEY"))?;
        let model =
            std::env::var("CLAUDE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-6".to_string());
        let base_url =
            std::env::var("CLAUDE_BASE_URL").unwrap_or_else(|_| DEFAULT_CLAUDE_URL.to_string());
        Ok(Self {
            client: RetryableHttpClient::llm()?,
            api_key,
            model,
            base_url,
            classification_prompt: classification_prompt_from_env()?,
            pricing: Pricing::claude_from_env(),
        })
    }

    // Exposes the configured model name so the runner can record which model was used.
    pub fn model(&self) -> &str {
        &self.model
    }

    // Classification flow:
    // 1. Build a Claude Messages request with the shared prompt and tool schema.
    // 2. Send it through the shared retry helper.
    // 3. Convert provider usage into the app's normalized usage/cost type.
    // 4. Read the submit_classification tool input into a ClassificationBatch.
    // 5. Validate that the batch matches the requested thread IDs exactly.
    // 6. Apply deterministic domain guards for high-confidence business rules.
    pub fn classify(&self, digests: &[ThreadDigest]) -> ClassifierResult<ClassificationResult> {
        if digests.is_empty() {
            return Ok(empty_result());
        }

        let request = ClaudeRequest {
            model: self.model.clone(),
            max_tokens: 16_384,
            temperature: 0.0,
            system: vec![ClaudeSystemBlock {
                kind: "text".to_string(),
                text: self.classification_prompt.trim().to_string(),
                cache_control: Some(json!({ "type": "ephemeral", "ttl": "5m" })),
            }],
            messages: vec![ClaudeMessage {
                role: "user".to_string(),
                content: serde_json::to_string_pretty(digests)?,
            }],
            tools: vec![claude_tool()?],
            tool_choice: json!({
                "type": "tool",
                "name": "submit_classification"
            }),
        };

        let response = self.send_with_retries(&request)?;
        let usage = self.usage_from_response(response.usage.as_ref());
        let mut batch = parse_classification(&response)?;
        if let Err(error) = validate_batch(digests, &batch) {
            if digests.len() > 1 {
                eprintln!(
                    "Claude returned incomplete batch; retrying {} thread(s) individually. Error: {error:#}",
                    digests.len()
                );
                return self.classify_individually(digests);
            }
            return Err(error);
        }
        apply_deterministic_guards(digests, &mut batch);
        Ok(ClassificationResult { batch, usage })
    }

    // Falls back to one request per thread when a batched model response is incomplete.
    // This trades latency/cost for a better chance of preserving useful classifications.
    fn classify_individually(
        &self,
        digests: &[ThreadDigest],
    ) -> ClassifierResult<ClassificationResult> {
        let mut classifications = Vec::with_capacity(digests.len());
        let mut usage = LlmUsage::default();
        for digest in digests {
            let result = self.classify(std::slice::from_ref(digest))?;
            classifications.extend(result.batch.classifications);
            usage.add(result.usage);
        }
        Ok(ClassificationResult {
            batch: ClassificationBatch { classifications },
            usage,
        })
    }

    // Supplies Claude-specific headers and request shape while the shared HTTP client
    // owns retry/backoff/status handling and JSON deserialization.
    fn send_with_retries(&self, request: &ClaudeRequest) -> ClassifierResult<ClaudeResponse> {
        self.client
            .execute(
                "Claude classification",
                self.client
                    .post(&self.base_url)
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", ANTHROPIC_VERSION)
                    .json(request),
            )
            .map_err(Into::into)
    }

    // Claude reports cache creation/read token counts separately from `input_tokens`.
    // We add them to get total input tokens, while Pricing::cost subtracts and prices
    // each category separately.
    fn usage_from_response(&self, usage: Option<&ClaudeUsage>) -> LlmUsage {
        let Some(usage) = usage else {
            return LlmUsage::default();
        };
        let uncached_input_tokens = usage.input_tokens.unwrap_or_default();
        let cache_creation_input_tokens = usage.cache_creation_input_tokens.unwrap_or_default();
        let cache_read_input_tokens = usage.cache_read_input_tokens.unwrap_or_default();
        let output_tokens = usage.output_tokens.unwrap_or_default();
        let total_input_tokens =
            uncached_input_tokens + cache_creation_input_tokens + cache_read_input_tokens;
        self.pricing.cost(
            total_input_tokens,
            cache_read_input_tokens,
            cache_creation_input_tokens,
            output_tokens,
        )
    }
}

impl Pricing {
    // Reads Claude pricing from env so model/pricing changes do not require code changes.
    fn claude_from_env() -> Self {
        Self {
            input_usd_per_1m: env_f64("CLAUDE_INPUT_USD_PER_1M"),
            cache_read_usd_per_1m: env_f64("CLAUDE_CACHE_READ_USD_PER_1M"),
            cache_write_usd_per_1m: env_f64("CLAUDE_CACHE_WRITE_USD_PER_1M"),
            output_usd_per_1m: env_f64("CLAUDE_OUTPUT_USD_PER_1M"),
            usd_to_inr: env_f64("USD_TO_INR").unwrap_or(100.0),
        }
    }
}

// Converts the OpenAI-style shared tool JSON into Claude's native tool shape.
fn claude_tool() -> ClassifierResult<ClaudeTool> {
    let tool: Value = serde_json::from_str(CLASSIFICATION_TOOL)?;
    let function = tool.get("function").ok_or_else(|| {
        ClassifierError::Schema("classification tool is missing function object".to_string())
    })?;
    let name = required_string(function, "name")?;
    let description = required_string(function, "description")?;
    let input_schema = function.get("parameters").cloned().ok_or_else(|| {
        ClassifierError::Schema("classification tool is missing parameters".to_string())
    })?;
    Ok(ClaudeTool {
        name,
        description,
        input_schema,
    })
}

// Reads a required string field from the shared tool JSON and turns missing
// schema pieces into actionable classifier configuration errors.
fn required_string(value: &Value, field: &str) -> ClassifierResult<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ClassifierError::Schema(format!("classification tool is missing {field}")))
}

// Finds Claude's submit_classification tool-use block and converts its input
// into the domain batch. Free-text content is not accepted for Claude responses.
fn parse_classification(response: &ClaudeResponse) -> ClassifierResult<ClassificationBatch> {
    let tool_use = response
        .content
        .iter()
        .find(|block| {
            block.kind == "tool_use" && block.name.as_deref() == Some("submit_classification")
        })
        .ok_or_else(|| {
            ClassifierError::ResponseFormat(
                "Claude response did not include submit_classification tool use".to_string(),
            )
        })?;
    let input = tool_use.input.clone().ok_or_else(|| {
        ClassifierError::ResponseFormat("Claude tool use did not include input".to_string())
    })?;
    classification_batch_from_value(input)
}
