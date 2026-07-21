use super::non_empty_env;
use crate::integrations::gmail::types::{
    GmailError, GmailResult, OAuthClient, OAuthTokenStore, StoredToken, TokenResponse,
};
use crate::integrations::http::RetryableHttpClient;
use chrono::Utc;
use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::time::Duration;
use tiny_http::{Response, Server};

pub(crate) const GMAIL_READONLY_SCOPE: &str = "https://www.googleapis.com/auth/gmail.readonly";
pub(crate) const GMAIL_SEND_SCOPE: &str = "https://www.googleapis.com/auth/gmail.send";

const CALLBACK: &str = "http://127.0.0.1:8765/oauth2callback";
const INTERACTIVE_OAUTH_CALLBACK_TIMEOUT_SECS: u64 = 600;

pub(crate) struct TokenRequest<'a> {
    pub(crate) account: &'a str,
    pub(crate) token_path: &'a Path,
    pub(crate) token_store: Option<&'a dyn OAuthTokenStore>,
    pub(crate) token_env_var: &'static str,
    pub(crate) required_scopes: &'a [&'static str],
    pub(crate) permission_label: &'static str,
    pub(crate) token_purpose: &'static str,
}

pub(crate) fn get_access_token(
    client: &RetryableHttpClient,
    oauth: &OAuthClient,
    request: TokenRequest<'_>,
) -> GmailResult<StoredToken> {
    let token_key = token_key(request.token_purpose, request.account);
    if let Some(store) = request.token_store
        && let Some(stored) = store
            .load_token(&token_key)
            .map_err(GmailError::TokenStore)?
    {
        // Reuse a stored access token only when it has the requested scope and
        // enough lifetime left to avoid expiring during the Gmail API call.
        if token_has_scopes(&stored, request.required_scopes)
            && stored.expires_at > Utc::now().timestamp() + 60
        {
            return Ok(stored);
        }

        // Expired access tokens can be replaced without user interaction when
        // Google previously issued a refresh token for the same permission set.
        if token_has_scopes(&stored, request.required_scopes)
            && let Some(refresh_value) = stored.refresh_token.as_deref()
        {
            return refresh_token(
                client,
                oauth,
                refresh_value,
                request.token_path,
                request.token_store,
                &token_key,
                &stored.scopes,
            );
        }
    }

    // Bootstrap tokens can come from secrets or local token files. When DB token
    // storage is enabled, seed it from that bootstrap source for future runs.
    if let Some(stored) = read_stored_token(request.token_path, request.token_env_var)? {
        if let Some(store) = request.token_store {
            store
                .save_token(&token_key, &stored)
                .map_err(GmailError::TokenStore)?;
        }
        if token_has_scopes(&stored, request.required_scopes)
            && stored.expires_at > Utc::now().timestamp() + 60
        {
            return Ok(stored);
        }
        if token_has_scopes(&stored, request.required_scopes)
            && let Some(refresh_value) = stored.refresh_token.as_deref()
        {
            return refresh_token(
                client,
                oauth,
                refresh_value,
                request.token_path,
                request.token_store,
                &token_key,
                &stored.scopes,
            );
        }
    }

    let stored = authorize_interactively(
        client,
        oauth,
        request.account,
        request.token_path,
        request.required_scopes,
        request.permission_label,
    )?;
    if let Some(store) = request.token_store {
        store
            .save_token(&token_key, &stored)
            .map_err(GmailError::TokenStore)?;
    }
    Ok(stored)
}

fn read_stored_token(
    token_path: &Path,
    token_env_var: &'static str,
) -> GmailResult<Option<StoredToken>> {
    if let Some(raw) = non_empty_env(token_env_var) {
        return serde_json::from_str(&raw).map(Some).map_err(|source| {
            GmailError::InvalidTokenEnv {
                var: token_env_var,
                source,
            }
        });
    }

    match fs::read_to_string(token_path) {
        Ok(raw) => return serde_json::from_str(&raw).map(Some).map_err(Into::into),
        Err(source) if source.kind() == ErrorKind::NotFound => {}
        Err(source) => {
            return Err(GmailError::ReadToken {
                path: token_path.display().to_string(),
                source,
            });
        }
    }
    Ok(None)
}

fn token_key(purpose: &str, account: &str) -> String {
    format!("{purpose}:{}", account.trim().to_lowercase())
}

