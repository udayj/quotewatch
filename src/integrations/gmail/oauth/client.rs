use super::{GOOGLE_AUTH_URI, GOOGLE_TOKEN_URI, non_empty_env};
use crate::integrations::gmail::types::{ClientSecretFile, GmailError, GmailResult, OAuthClient};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

pub(crate) fn read_oauth_client(path: &Path) -> GmailResult<OAuthClient> {
    // Hosted runs can provide the full Google OAuth client JSON as one secret.
    if let Some(raw) = non_empty_env("GOOGLE_OAUTH_CLIENT_JSON") {
        let parsed: ClientSecretFile =
            serde_json::from_str(&raw).map_err(|source| GmailError::InvalidOAuthClientEnv {
                var: "GOOGLE_OAUTH_CLIENT_JSON",
                source,
            })?;
        return oauth_client_from_secret_file(parsed);
    }

    // If either individual field is set, treat env configuration as intentional
    // and require the complete client id/secret pair there.
    if non_empty_env("GOOGLE_OAUTH_CLIENT_ID").is_some()
        || non_empty_env("GOOGLE_OAUTH_CLIENT_SECRET").is_some()
    {
        return oauth_client_from_env(path);
    }

    // Local runs commonly use Google's downloaded client_secret.json file.
    match fs::read_to_string(path) {
        Ok(raw) => parse_oauth_client_file(&raw),
        Err(source) if source.kind() == ErrorKind::NotFound => {
            Err(GmailError::MissingOAuthClient {
                path: path.display().to_string(),
            })
        }
        Err(source) => Err(GmailError::ReadOAuthClient {
            path: path.display().to_string(),
            source,
        }),
    }
}

fn parse_oauth_client_file(raw: &str) -> GmailResult<OAuthClient> {
    let parsed: ClientSecretFile = serde_json::from_str(raw)?;
    oauth_client_from_secret_file(parsed)
}

fn oauth_client_from_secret_file(parsed: ClientSecretFile) -> GmailResult<OAuthClient> {
    parsed.installed.or(parsed.web).ok_or_else(|| {
        GmailError::InvalidOAuthClient(
            "client secret must contain 'installed' or 'web'".to_string(),
        )
    })
}

fn oauth_client_from_env(path: &Path) -> GmailResult<OAuthClient> {
    let Some(client_id) = non_empty_env("GOOGLE_OAUTH_CLIENT_ID") else {
        return Err(GmailError::MissingOAuthClient {
            path: path.display().to_string(),
        });
    };
    let Some(client_secret) = non_empty_env("GOOGLE_OAUTH_CLIENT_SECRET") else {
        return Err(GmailError::MissingOAuthClient {
            path: path.display().to_string(),
        });
    };

    Ok(OAuthClient {
        client_id,
        client_secret,
        auth_uri: non_empty_env("GOOGLE_OAUTH_AUTH_URI").unwrap_or_else(|| GOOGLE_AUTH_URI.into()),
        token_uri: non_empty_env("GOOGLE_OAUTH_TOKEN_URI")
            .unwrap_or_else(|| GOOGLE_TOKEN_URI.into()),
    })
}
