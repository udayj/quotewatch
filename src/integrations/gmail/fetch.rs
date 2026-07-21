use super::oauth::{GMAIL_READONLY_SCOPE, TokenRequest, get_access_token, read_oauth_client};
use super::parse::parse_thread;
use super::types::{GmailError, GmailResult, OAuthTokenStore};
use crate::domain::types::{GmailExport, GmailThread};
use crate::integrations::http::RetryableHttpClient;
use chrono::{Duration, NaiveDate, Utc};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub fn fetch_gmail_export(
    account: &str,
    start: NaiveDate,
    end: NaiveDate,
    limit: usize,
    client_secret_path: &Path,
    token_path: &Path,
    token_store: Option<&dyn OAuthTokenStore>,
) -> GmailResult<GmailExport> {
    if end < start {
        return Err(GmailError::InvalidDateWindow(
            "end date must be on or after start date".to_string(),
        ));
    }

    // The OAuth client identifies this app; the access token below identifies
    // the Gmail account and granted read-only permission.
    let oauth = read_oauth_client(client_secret_path)?;
    let client = RetryableHttpClient::gmail()?;
    let token = get_access_token(
        &client,
        &oauth,
        TokenRequest {
            account,
            token_path,
            token_store,
            token_env_var: "GMAIL_TOKEN_JSON",
            required_scopes: &[GMAIL_READONLY_SCOPE],
            permission_label: "Gmail read-only",
            token_purpose: "gmail_read",
        },
    )?;
    let query = gmail_date_query(start, end);
    let control_label_names_by_id = control_label_names_by_id(&client, &token.access_token)?;
    let threads = export_threads(
        &client,
        &token.access_token,
        &query,
        limit,
        &control_label_names_by_id,
    )?;

    Ok(GmailExport {
        account: account.to_string(),
        query,
        date_window_start: start,
        date_window_end: end,
        generated_at: Utc::now().to_rfc3339(),
        thread_count: threads.len(),
        threads,
    })
}

pub fn fetch_gmail_threads_by_id(
    account: &str,
    thread_ids: &[String],
    client_secret_path: &Path,
    token_path: &Path,
    token_store: Option<&dyn OAuthTokenStore>,
) -> GmailResult<Vec<GmailThread>> {
    if thread_ids.is_empty() {
        return Ok(Vec::new());
    }

    let oauth = read_oauth_client(client_secret_path)?;
    let client = RetryableHttpClient::gmail()?;
    let token = get_access_token(
        &client,
        &oauth,
        TokenRequest {
            account,
            token_path,
            token_store,
            token_env_var: "GMAIL_TOKEN_JSON",
            required_scopes: &[GMAIL_READONLY_SCOPE],
            permission_label: "Gmail read-only",
            token_purpose: "gmail_read",
        },
    )?;
    let control_label_names_by_id = control_label_names_by_id(&client, &token.access_token)?;
    export_threads_by_id(
        &client,
        &token.access_token,
        thread_ids,
        &control_label_names_by_id,
    )
}

fn gmail_date_query(start: NaiveDate, end: NaiveDate) -> String {
    let before = end + Duration::days(1);
    format!(
        "after:{} before:{}",
        start.format("%Y/%m/%d"),
        before.format("%Y/%m/%d")
    )
}

fn export_threads(
    client: &RetryableHttpClient,
    access_token: &str,
    query: &str,
    limit: usize,
    control_label_names_by_id: &HashMap<String, String>,
) -> GmailResult<Vec<GmailThread>> {
    let mut page_token: Option<String> = None;
    let mut seen = HashSet::new();
    let mut thread_ids = Vec::new();

    while thread_ids.len() < limit {
        let mut req = client
            .get("https://gmail.googleapis.com/gmail/v1/users/me/messages")
            .bearer_auth(access_token)
            .query(&[
                ("q", query),
                ("maxResults", "100"),
                ("fields", "messages(id,threadId),nextPageToken"),
            ]);
        if let Some(token) = page_token.as_deref() {
            req = req.query(&[("pageToken", token)]);
        }

        let list: Value = client.execute("Gmail message list", req)?;
        if let Some(messages) = list.get("messages").and_then(Value::as_array) {
            for msg in messages {
                if let Some(thread_id) = msg.get("threadId").and_then(Value::as_str)
                    && seen.insert(thread_id.to_string())
                {
                    thread_ids.push(thread_id.to_string());
                    if thread_ids.len() == limit {
                        break;
                    }
                }
            }
        }

        page_token = list
            .get("nextPageToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if page_token.is_none() {
            break;
        }
    }

    export_threads_by_id(client, access_token, &thread_ids, control_label_names_by_id)
}

fn export_threads_by_id(
    client: &RetryableHttpClient,
    access_token: &str,
    thread_ids: &[String],
    control_label_names_by_id: &HashMap<String, String>,
) -> GmailResult<Vec<GmailThread>> {
    let mut threads = Vec::new();
    for thread_id in thread_ids {
        let url = format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/threads/{}",
            thread_id
        );
        let operation = format!("Gmail thread {thread_id}");
        let thread: Value = client.execute(
            &operation,
            client
                .get(url)
                .bearer_auth(access_token)
                .query(&[("format", "full")]),
        )?;
        threads.push(parse_thread(thread, control_label_names_by_id));
    }
    Ok(threads)
}

fn control_label_names_by_id(
    client: &RetryableHttpClient,
    access_token: &str,
) -> GmailResult<HashMap<String, String>> {
    const CONTROL_LABELS: [&str; 3] = ["Ignore", "Order Received", "Order Closed"];

    let labels: Value = client.execute(
        "Gmail labels",
        client
            .get("https://gmail.googleapis.com/gmail/v1/users/me/labels")
            .bearer_auth(access_token)
            .query(&[("fields", "labels(id,name)")]),
    )?;

    let mut names_by_id = HashMap::new();
    if let Some(labels) = labels.get("labels").and_then(Value::as_array) {
        for label in labels {
            let Some(id) = label.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(name) = label.get("name").and_then(Value::as_str) else {
                continue;
            };
            if CONTROL_LABELS.contains(&name) {
                names_by_id.insert(id.to_string(), name.to_string());
            }
        }
    }
    Ok(names_by_id)
}
