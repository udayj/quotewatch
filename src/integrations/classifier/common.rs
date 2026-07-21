use super::types::{ClassifierError, ClassifierResult};
use crate::domain::types::{ClassificationBatch, ClassificationResult, LlmUsage, ThreadDigest};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;

pub(super) const CLASSIFICATION_TOOL: &str =
    include_str!("../../../assets/llm/classification_tool.json");

// Loads the classification prompt at runtime so operational rules and examples
// remain outside the repository and can be changed without rebuilding the binary.
pub(super) fn classification_prompt_from_env() -> ClassifierResult<String> {
    let path = std::env::var("CLASSIFICATION_PROMPT_FILE")
        .map_err(|_| ClassifierError::MissingEnv("CLASSIFICATION_PROMPT_FILE"))?;
    if path.trim().is_empty() {
        return Err(ClassifierError::Config(
            "CLASSIFICATION_PROMPT_FILE must name a readable prompt file".to_string(),
        ));
    }

    let prompt = fs::read_to_string(&path).map_err(|error| {
        ClassifierError::Config(format!(
            "could not read classification prompt file {path}: {error}"
        ))
    })?;
    if prompt.trim().is_empty() {
        return Err(ClassifierError::Config(format!(
            "classification prompt file {path} is empty"
        )));
    }
    Ok(prompt)
}

// Shared empty classification result for callers that have no digests to classify.
pub(super) fn empty_result() -> ClassificationResult {
    ClassificationResult {
        batch: ClassificationBatch {
            classifications: Vec::new(),
        },
        usage: LlmUsage::default(),
    }
}

// Converts a provider-specific JSON payload into the domain batch shape.
// At this point the JSON has already been located in the provider response.
pub(super) fn classification_batch_from_value(
    value: Value,
) -> ClassifierResult<ClassificationBatch> {
    serde_json::from_value(value).map_err(|err| {
        ClassifierError::Schema(format!("classifier JSON did not match schema: {err}"))
    })
}

// Checks that the model returned exactly one classification for each requested
// thread, with no invented thread IDs. This catches partial or malformed batches
// before deterministic business rules are applied.
pub(super) fn validate_batch(
    digests: &[ThreadDigest],
    batch: &ClassificationBatch,
) -> ClassifierResult<()> {
    let expected = digests
        .iter()
        .map(|digest| digest.thread_id.as_str())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();

    for classification in &batch.classifications {
        if !expected.contains(classification.thread_id.as_str()) {
            return Err(ClassifierError::Schema(format!(
                "classifier returned unknown thread_id {}",
                classification.thread_id
            )));
        }
        if !seen.insert(classification.thread_id.as_str()) {
            return Err(ClassifierError::Schema(format!(
                "classifier returned duplicate thread_id {}",
                classification.thread_id
            )));
        }
    }

    let missing = expected.difference(&seen).copied().collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(ClassifierError::Schema(format!(
            "classifier omitted thread_id(s): {}",
            missing.join(", ")
        )));
    }

    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Pricing {
    pub(super) input_usd_per_1m: Option<f64>,
    pub(super) cache_read_usd_per_1m: Option<f64>,
    pub(super) cache_write_usd_per_1m: Option<f64>,
    pub(super) output_usd_per_1m: Option<f64>,
    pub(super) usd_to_inr: f64,
}

impl Pricing {
    // Converts provider token counts into normalized usage and estimated cost.
    // `input_tokens` must be the total prompt/input tokens for the request; the
    // cached portions are subtracted here so each token category is priced once.
    pub(super) fn cost(
        &self,
        input_tokens: i64,
        cache_read_input_tokens: i64,
        cache_creation_input_tokens: i64,
        output_tokens: i64,
    ) -> LlmUsage {
        let uncached_input_tokens = input_tokens
            .saturating_sub(cache_read_input_tokens)
            .saturating_sub(cache_creation_input_tokens);
        let cost_usd = (uncached_input_tokens as f64 * self.input_usd_per_1m.unwrap_or(0.0)
            + cache_read_input_tokens as f64 * self.cache_read_usd_per_1m.unwrap_or(0.0)
            + cache_creation_input_tokens as f64 * self.cache_write_usd_per_1m.unwrap_or(0.0)
            + output_tokens as f64 * self.output_usd_per_1m.unwrap_or(0.0))
            / 1_000_000.0;
        LlmUsage {
            input_tokens,
            cached_input_tokens: cache_read_input_tokens,
            cache_creation_input_tokens,
            output_tokens,
            cost_usd,
            cost_inr: cost_usd * self.usd_to_inr,
        }
    }
}