fn refresh_token(
    client: &RetryableHttpClient,
    oauth: &OAuthClient,
    refresh_token: &str,
    token_path: &Path,
    token_store: Option<&dyn OAuthTokenStore>,
    token_key: &str,
    existing_scopes: &[String],
) -> GmailResult<StoredToken> {
    // OAuth refresh-token grant: send the long-lived refresh token plus the app
    // client credentials to Google's token endpoint, and receive a new short-lived
    // access token. Google often omits refresh_token in this response, so keep the
    // existing refresh token unless a replacement is explicitly returned.
    let res: TokenResponse = client.execute(
        "Google OAuth refresh token",
        client.post(&oauth.token_uri).form(&[
            ("client_id", oauth.client_id.as_str()),
            ("client_secret", oauth.client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ]),
    )?;

    let scopes = scopes_from_response(res.scope.as_deref(), existing_scopes);
    let stored = StoredToken {
        access_token: res.access_token,
        refresh_token: res
            .refresh_token
            .or_else(|| Some(refresh_token.to_string())),
        expires_at: Utc::now().timestamp() + res.expires_in.unwrap_or(3600),
        scopes,
    };
    if let Some(store) = token_store {
        store
            .save_token(token_key, &stored)
            .map_err(GmailError::TokenStore)?;
    } else {
        fs::write(token_path, serde_json::to_string_pretty(&stored)?)
            .map_err(GmailError::WriteToken)?;
    }
    Ok(stored)
}

fn authorize_interactively(
    client: &RetryableHttpClient,
    oauth: &OAuthClient,
    account: &str,
    token_path: &Path,
    scopes: &[&str],
    permission_label: &str,
) -> GmailResult<StoredToken> {
    // First-time OAuth flow for local/interactive runs:
    // 1. Print a Google consent URL with the requested Gmail scopes.
    // 2. Listen on localhost for Google's redirect containing an auth code.
    // 3. Exchange that short-lived code for access/refresh tokens.
    // Hosted runs should normally avoid this path by providing bootstrap tokens.
    let state = format!("quotewatch-{}", Utc::now().timestamp());
    let url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&access_type=offline&prompt=consent&login_hint={}&state={}",
        oauth.auth_uri,
        urlencoding::encode(&oauth.client_id),
        urlencoding::encode(CALLBACK),
        urlencoding::encode(&scopes.join(" ")),
        urlencoding::encode(account),
        urlencoding::encode(&state)
    );

    println!("\nOpen this URL and approve {permission_label} access for {account}:\n\n{url}\n");
    let server =
        Server::http("127.0.0.1:8765").map_err(|err| GmailError::OAuthCallback(err.to_string()))?;
    let request = server
        .recv_timeout(Duration::from_secs(
            INTERACTIVE_OAUTH_CALLBACK_TIMEOUT_SECS,
        ))
        .map_err(|err| GmailError::OAuthCallback(err.to_string()))?
        .ok_or_else(|| {
            eprintln!(
                "interactive OAuth timed out after {INTERACTIVE_OAUTH_CALLBACK_TIMEOUT_SECS}s waiting for {permission_label} callback for account {account}; hosted runs should provide a valid stored token or refresh token"
            );
            GmailError::OAuthCallback(format!(
                "interactive OAuth timed out after {INTERACTIVE_OAUTH_CALLBACK_TIMEOUT_SECS}s waiting for localhost callback"
            ))
        })?;
    let path = request.url().to_string();
    let params = parse_query_params(&path);
    request
        .respond(Response::from_string(
            "Authorization received. You can close this browser tab.",
        ))
        .map_err(|err| GmailError::OAuthCallback(err.to_string()))?;

    if params.get("state") != Some(&state) {
        return Err(GmailError::OAuthCallback(
            "OAuth state mismatch".to_string(),
        ));
    }
    let code = params.get("code").ok_or_else(|| {
        GmailError::OAuthCallback("OAuth callback did not include a code".to_string())
    })?;

    let res: TokenResponse = client
        .post(&oauth.token_uri)
        .form(&[
            ("client_id", oauth.client_id.as_str()),
            ("client_secret", oauth.client_secret.as_str()),
            ("code", code.as_str()),
            ("redirect_uri", CALLBACK),
            ("grant_type", "authorization_code"),
        ])
        .send()?
        .error_for_status()?
        .json()?;

    let requested_scopes = scopes
        .iter()
        .map(|scope| scope.to_string())
        .collect::<Vec<_>>();
    let granted_scopes = scopes_from_response(res.scope.as_deref(), &requested_scopes);
    let stored = StoredToken {
        access_token: res.access_token,
        refresh_token: res.refresh_token,
        expires_at: Utc::now().timestamp() + res.expires_in.unwrap_or(3600),
        scopes: granted_scopes,
    };
    fs::write(token_path, serde_json::to_string_pretty(&stored)?)
        .map_err(GmailError::WriteToken)?;
    Ok(stored)
}

fn token_has_scopes(stored: &StoredToken, required_scopes: &[&str]) -> bool {
    // A token is reusable only if it covers every Gmail permission the caller
    // needs. Older token files may not record scopes; treat those as read-only
    // tokens because the original app only requested gmail.readonly.
    if stored.scopes.is_empty() {
        return required_scopes.len() == 1 && required_scopes[0] == GMAIL_READONLY_SCOPE;
    }
    required_scopes.iter().all(|scope| {
        stored
            .scopes
            .iter()
            .any(|stored_scope| stored_scope == scope)
    })
}

fn scopes_from_response(scope: Option<&str>, fallback: &[String]) -> Vec<String> {
    scope
        .map(|scope| scope.split_whitespace().map(str::to_string).collect())
        .unwrap_or_else(|| fallback.to_vec())
}

fn parse_query_params(path: &str) -> HashMap<String, String> {
    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
    query
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((
                urlencoding::decode(key).ok()?.into_owned(),
                urlencoding::decode(value).ok()?.into_owned(),
            ))
        })
        .collect()
}
