use super::cli::{Cli, Command, ReportArgs, RunArgs};
use super::dates::resolve_dates;
use super::error::AppError;
use super::export::{export_json, gmail_account_from_env};
use super::preparation::prepare_run;
use super::report_email::{ReportEmailSummary, send_report_email_if_configured};
use crate::domain::types::{ReportItem, RunCostSummary};
use crate::error::Result;
use crate::integrations::classifier::Classifier;
use crate::integrations::gmail;
use crate::reporting::report::{ReportFormat, render_concise_report_html, render_report};
use crate::storage::database::Database;
use chrono::{Days, NaiveDate};
use std::path::Path;

pub fn execute(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Run(args) => run(cli.db.as_path(), args),
        Command::Report(args) => report(cli.db.as_path(), args),
        Command::ExportJson(args) => export_json(args),
    }
}

fn run(db_path: &Path, args: RunArgs) -> Result<()> {
    let (start, end) = resolve_dates(args.date, args.start, args.end)?;
    let account = gmail_account_from_env()?;
    let db = Database::open(db_path)?;
    if args.input_json.is_some() && start != end {
        return Err(AppError::InvalidArgs(
            "--input-json can only be used with a single --date".to_string(),
        )
        .into());
    }
    let classifier = if args.skip_classify {
        None
    } else {
        Some(Classifier::from_env()?)
    };

    let mut current = start;
    loop {
        run_one_day(&db, classifier.as_ref(), &args, &account, current)?;
        if current == end {
            break;
        }
        current = current
            .checked_add_days(Days::new(1))
            .ok_or(AppError::DateOverflow)?;
    }

    let report_items = db.report_items()?;
    let cost = db.latest_run_cost_summary()?;
    print_and_email_concise_report(
        db.token_store(),
        &args.client_secret,
        &report_items,
        cost.as_ref(),
        Some(end),
    )?;
    Ok(())
}

// Processes one as-of date by preparing all changes before the atomic DB commit.
fn run_one_day(
    db: &Database,
    classifier: Option<&Classifier>,
    args: &RunArgs,
    account: &str,
    date: NaiveDate,
) -> Result<()> {
    let prepared = prepare_run(db, classifier, args, account, date)?;
    db.commit_prepared_run(&prepared)?;
    Ok(())
}

fn report(db_path: &Path, args: ReportArgs) -> Result<()> {
    let db = Database::open(db_path)?;
    let items = db.report_items()?;
    let cost = db.latest_run_cost_summary()?;
    let run_date = db.latest_run_date()?;
    let format = args.format.into();
    if args.email && matches!(format, ReportFormat::Concise) {
        return print_and_email_concise_report(
            db.token_store(),
            &args.client_secret,
            &items,
            cost.as_ref(),
            run_date,
        );
    }

    let report_body = render_report(&items, format, cost.as_ref());
    println!("{report_body}");
    if args.email {
        send_report_email_if_configured(
            db.token_store(),
            &args.client_secret,
            &report_body,
            None,
            ReportEmailSummary::from_items(&items),
            run_date,
        )?;
    }
    Ok(())
}

fn print_and_email_concise_report(
    token_store: Option<&dyn gmail::OAuthTokenStore>,
    client_secret: &Path,
    report_items: &[ReportItem],
    cost: Option<&RunCostSummary>,
    run_date: Option<NaiveDate>,
) -> Result<()> {
    let report_body = render_report(report_items, ReportFormat::Concise, cost);
    let email_body = render_concise_report_html(report_items, cost);
    println!("{report_body}");
    send_report_email_if_configured(
        token_store,
        client_secret,
        &report_body,
        Some(&email_body),
        ReportEmailSummary::from_items(report_items),
        run_date,
    )
}
