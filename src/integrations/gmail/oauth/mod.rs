mod client;
mod token;

pub(crate) use client::read_oauth_client;
pub(crate) use token::{GMAIL_READONLY_SCOPE, GMAIL_SEND_SCOPE, TokenRequest, get_access_token};

const GOOGLE_AUTH_URI: &str = "https://accounts.google.com/o/oauth2/auth";
const GOOGLE_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
