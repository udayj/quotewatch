use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub(crate) struct Scenario {
    pub(crate) name: String,
    pub(crate) batch_size: Option<usize>,
    pub(crate) dates: Vec<ScenarioDate>,
    pub(crate) expected: Expected,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ScenarioDate {
    pub(crate) date: String,
    pub(crate) export: PathBuf,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Expected {
    #[serde(default)]
    pub(crate) prompt_hash: Option<String>,
    #[serde(default)]
    pub(crate) tool_hash: Option<String>,
    #[serde(default)]
    pub(crate) threads_seen_by_date: BTreeMap<String, usize>,
    #[serde(default)]
    pub(crate) state_counts_by_date: BTreeMap<String, BTreeMap<String, usize>>,
    #[serde(default)]
    pub(crate) final_threads: BTreeMap<String, ExpectedThread>,
    #[serde(default)]
    pub(crate) transitions: Vec<ExpectedTransition>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ExpectedThread {
    pub(crate) state: Option<String>,
    pub(crate) quote_sent_at: Option<String>,
    pub(crate) latest_customer_message_at: Option<String>,
    pub(crate) latest_company_message_at: Option<String>,
}

#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
pub(crate) struct ExpectedTransition {
    pub(crate) thread_id: String,
    pub(crate) previous_state: Option<String>,
    pub(crate) new_state: String,
    pub(crate) as_of_date: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct EvalArtifact {
    pub(crate) scenario: String,
    pub(crate) generated_at: String,
    pub(crate) model: String,
    pub(crate) prompt_hash: String,
    pub(crate) tool_hash: String,
    pub(crate) totals: UsageTotals,
    pub(crate) dates: BTreeMap<String, DateActual>,
    pub(crate) final_threads: BTreeMap<String, ThreadActual>,
    pub(crate) transitions: Vec<ExpectedTransition>,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct DateActual {
    pub(crate) threads_seen: usize,
    pub(crate) state_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ThreadActual {
    pub(crate) state: String,
    pub(crate) quote_sent_at: Option<String>,
    pub(crate) latest_customer_message_at: Option<String>,
    pub(crate) latest_company_message_at: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct UsageTotals {
    pub(crate) input_tokens: i64,
    pub(crate) cached_input_tokens: i64,
    pub(crate) cache_creation_input_tokens: i64,
    pub(crate) output_tokens: i64,
    pub(crate) cost_usd: f64,
    pub(crate) cost_inr: f64,
}
