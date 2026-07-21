use super::oauth::{GMAIL_SEND_SCOPE, TokenRequest, get_access_token, read_oauth_client};
use super::types::{GmailResult, HtmlEmail, OAuthTokenStore, TextEmail};
use crate::integrations::http::RetryableHttpClient;
use base64::Engine;
use serde_json::json;
use std::path::Path;

pub fn send_email(
    account: &str,
    email: TextEmail<'_>,
    client_secret_path: &Path,
    token_path: &Path,
    token_store: Option<&dyn OAuthTokenStore>,
) -> GmailResult<()> {
    let oauth = read_oauth_client(client_secret_path)?;
    let client = RetryableHttpClient::gmail()?;
    let token = get_access_token(
        &client,
        &oauth,
        TokenRequest {
            account,
            token_path,
            token_store,
            token_env_var: "REPORT_EMAIL_TOKEN_JSON",
            required_scopes: &[GMAIL_SEND_SCOPE],
            permission_label: "Gmail send",
            token_purpose: "gmail_send",
        },
    )?;
    let raw = raw_email(email.from_header, email.to, email.subject, email.body);
    client
        .post("https://gmail.googleapis.com/gmail/v1/users/me/messages/send")
        .bearer_auth(&token.access_token)
        .json(&json!({ "raw": raw }))
        .send()?
        .error_for_status()?;
    Ok(())
}

pub fn send_html_email(
    account: &str,
    email: HtmlEmail<'_>,
    client_secret_path: &Path,
    token_path: &Path,
    token_store: Option<&dyn OAuthTokenStore>,
) -> GmailResult<()> {
    let oauth = read_oauth_client(client_secret_path)?;
    let client = RetryableHttpClient::gmail()?;
    let token = get_access_token(
        &client,
        &oauth,
        TokenRequest {
            account,
            token_path,
            token_store,
            token_env_var: "REPORT_EMAIL_TOKEN_JSON",
            required_scopes: &[GMAIL_SEND_SCOPE],
            permission_label: "Gmail send",
            token_purpose: "gmail_send",
        },
    )?;
    let raw = raw_html_email(
        email.from_header,
        email.to,
        email.subject,
        email.text_body,
        email.html_body,
    );
    client
        .post("https://gmail.googleapis.com/gmail/v1/users/me/messages/send")
        .bearer_auth(&token.access_token)
        .json(&json!({ "raw": raw }))
        .send()?
        .error_for_status()?;
    Ok(())
}

fn raw_email(from: &str, to: &str, subject: &str, body: &str) -> String {
    let message = format!(
        "From: {from}\r\nTo: {to}\r\nSubject: {subject}\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\n{body}"
    );
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(message.as_bytes())
}

fn raw_html_email(from: &str, to: &str, subject: &str, text_body: &str, html_body: &str) -> String {
    let boundary = "quotewatch-report-boundary";
    let message = format!(
        "From: {from}\r\nTo: {to}\r\nSubject: {subject}\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"{boundary}\"\r\n\r\n--{boundary}\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\n{text_body}\r\n\r\n--{boundary}\r\nContent-Type: text/html; charset=UTF-8\r\n\r\n{html_body}\r\n\r\n--{boundary}--"
    );
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(message.as_bytes())
}
