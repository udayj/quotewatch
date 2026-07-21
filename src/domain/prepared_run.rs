use super::types::{GmailThread, LlmUsage, ThreadClassification, ThreadDigest, ThreadState};
use chrono::NaiveDate;
use std::collections::HashSet;
use thiserror::Error;

/// Logical result of one daily CRM run before it is persisted.
#[derive(Debug, Clone)]
pub struct PreparedRun {
    pub run_key: String,
    pub label: Option<String>,
    pub account: String,
    pub as_of_date: NaiveDate,
    pub model: Option<String>,
    pub started_at: String,
    pub finished_at: String,
    pub threads_seen: usize,
    pub inputs: Vec<PreparedThreadInput>,
    pub classifications: Vec<PreparedClassification>,
    pub usage: LlmUsage,
    pub notes: Option<String>,
}

impl PreparedRun {
    /// Builds and validates the logical output of a run.
    pub fn from_parts(parts: PreparedRunParts) -> Result<Self, PreparedRunError> {
        let run_key = run_key(
            &parts.account,
            parts.as_of_date,
            parts.label.as_deref(),
            parts.model.as_deref(),
        );
        let prepared = Self {
            run_key,
            label: parts.label,
            account: parts.account,
            as_of_date: parts.as_of_date,
            model: parts.model,
            started_at: parts.started_at,
            finished_at: parts.finished_at,
            threads_seen: parts.threads_seen,
            inputs: parts.inputs,
            classifications: parts.classifications,
            usage: parts.usage,
            notes: parts.notes,
        };
        prepared.validate()?;
        Ok(prepared)
    }

    /// Checks invariants that make the prepared run safe to persist atomically.
    pub fn validate(&self) -> Result<(), PreparedRunError> {
        if self.account.trim().is_empty() {
            return Err(PreparedRunError::EmptyAccount);
        }
        if self.run_key.trim().is_empty() {
            return Err(PreparedRunError::EmptyRunKey);
        }
        if self.threads_seen != self.inputs.len() {
            return Err(PreparedRunError::ThreadCountMismatch {
                threads_seen: self.threads_seen,
                inputs: self.inputs.len(),
            });
        }

        let mut input_ids = HashSet::new();
        for input in &self.inputs {
            if input.thread.thread_id != input.digest.thread_id {
                return Err(PreparedRunError::ThreadInputMismatch {
                    thread_id: input.thread.thread_id.clone(),
                    digest_id: input.digest.thread_id.clone(),
                });
            }
            if !input_ids.insert(input.digest.thread_id.as_str()) {
                return Err(PreparedRunError::DuplicateThreadInput {
                    thread_id: input.digest.thread_id.clone(),
                });
            }
        }

        let mut classification_ids = HashSet::new();
        for classification in &self.classifications {
            let thread_id = classification.classification.thread_id.as_str();
            if !input_ids.contains(thread_id) {
                return Err(PreparedRunError::ClassificationWithoutInput {
                    thread_id: classification.classification.thread_id.clone(),
                });
            }
            if !classification_ids.insert(thread_id) {
                return Err(PreparedRunError::DuplicateClassification {
                    thread_id: classification.classification.thread_id.clone(),
                });
            }
        }

        Ok(())
    }
}

/// Named construction parts for a validated `PreparedRun`.
pub struct PreparedRunParts {
    pub label: Option<String>,
    pub account: String,
    pub as_of_date: NaiveDate,
    pub model: Option<String>,
    pub started_at: String,
    pub finished_at: String,
    pub threads_seen: usize,
    pub inputs: Vec<PreparedThreadInput>,
    pub classifications: Vec<PreparedClassification>,
    pub usage: LlmUsage,
    pub notes: Option<String>,
}

/// Raw thread snapshot and digest prepared for persistence.
#[derive(Debug, Clone)]
pub struct PreparedThreadInput {
    pub thread: GmailThread,
    pub digest: ThreadDigest,
}

/// Classification paired with the DB state it was prepared against.
#[derive(Debug, Clone)]
pub struct PreparedClassification {
    pub classification: ThreadClassification,
    pub previous_state: Option<ThreadState>,
}

/// Validation errors for malformed prepared runs.
#[derive(Debug, Error)]
pub enum PreparedRunError {
    #[error("prepared run account must not be empty")]
    EmptyAccount,

