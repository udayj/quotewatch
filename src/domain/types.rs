use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GmailExport {
    pub account: String,
    pub query: String,
    pub date_window_start: NaiveDate,
    pub date_window_end: NaiveDate,
    pub generated_at: String,
    pub thread_count: usize,
    pub threads: Vec<GmailThread>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GmailThread {
    pub thread_id: String,
    pub history_id: Option<String>,
    #[serde(default)]
    pub control_label_names: Vec<String>,
    pub messages: Vec<GmailMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GmailMessage {
    pub id: String,
    pub thread_id: String,
    #[serde(default)]
    pub label_ids: Vec<String>,
    pub internal_date_ms: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub cc: Option<String>,
    pub date: Option<String>,
    pub subject: Option<String>,
    pub message_id_header: Option<String>,
    pub snippet: Option<String>,
    pub text_body: String,
    pub attachments: Vec<AttachmentMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentMeta {
    pub filename: String,
    pub mime_type: Option<String>,
    pub attachment_id: Option<String>,
    pub size: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadDigest {
    pub thread_id: String,
    pub subject: String,
    pub as_of_date: NaiveDate,
    pub account: String,
    pub messages_kept: usize,
    pub messages_omitted_before_digest: usize,
    pub omitted_keyword_summary: OmittedKeywordSummary,
    pub messages: Vec<DigestMessage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OmittedKeywordSummary {
    pub omitted_total: usize,
    pub external_request_like: usize,
    pub company_quote_like: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigestMessage {
    pub message_id: String,
    pub date: Option<String>,
    pub internal_date_ms: Option<String>,
    pub direction: MessageDirection,
    pub from: Option<String>,
    pub to: Option<String>,
    pub subject: Option<String>,
    pub snippet: Option<String>,
    pub body_excerpt: String,
    pub attachments: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageDirection {
    CompanyToExternal,
    ExternalToCompany,
    InternalOrOther,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ThreadState {
    QuotationReplyPending,
    FollowupPending,
    Ignored,
}

impl ThreadState {
    pub fn as_db_value(&self) -> &'static str {
        match self {
            Self::QuotationReplyPending => "QUOTATION_REPLY_PENDING",
            Self::FollowupPending => "FOLLOWUP_PENDING",
            Self::Ignored => "IGNORED",
        }
    }

    pub fn from_db_value(value: &str) -> Self {
        match value {
            "QUOTATION_REPLY_PENDING" => Self::QuotationReplyPending,
            "FOLLOWUP_PENDING" => Self::FollowupPending,
            _ => Self::Ignored,
        }
    }

    pub fn expiry_days(&self) -> Option<u64> {
        match self {
            Self::QuotationReplyPending => Some(7),
            Self::FollowupPending => Some(14),
            Self::Ignored => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationBatch {
    pub classifications: Vec<ThreadClassification>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadClassification {
    pub thread_id: String,
    pub state: ThreadState,
    pub requester_name: Option<String>,
    pub requester_email: Option<String>,
    pub requester_org: Option<String>,
    pub subject: Option<String>,
    pub latest_customer_message_at: Option<String>,
    pub latest_company_message_at: Option<String>,
    pub quote_sent_at: Option<String>,
    pub latest_relevant_message_at: Option<String>,
    pub extracted_requirement: Option<String>,
    pub short_reason: String,
    pub confidence: f32,
}

#[derive(Debug, Clone)]
pub struct ReportItem {
    pub thread_id: String,
    pub account: Option<String>,
    pub state: ThreadState,
    pub requester_name: Option<String>,
    pub requester_email: Option<String>,
    pub requester_org: Option<String>,
    pub subject: Option<String>,
    pub latest_customer_message_at: Option<String>,
    pub latest_company_message_at: Option<String>,
    pub quote_sent_at: Option<String>,
    pub extracted_requirement: Option<String>,
    pub short_reason: Option<String>,
    pub confidence: f32,
    pub expires_on: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LlmUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub cost_inr: f64,
}

impl LlmUsage {
    pub fn add(&mut self, other: Self) {
        self.input_tokens += other.input_tokens;
        self.cached_input_tokens += other.cached_input_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.output_tokens += other.output_tokens;
        self.cost_usd += other.cost_usd;
        self.cost_inr += other.cost_inr;
    }
}

#[derive(Debug, Clone)]
pub struct ClassificationResult {
    pub batch: ClassificationBatch,
    pub usage: LlmUsage,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RunCostSummary {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub output_tokens: i64,
    pub cost_inr: f64,
    pub elapsed_seconds: Option<i64>,
}
