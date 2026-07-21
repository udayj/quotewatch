use super::cli::ExportJsonArgs;
use super::dates::resolve_date_arg;
use super::error::AppError;
use crate::domain::types::GmailExport;
use crate::error::Result;
use crate::integrations::gmail;
use std::fs;
use std::path::Path;

pub(super) fn export_json(args: ExportJsonArgs) -> Result<()> {
    let account = gmail_account_from_env()?;
    let date = resolve_date_arg(args.date)?;
    let export = gmail::fetch_gmail_export(
        &account,
        date,
        date,
        args.limit,
        &args.client_secret,
        &args.token,
        None,
    )?;
    if let Some(parent) = args.out.parent() {
        fs::create_dir_all(parent).map_err(|source| AppError::WriteFile {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let raw = serde_json::to_string_pretty(&export).map_err(AppError::SerializeExport)?;
    fs::write(&args.out, raw).map_err(|source| AppError::WriteFile {
        path: args.out.clone(),
        source,
    })?;
    println!(
        "Wrote {} threads to {}",
        export.thread_count,
        args.out.display()
    );
    Ok(())
}

pub(super) fn gmail_account_from_env() -> Result<String> {
    std::env::var("GMAIL_ACCOUNT")
        .map_err(|_| AppError::MissingEnv("GMAIL_ACCOUNT"))
        .map_err(Into::into)
}

pub(super) fn read_export(path: &Path) -> Result<GmailExport> {
    let raw = fs::read_to_string(path).map_err(|source| AppError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&raw)
        .map_err(|source| AppError::ParseExport {
            path: path.to_path_buf(),
            source,
        })
        .map_err(Into::into)
}