// Reads a floating-point environment variable when present and parseable.
// Missing or invalid values intentionally fall back to provider defaults.
pub(super) fn env_f64(name: &str) -> Option<f64> {
    std::env::var(name).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::{OmittedKeywordSummary, ThreadClassification, ThreadState};
    use chrono::NaiveDate;

    #[test]
    fn pricing_cost_prices_each_token_bucket_once() {
        let pricing = Pricing {
            input_usd_per_1m: Some(2.0),
            cache_read_usd_per_1m: Some(0.5),
            cache_write_usd_per_1m: Some(1.5),
            output_usd_per_1m: Some(4.0),
            usd_to_inr: 80.0,
        };

        let usage = pricing.cost(1_000, 100, 200, 50);

        assert_eq!(usage.input_tokens, 1_000);
        assert_eq!(usage.cached_input_tokens, 100);
        assert_eq!(usage.cache_creation_input_tokens, 200);
        assert_eq!(usage.output_tokens, 50);
        assert_float_eq(usage.cost_usd, 0.00195);
        assert_float_eq(usage.cost_inr, 0.156);
    }

    #[test]
    fn pricing_missing_bucket_price_defaults_to_zero() {
        let pricing = Pricing {
            input_usd_per_1m: Some(1.0),
            cache_read_usd_per_1m: None,
            cache_write_usd_per_1m: None,
            output_usd_per_1m: Some(2.0),
            usd_to_inr: 100.0,
        };

        let usage = pricing.cost(1_000, 100, 200, 50);

        assert_float_eq(usage.cost_usd, 0.0008);
        assert_float_eq(usage.cost_inr, 0.08);
    }

    #[test]
    fn validate_batch_accepts_exact_thread_ids_once() {
        let digests = vec![digest("thread-a"), digest("thread-b")];
        let batch = ClassificationBatch {
            classifications: vec![classification("thread-a"), classification("thread-b")],
        };

        assert!(validate_batch(&digests, &batch).is_ok());
    }

    #[test]
    fn validate_batch_rejects_unknown_thread_id() {
        let digests = vec![digest("thread-a")];
        let batch = ClassificationBatch {
            classifications: vec![classification("thread-b")],
        };

        let error = validate_batch(&digests, &batch).unwrap_err();

        assert!(
            matches!(error, ClassifierError::Schema(message) if message.contains("unknown thread_id thread-b"))
        );
    }

    #[test]
    fn validate_batch_rejects_duplicate_thread_id() {
        let digests = vec![digest("thread-a")];
        let batch = ClassificationBatch {
            classifications: vec![classification("thread-a"), classification("thread-a")],
        };

        let error = validate_batch(&digests, &batch).unwrap_err();

        assert!(
            matches!(error, ClassifierError::Schema(message) if message.contains("duplicate thread_id thread-a"))
        );
    }

    #[test]
    fn validate_batch_rejects_missing_thread_id() {
        let digests = vec![digest("thread-a"), digest("thread-b")];
        let batch = ClassificationBatch {
            classifications: vec![classification("thread-a")],
        };

        let error = validate_batch(&digests, &batch).unwrap_err();

        assert!(
            matches!(error, ClassifierError::Schema(message) if message.contains("omitted thread_id(s): thread-b"))
        );
    }

    fn digest(thread_id: &str) -> ThreadDigest {
        ThreadDigest {
            thread_id: thread_id.to_string(),
            subject: "Test subject".to_string(),
            as_of_date: NaiveDate::from_ymd_opt(2026, 6, 29).unwrap(),
            account: "sales@example.com".to_string(),
            messages_kept: 0,
            messages_omitted_before_digest: 0,
            omitted_keyword_summary: OmittedKeywordSummary::default(),
            messages: Vec::new(),
        }
    }

    fn classification(thread_id: &str) -> ThreadClassification {
        ThreadClassification {
            thread_id: thread_id.to_string(),
            state: ThreadState::Ignored,
            requester_name: None,
            requester_email: None,
            requester_org: None,
            subject: None,
            latest_customer_message_at: None,
            latest_company_message_at: None,
            quote_sent_at: None,
            latest_relevant_message_at: None,
            extracted_requirement: None,
            short_reason: "test".to_string(),
            confidence: 1.0,
        }
    }

    fn assert_float_eq(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 0.000_001,
            "actual {actual} did not match expected {expected}"
        );
    }
}