    #[error("prepared run key must not be empty")]
    EmptyRunKey,

    #[error("prepared run saw {threads_seen} threads but contains {inputs} inputs")]
    ThreadCountMismatch { threads_seen: usize, inputs: usize },

    #[error("prepared input thread id {thread_id} does not match digest id {digest_id}")]
    ThreadInputMismatch {
        thread_id: String,
        digest_id: String,
    },

    #[error("prepared run contains duplicate input for thread {thread_id}")]
    DuplicateThreadInput { thread_id: String },

    #[error("classification for thread {thread_id} has no prepared input")]
    ClassificationWithoutInput { thread_id: String },

    #[error("prepared run contains duplicate classification for thread {thread_id}")]
    DuplicateClassification { thread_id: String },
}

fn run_key(
    account: &str,
    as_of_date: NaiveDate,
    label: Option<&str>,
    model: Option<&str>,
) -> String {
    let label = label.unwrap_or("default");
    let model = model.unwrap_or("skip-classify");
    format!("daily:{account}:{as_of_date}:{label}:{model}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::{MessageDirection, OmittedKeywordSummary, ThreadState};

    #[test]
    fn prepared_run_rejects_duplicate_classifications() {
        let input = test_thread_input("thread-1");
        let classification = test_prepared_classification("thread-1");
        let error = PreparedRun::from_parts(PreparedRunParts {
            label: None,
            account: "sales@example.com".to_string(),
            as_of_date: date(),
            model: Some("model-a".to_string()),
            started_at: "2026-07-04T00:00:00Z".to_string(),
            finished_at: "2026-07-04T00:00:01Z".to_string(),
            threads_seen: 1,
            inputs: vec![input],
            classifications: vec![classification.clone(), classification],
            usage: LlmUsage::default(),
            notes: None,
        })
        .unwrap_err();

        assert!(matches!(
            error,
            PreparedRunError::DuplicateClassification { .. }
        ));
    }

    #[test]
    fn prepared_run_rejects_classification_without_input() {
        let error = PreparedRun::from_parts(PreparedRunParts {
            label: None,
            account: "sales@example.com".to_string(),
            as_of_date: date(),
            model: Some("model-a".to_string()),
            started_at: "2026-07-04T00:00:00Z".to_string(),
            finished_at: "2026-07-04T00:00:01Z".to_string(),
            threads_seen: 0,
            inputs: Vec::new(),
            classifications: vec![test_prepared_classification("thread-1")],
            usage: LlmUsage::default(),
            notes: None,
        })
        .unwrap_err();

        assert!(matches!(
            error,
            PreparedRunError::ClassificationWithoutInput { .. }
        ));
    }

    fn test_thread_input(thread_id: &str) -> PreparedThreadInput {
        PreparedThreadInput {
            thread: GmailThread {
                thread_id: thread_id.to_string(),
                history_id: None,
                control_label_names: Vec::new(),
                messages: Vec::new(),
            },
            digest: ThreadDigest {
                thread_id: thread_id.to_string(),
                subject: "Subject".to_string(),
                as_of_date: date(),
                account: "sales@example.com".to_string(),
                messages_kept: 1,
                messages_omitted_before_digest: 0,
                omitted_keyword_summary: OmittedKeywordSummary::default(),
                messages: vec![crate::domain::types::DigestMessage {
                    message_id: "message-1".to_string(),
                    date: Some("2026-07-04".to_string()),
                    internal_date_ms: None,
                    direction: MessageDirection::ExternalToCompany,
                    from: None,
                    to: None,
                    subject: Some("Subject".to_string()),
                    snippet: None,
                    body_excerpt: "hello".to_string(),
                    attachments: Vec::new(),
                }],
            },
        }
    }

    fn test_prepared_classification(thread_id: &str) -> PreparedClassification {
        PreparedClassification {
            previous_state: None,
            classification: ThreadClassification {
                thread_id: thread_id.to_string(),
                state: ThreadState::QuotationReplyPending,
                requester_name: None,
                requester_email: None,
                requester_org: None,
                subject: Some("Subject".to_string()),
                latest_customer_message_at: None,
                latest_company_message_at: None,
                quote_sent_at: None,
                latest_relevant_message_at: None,
                extracted_requirement: None,
                short_reason: "Needs a reply".to_string(),
                confidence: 0.9,
            },
        }
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 7, 4).unwrap()
    }
}
