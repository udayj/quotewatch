use super::common::{
    CLASSIFICATION_TOOL, Pricing, classification_batch_from_value, classification_prompt_from_env,
    empty_result, env_f64, validate_batch,
};
use super::guards::apply_deterministic_guards;
use super::types::{
    ClassifierError, ClassifierResult,
    fireworks::{ChatChoiceMessage, ChatMessage, ChatRequest, ChatResponse, ChatUsage},
};
use crate::domain::types::{ClassificationBatch, ClassificationResult, LlmUsage, ThreadDigest};
use crate::integrations::http::RetryableHttpClient;
use serde_json::{Value, json};

const DEFAULT_FIREWORKS_URL: &str = "https://api.fireworks.ai/inference/v1/chat/completions";

pub struct FireworksClassifier {
    client: RetryableHttpClient,
    api_key: String,
    model: String,
    base_url: String,
    classification_prompt: String,
    pricing: Pricing,
}

impl FireworksClassifier {
    // Builds the Fireworks classifier from environment configuration.
    // Required: FIREWORKS_API_KEY. Optional values choose model, endpoint, and pricing.
    pub fn from_env() -> ClassifierResult<Self> {
        let api_key = std::env::var("FIREWORKS_API_KEY")
            .map_err(|_| ClassifierError::MissingEnv("FIREWORKS_API_KEY"))?;
        let model = std::env::var("FIREWORKS_MODEL")
            .unwrap_or_else(|_| "accounts/fireworks/models/kimi-k2-instruct".to_string());
        let base_url = std::env::var("FIREWORKS_BASE_URL")
            .unwrap_or_else(|_| DEFAULT_FIREWORKS_URL.to_string());
        let pricing = Pricing::fireworks_from_env();
        Ok(Self {
            client: RetryableHttpClient::llm()?,
            api_key,
            model,
            base_url,
            classification_prompt: classification_prompt_from_env()?,
            pricing,
        })
    }

    // Exposes the configured model name so the runner can record which model was used.
    pub fn model(&self) -> &str {
        &self.model
    }

    // Classification flow:
    // 1. Build an OpenAI-compatible chat request with the shared prompt and tool schema.
    // 2. Send it through the shared retry helper.
    // 3. Convert provider usage into the app's normalized usage/cost type.
    // 4. Read the submit_classification tool arguments into a ClassificationBatch.
    // 5. Validate that the batch matches the requested thread IDs exactly.
    // 6. Apply deterministic domain guards for high-confidence business rules.
    pub fn classify(&self, digests: &[ThreadDigest]) -> ClassifierResult<ClassificationResult> {
        if digests.is_empty() {
            return Ok(empty_result());
        }

        let request = ChatRequest {
            model: self.model.clone(),
            temperature: 0.0,
            max_tokens: 16_384,
            reasoning_effort: "none".to_string(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: self.classification_prompt.trim().to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: serde_json::to_string_pretty(digests)?,
                },
            ],
            tools: vec![serde_json::from_str(CLASSIFICATION_TOOL)?],
            tool_choice: json!({
                "type": "function",
                "function": {
                    "name": "submit_classification"
                }
            }),
        };

        let response: ChatResponse = self.send_with_retries(&request)?;
        let usage = self.usage_from_response(response.usage.as_ref());
        let message = response
            .choices
            .first()
            .map(|choice| &choice.message)
            .ok_or_else(|| {
                ClassifierError::ResponseFormat(
                    "classification response did not include a message".to_string(),
                )
            })?;

