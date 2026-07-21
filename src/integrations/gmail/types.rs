use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type GmailResult<T> = std::result::Result<T, GmailError>;
pub type OAuthTokenStoreResult<T> = std::result::Result<T, String>;

pub trait OAuthTokenStore {
    fn load_token(&self, token_key: &str) -> OAuthTokenStoreResult<Option<StoredToken>>;
    fn save_token(&self, token_key: &str, token: &StoredToken) -> OAuthTokenStoreResult<()>;
}

/// Fields for a multipart report email sent through Gmail.
pub struct TextEmail<'a> {
    pub from_header: &'a str,
    pub to: &'a str,
    pub subject: &'a str,
    pub body: &'a str,
}

/// Fields for a multipart report email sent through Gmail.
pub struct HtmlEmail<'a> {
    pub from_header: &'a str,
    pub to: &'a str,
    pub subject: &'a str,
    pub text_body: &'a str,
    pub html_body: &'a str,
}

#[derive(Debug, Error)]
pub enum GmailError {
    #[error("invalid date window: {0}")]
    InvalidDateWindow(String),

    #[error("failed to read OAuth client file {path}: {source}")]
    ReadOAuthClient {
        path: String,
        source: std::io::Error,
    },

    #[error(
        "OAuth client not found: file {path} is missing and GOOGLE_OAUTH_CLIENT_JSON or GOOGLE_OAUTH_CLIENT_ID/GOOGLE_OAUTH_CLIENT_SECRET are not set"
    )]
    MissingOAuthClient { path: String },

    #[error("invalid OAuth client environment variable {var}: {source}")]
    InvalidOAuthClientEnv {
        var: &'static str,
        source: serde_json::Error,
    },

    #[error("invalid OAuth client file: {0}")]
    InvalidOAuthClient(String),

    #[error("failed to read token file {path}: {source}")]
    ReadToken {
        path: String,
        source: std::io::Error,
    },

    #[error("invalid token environment variable {var}: {source}")]
    InvalidTokenEnv {
        var: &'static str,
        source: serde_json::Error,
    },

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("HTTP retry error: {0}")]
    HttpRetry(String),

    #[error("failed to write token file: {0}")]
    WriteToken(std::io::Error),

    #[error("OAuth token store error: {0}")]
    TokenStore(String),

    #[error("OAuth callback error: {0}")]
    OAuthCallback(String),
}

#[derive(Debug, Deserialize)]
pub(super) struct ClientSecretFile {
    pub(super) installed: Option<OAuthClient>,
    pub(super) web: Option<OAuthClient>,
}

#[derive(Debug, Deserialize)]
pub(super) struct OAuthClient {
    pub(super) client_id: String,
    pub(super) client_secret: String,
    pub(super) auth_uri: String,
    pub(super) token_uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: i64,
    #[serde(default)]
    pub scopes: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TokenResponse {
    pub(super) access_token: String,
    pub(super) expires_in: Option<i64>,
    pub(super) refresh_token: Option<String>,
    pub(super) scope: Option<String>,
}

impl From<crate::integrations::http::HttpError> for GmailError {
    fn from(error: crate::integrations::http::HttpError) -> Self {
        Self::HttpRetry(error.to_string())
    }
}
