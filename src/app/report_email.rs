use super::error::AppError;
use crate::domain::types::{ReportItem, ThreadState};
use crate::error::Result;
use crate::integrations::gmail;
use chrono::{Datelike, NaiveDate};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub(super) struct ReportEmailSummary {
    quotation_pending: usize,
    followup_pending: usize,
}

impl ReportEmailSummary {
    pub(super) fn from_items(items: &[ReportItem]) -> Self {
        let quotation_pending = items
            .iter()
            .filter(|item| item.state == ThreadState::QuotationReplyPending)
            .count();
        let followup_pending = items
            .iter()
            .filter(|item| item.state == ThreadState::FollowupPending)
            .count();
        Self {
            quotation_pending,
            followup_pending,
        }
    }
}

pub(super) fn send_report_email_if_configured(
    token_store: Option<&dyn gmail::OAuthTokenStore>,
    client_secret: &Path,
    report_body: &str,
    html_body: Option<&str>,
    summary: ReportEmailSummary,
    run_date: Option<NaiveDate>,
) -> Result<()> {
    let Some(to) = std::env::var("REPORT_EMAIL_TO")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };

    let from = std::env::var("REPORT_EMAIL_FROM")
        .map_err(|_| AppError::MissingEnv("REPORT_EMAIL_FROM"))?;
    let from_header = report_email_from_header(&from);
    let token_path = std::env::var("REPORT_EMAIL_TOKEN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("report_token.json"));
    let subject = report_email_subject(summary, run_date);

    let send_result = if let Some(html_body) = html_body {
        gmail::send_html_email(
            &from,
            gmail::HtmlEmail {
                from_header: &from_header,
                to: &to,
                subject: &subject,
                text_body: report_body,
                html_body,
            },
            client_secret,
            token_path.as_path(),
            token_store,
        )
    } else {
        gmail::send_email(
            &from,
            gmail::TextEmail {
                from_header: &from_header,
                to: &to,
                subject: &subject,
                body: report_body,
            },
            client_secret,
            token_path.as_path(),
            token_store,
        )
    };

    if let Err(error) = &send_result {
        eprintln!(
            "report email send failed; committed DB state should still be correct; from={from}; to={to}; subject={subject}: {error}"
        );
    }
    send_result?;

    println!("Sent concise report to {to}");
    Ok(())
}

fn report_email_from_header(address: &str) -> String {
    let Some(name) = std::env::var("REPORT_EMAIL_FROM_NAME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return address.to_string();
    };

    format!("{} <{}>", quote_display_name(&name), address.trim())
}

fn report_email_subject(summary: ReportEmailSummary, run_date: Option<NaiveDate>) -> String {
    let base = format!(
        "{} {}, {} {} Pending",
        summary.quotation_pending,
        pluralize(summary.quotation_pending, "Quote", "Quotes"),
        summary.followup_pending,
        pluralize(summary.followup_pending, "Followup", "Followups")
    );
    match run_date {
        Some(date) => format!("{base} - {}", format_subject_date(date)),
        None => base.to_string(),
    }
}

fn pluralize(count: usize, singular: &'static str, plural: &'static str) -> &'static str {
    if count == 1 { singular } else { plural }
}

fn format_subject_date(date: NaiveDate) -> String {
    format!(
        "{} {}, {}",
        ordinal_day(date.day()),
        date.format("%B"),
        date.year()
    )
}

fn ordinal_day(day: u32) -> String {
    let suffix = match day % 100 {
        11..=13 => "th",
        _ => match day % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        },
    };
    format!("{day}{suffix}")
}

fn quote_display_name(name: &str) -> String {
    format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_appends_friendly_run_date() {
        let date = NaiveDate::from_ymd_opt(2026, 6, 17).unwrap();

        assert_eq!(
            report_email_subject(
                ReportEmailSummary {
                    quotation_pending: 3,
                    followup_pending: 5,
                },
                Some(date)
            ),
            "3 Quotes, 5 Followups Pending - 17th June, 2026"
        );
    }

    #[test]
    fn subject_uses_singular_labels() {
        assert_eq!(
            report_email_subject(
                ReportEmailSummary {
                    quotation_pending: 1,
                    followup_pending: 1,
                },
                None
            ),
            "1 Quote, 1 Followup Pending"
        );
    }

    #[test]
    fn summary_counts_actionable_report_items() {
        let summary = ReportEmailSummary::from_items(&[
            report_item(ThreadState::QuotationReplyPending),
            report_item(ThreadState::QuotationReplyPending),
            report_item(ThreadState::FollowupPending),
            report_item(ThreadState::Ignored),
        ]);

        assert_eq!(summary.quotation_pending, 2);
        assert_eq!(summary.followup_pending, 1);
    }

    #[test]
    fn ordinal_day_handles_teens() {
        assert_eq!(ordinal_day(1), "1st");
        assert_eq!(ordinal_day(2), "2nd");
        assert_eq!(ordinal_day(3), "3rd");
        assert_eq!(ordinal_day(11), "11th");
        assert_eq!(ordinal_day(12), "12th");
        assert_eq!(ordinal_day(13), "13th");
        assert_eq!(ordinal_day(21), "21st");
    }

    fn report_item(state: ThreadState) -> ReportItem {
        ReportItem {
            thread_id: "thread-1".to_string(),
            account: Some("sales@example.com".to_string()),
            state,
            requester_name: None,
            requester_email: None,
            requester_org: None,
            subject: None,
            latest_customer_message_at: None,
            latest_company_message_at: None,
            quote_sent_at: None,
            extracted_requirement: None,
            short_reason: None,
            confidence: 0.0,
            expires_on: None,
        }
    }
}