        let mut batch = parse_classification(message)?;
        if let Err(error) = validate_batch(digests, &batch) {
            if digests.len() > 1 {
                eprintln!(
                    "Classifier returned incomplete batch; retrying {} thread(s) individually. Error: {error:#}",
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

    // Supplies Fireworks-specific auth and request shape while the shared HTTP client
    // owns retry/backoff/status handling and JSON deserialization.
    fn send_with_retries(&self, request: &ChatRequest) -> ClassifierResult<ChatResponse> {
        self.client
            .execute(
                "Fireworks classification",
                self.client
                    .post(&self.base_url)
                    .bearer_auth(&self.api_key)
                    .json(request),
            )
            .map_err(Into::into)
    }

    // Fireworks' OpenAI-compatible usage reports `cached_tokens` as a subset of
    // `prompt_tokens`, so prompt_tokens is already the total input token count.
    fn usage_from_response(&self, usage: Option<&ChatUsage>) -> LlmUsage {
        let Some(usage) = usage else {
            return LlmUsage::default();
        };
        let input_tokens = usage.prompt_tokens.unwrap_or_default();
        let cached_input_tokens = usage
            .prompt_tokens_details
            .as_ref()
            .and_then(|details| details.cached_tokens)
            .unwrap_or_default();
        let output_tokens = usage.completion_tokens.unwrap_or_default();
        self.pricing
            .cost(input_tokens, cached_input_tokens, 0, output_tokens)
    }
}

impl Pricing {
    // Reads Fireworks pricing from env so model/pricing changes do not require code changes.
    fn fireworks_from_env() -> Self {
        Self {
            input_usd_per_1m: env_f64("FIREWORKS_INPUT_USD_PER_1M"),
            cache_read_usd_per_1m: env_f64("FIREWORKS_CACHED_INPUT_USD_PER_1M"),
            cache_write_usd_per_1m: None,
            output_usd_per_1m: env_f64("FIREWORKS_OUTPUT_USD_PER_1M"),
            usd_to_inr: env_f64("USD_TO_INR").unwrap_or(100.0),
        }
    }
}

// Converts the parsed provider payload into the domain batch shape.
fn parse_classification(message: &ChatChoiceMessage) -> ClassifierResult<ClassificationBatch> {
    let value = classification_payload(message)?;
    classification_batch_from_value(value)
}

// Locates the classification payload in the provider message. Tool calls are
// preferred, but plain JSON content is accepted as a compatibility fallback.
fn classification_payload(message: &ChatChoiceMessage) -> ClassifierResult<Value> {
    if let Some(tool_call) = message
        .tool_calls
        .as_deref()
        .and_then(|calls| calls.iter().find(|call| call.kind == "function"))
    {
        if tool_call.function.name != "submit_classification" {
            return Err(ClassifierError::ResponseFormat(format!(
                "classifier called unexpected tool {}",
                tool_call.function.name
            )));
        }
        return parse_tool_arguments(&tool_call.function.arguments);
    }

    let Some(content) = message.content.as_deref() else {
        return Err(ClassifierError::ResponseFormat(
            "classification response had neither tool call nor JSON content".to_string(),
        ));
    };
    serde_json::from_str(content).map_err(|err| {
        ClassifierError::ResponseFormat(format!(
            "classifier returned invalid JSON content: {err}; content: {content}"
        ))
    })
}

// Fireworks may return tool arguments either as a JSON object or as a JSON-encoded
// string, depending on provider/model behavior. Normalize both into Value.
fn parse_tool_arguments(arguments: &Value) -> ClassifierResult<Value> {
    match arguments {
        Value::String(raw) => serde_json::from_str(raw).map_err(|err| {
            ClassifierError::ResponseFormat(format!(
                "classifier tool arguments were not valid JSON: {err}; arguments: {raw}"
            ))
        }),
        Value::Object(_) => Ok(arguments.clone()),
        other => Err(ClassifierError::ResponseFormat(format!(
            "classifier tool arguments had unexpected shape: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::fireworks::{ToolCall, ToolCallFunction};
    use super::*;

    #[test]
    fn parse_classification_rejects_submit_tool_arguments_with_wrong_schema() {
        let message = ChatChoiceMessage {
            content: None,
            tool_calls: Some(vec![ToolCall {
                kind: "function".to_string(),
                function: ToolCallFunction {
                    name: "submit_classification".to_string(),
                    arguments: json!({ "items": [] }),
                },
            }]),
        };

        let error = parse_classification(&message).unwrap_err();

        assert!(
            matches!(error, ClassifierError::Schema(message) if message.contains("classifier JSON did not match schema"))
        );
    }
}
