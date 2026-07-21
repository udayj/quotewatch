mod claude;
mod common;
mod fireworks;
mod guards;
mod types;

use crate::domain::types::{ClassificationResult, ThreadDigest};
use claude::ClaudeClassifier;
use fireworks::FireworksClassifier;
pub use types::ClassifierError;
use types::ClassifierResult;

pub struct Classifier {
    provider: ClassifierProvider,
}

impl Classifier {
    pub fn from_env() -> ClassifierResult<Self> {
        let provider_name = std::env::var("CLASSIFIER_PROVIDER")
            .or_else(|_| std::env::var("LLM_PROVIDER"))
            .unwrap_or_else(|_| "fireworks".to_string());
        let provider = match provider_name.trim().to_lowercase().as_str() {
            "fireworks" | "fireworks_hq" | "kimi" | "kimi-k2p6" => {
                ClassifierProvider::Fireworks(FireworksClassifier::from_env()?)
            }
            "claude" | "anthropic" | "sonnet" | "sonnet-4.6" => {
                ClassifierProvider::Claude(ClaudeClassifier::from_env()?)
            }
            other => {
                return Err(ClassifierError::Config(format!(
                    "unsupported CLASSIFIER_PROVIDER/LLM_PROVIDER value: {other}"
                )));
            }
        };
        Ok(Self { provider })
    }

    pub fn model(&self) -> &str {
        match &self.provider {
            ClassifierProvider::Fireworks(provider) => provider.model(),
            ClassifierProvider::Claude(provider) => provider.model(),
        }
    }

    pub fn classify(&self, digests: &[ThreadDigest]) -> ClassifierResult<ClassificationResult> {
        match &self.provider {
            ClassifierProvider::Fireworks(provider) => provider.classify(digests),
            ClassifierProvider::Claude(provider) => provider.classify(digests),
        }
    }
}

enum ClassifierProvider {
    Fireworks(FireworksClassifier),
    Claude(ClaudeClassifier),
}
