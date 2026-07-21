use crate::domain::types::{ReportItem, RunCostSummary, ThreadState};
use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone};

pub(crate) fn split_report_items(items: &[ReportItem]) -> (Vec<&ReportItem>, Vec<&ReportItem>) {
    let pending = items
        .iter()
        .filter(|item| item.state == ThreadState::QuotationReplyPending)
        .collect::<Vec<_>>();
    let followups = items
        .iter()
        .filter(|item| item.state == ThreadState::FollowupPending)
        .collect::<Vec<_>>();
    (pending, followups)
}

pub(crate) fn format_summary_counts(quotation_pending: usize, followup_pending: usize) -> String {
    format!("Quotation Pending = {quotation_pending}\nFollowup Pending = {followup_pending}\n\n")
}

pub(crate) fn format_concise_item(
    index: usize,
    item: &ReportItem,
    date_field: DateField,
) -> String {
    let name = display_name(item);
    let date_label = match date_field {
        DateField::Received => "Received",
        DateField::QuoteSent => "Quote sent",
    };
    let date = item_date(item, date_field)
        .map(format_datetime)
        .unwrap_or_else(|| "Unknown".to_string());
    let subject = item.subject.as_deref().unwrap_or("(no subject)");
    let requirement = item
        .extracted_requirement
        .as_deref()
        .map(|value| truncate(value, 100))
        .unwrap_or_else(|| "Not extracted".to_string());

    format!(
        "{}. {}\n   {}: {}\n   Subject: {}\n   Requirement: {}\n\n",
        index, name, date_label, date, subject, requirement
    )
}

pub(crate) fn format_detailed_item(
    index: usize,
    item: &ReportItem,
    date_field: DateField,
) -> String {
    let mut out = format_concise_item(index, item, date_field);
    if let Some(requirement) = item.extracted_requirement.as_deref() {
        out.push_str(&format!("   Requirement: {requirement}\n"));
    }
    if let Some(reason) = item.short_reason.as_deref() {
        out.push_str(&format!("   Reason: {reason}\n"));
    }
    if let Some(quote_sent) = item.quote_sent_at.as_deref() {
        out.push_str(&format!("   Quote date from classifier: {quote_sent}\n"));
    }
    if let Some(latest_company) = item.latest_company_message_at.as_deref() {
        out.push_str(&format!("   Latest company mail: {latest_company}\n"));
    }
    if let Some(expires_on) = item.expires_on.as_deref() {
        out.push_str(&format!("   Active until: {expires_on}\n"));
    }
    out.push_str(&format!("   Confidence: {:.2}\n\n", item.confidence));
    out
}

pub(crate) fn display_name(item: &ReportItem) -> String {
    match (
        item.requester_org.as_deref(),
        item.requester_name.as_deref(),
        item.requester_email.as_deref(),
    ) {
        (Some(org), Some(name), _) if !org.eq_ignore_ascii_case(name) => format!("{org} / {name}"),
        (Some(org), _, _) => org.to_string(),
        (_, Some(name), Some(email)) => format!("{name} <{email}>"),
        (_, Some(name), _) => name.to_string(),
        (_, _, Some(email)) => email.to_string(),
        _ => "Unknown requester".to_string(),
    }
}

pub(crate) fn format_concise_cost(cost: Option<&RunCostSummary>) -> String {
    let amount = cost.map(|cost| cost.cost_inr).unwrap_or_default();
    let elapsed = cost
        .and_then(|cost| cost.elapsed_seconds)
        .map(format_duration)
        .unwrap_or_else(|| "Unknown".to_string());
    format!("Cost: Rs. {:.2}\nRuntime: {}\n\n", amount, elapsed)
}

pub(crate) fn format_detailed_cost(cost: Option<&RunCostSummary>) -> String {
    let Some(cost) = cost else {
        return "Cost: Rs. 0.00\n\n".to_string();
    };
    format!(
        "Cost: Rs. {:.2}\nRuntime: {}\nInput tokens: {}\nCached input tokens: {}\nCache creation input tokens: {}\nOutput tokens: {}\n\n",
        cost.cost_inr,
        cost.elapsed_seconds
            .map(format_duration)
            .unwrap_or_else(|| "Unknown".to_string()),
        cost.input_tokens,
        cost.cached_input_tokens,
        cost.cache_creation_input_tokens,
        cost.output_tokens
    )
}

pub(crate) fn format_duration(seconds: i64) -> String {
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    match (minutes, seconds) {
        (0, seconds) => format!("{seconds} seconds"),
        (1, 0) => "1 minute".to_string(),
        (1, seconds) => format!("1 minute {seconds} seconds"),
        (minutes, 0) => format!("{minutes} minutes"),
        (minutes, seconds) => format!("{minutes} minutes {seconds} seconds"),
    }
}

pub(crate) fn format_datetime(value: &str) -> String {
    parse_datetime(value)
        .map(|datetime| datetime.format("%d-%m-%Y, %I:%M %p").to_string())
        .unwrap_or_else(|| value.to_string())
}

pub(crate) fn item_date(item: &ReportItem, date_field: DateField) -> Option<&str> {
    match date_field {
        DateField::Received => item
            .latest_customer_message_at
            .as_deref()
            .or(item.quote_sent_at.as_deref()),
        DateField::QuoteSent => item
            .latest_company_message_at
            .as_deref()
            .or(item.quote_sent_at.as_deref())
            .or(item.latest_customer_message_at.as_deref()),
    }
}

pub(crate) fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub(crate) fn gmail_thread_url(item: &ReportItem) -> Option<String> {
    let account = item.account.as_deref()?.trim();
    if account.is_empty() || item.thread_id.is_empty() {
        return None;
    }

    Some(format!(
        "https://mail.google.com/mail?authuser={}#all/{}",
        urlencoding::encode(account),
        urlencoding::encode(&item.thread_id)
    ))
}

fn parse_datetime(value: &str) -> Option<DateTime<FixedOffset>> {
    let ist = FixedOffset::east_opt(5 * 3600 + 30 * 60)?;
    if let Ok(datetime) = DateTime::parse_from_rfc2822(value) {
        return Some(datetime.with_timezone(&ist));
    }
    if let Ok(datetime) = DateTime::parse_from_rfc3339(value) {
        return Some(datetime.with_timezone(&ist));
    }
    if let Ok(datetime) = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S") {
        return ist.from_local_datetime(&datetime).single();
    }
    None
}

pub(crate) fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out = value.chars().take(max_chars).collect::<String>();
    out.push_str("...");
    out
}

#[derive(Clone, Copy)]
pub(crate) enum DateField {
    Received,
    QuoteSent,
}
