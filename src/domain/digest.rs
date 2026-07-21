use crate::domain::types::{
    DigestMessage, GmailMessage, GmailThread, MessageDirection, OmittedKeywordSummary, ThreadDigest,
};
use chrono::{Datelike, FixedOffset, NaiveDate, TimeZone};

const MAX_BODY_CHARS: usize = 2_000;
const MAX_MESSAGES_PER_DIGEST: usize = 14;

pub fn build_digest(thread: &GmailThread, account: &str, as_of_date: NaiveDate) -> ThreadDigest {
    // Backtests and daily runs use the same rule: classify only what was
    // visible by the run's as-of date, even if a stored thread snapshot contains
    // later messages.
    let mut eligible = thread
        .messages
        .iter()
        .filter(|message| message_is_on_or_before(message, as_of_date))
        .collect::<Vec<_>>();
    eligible.sort_by_key(|message| message.internal_date_ms.as_deref().unwrap_or(""));

    let omitted_count = eligible.len().saturating_sub(MAX_MESSAGES_PER_DIGEST);
    let kept_start = eligible.len().saturating_sub(MAX_MESSAGES_PER_DIGEST);
    let omitted_summary = summarize_omitted(&eligible[..kept_start], account);
    let messages = eligible[kept_start..]
        .iter()
        .map(|message| digest_message(message, account))
        .collect::<Vec<_>>();

    ThreadDigest {
        thread_id: thread.thread_id.clone(),
        subject: first_subject(thread).unwrap_or_default(),
        as_of_date,
        account: account.to_string(),
        messages_kept: messages.len(),
        messages_omitted_before_digest: omitted_count,
        omitted_keyword_summary: omitted_summary,
        messages,
    }
}

fn first_subject(thread: &GmailThread) -> Option<String> {
    thread
        .messages
        .iter()
        .find_map(|message| message.subject.clone())
}

fn digest_message(message: &GmailMessage, account: &str) -> DigestMessage {
    DigestMessage {
        message_id: message.id.clone(),
        date: message.date.clone(),
        internal_date_ms: message.internal_date_ms.clone(),
        direction: direction(message, account),
        from: message.from.clone(),
        to: message.to.clone(),
        subject: message.subject.clone(),
        snippet: message.snippet.clone(),
        body_excerpt: cap(clean_body(&message.text_body), MAX_BODY_CHARS),
        attachments: message
            .attachments
            .iter()
            .map(|attachment| attachment.filename.clone())
            .collect(),
    }
}

fn summarize_omitted(messages: &[&GmailMessage], account: &str) -> OmittedKeywordSummary {
    let mut summary = OmittedKeywordSummary {
        omitted_total: messages.len(),
        ..OmittedKeywordSummary::default()
    };
    for message in messages {
        let text = format!(
            "{}\n{}\n{}",
            message.subject.as_deref().unwrap_or(""),
            message.snippet.as_deref().unwrap_or(""),
            message.text_body
        )
        .to_lowercase();
        match direction(message, account) {
            MessageDirection::ExternalToCompany if contains_request_keyword(&text) => {
                summary.external_request_like += 1;
            }
            MessageDirection::CompanyToExternal if contains_quote_keyword(&text) => {
                summary.company_quote_like += 1;
            }
            _ => {}
        }
    }
    summary
}

fn direction(message: &GmailMessage, account: &str) -> MessageDirection {
    let from = message.from.as_deref().unwrap_or("").to_lowercase();
    let to = message.to.as_deref().unwrap_or("").to_lowercase();
    let cc = message.cc.as_deref().unwrap_or("").to_lowercase();
    let account = account.to_lowercase();
    let company_domain = account
        .split_once('@')
        .map(|(_, domain)| domain)
        .unwrap_or(account.as_str());

    if from.contains(&account) || from.contains(company_domain) {
        MessageDirection::CompanyToExternal
    } else if to.contains(&account)
        || cc.contains(&account)
        || to.contains(company_domain)
        // Some RFQs arrive via BCC or undisclosed recipients. If Gmail returned
        // the message for this mailbox and the sender is not the company, treat it as
        // external-to-company and let the classifier decide relevance.
        || !from.is_empty()
    {
        MessageDirection::ExternalToCompany
    } else {
        MessageDirection::InternalOrOther
    }
}

fn message_is_on_or_before(message: &GmailMessage, as_of_date: NaiveDate) -> bool {
    let Some(ms_text) = message.internal_date_ms.as_deref() else {
        return true;
    };
    let Ok(ms) = ms_text.parse::<i64>() else {
        return true;
    };
    let Some(datetime) = chrono::DateTime::from_timestamp_millis(ms) else {
        return true;
    };
    let Some(offset) = FixedOffset::east_opt(5 * 3600 + 30 * 60) else {
        return datetime.date_naive() <= as_of_date;
    };
    let Some(local_cutoff) = offset
        .with_ymd_and_hms(
            as_of_date.year(),
            as_of_date.month(),
            as_of_date.day(),
            23,
            59,
            59,
        )
        .single()
    else {
        return datetime.date_naive() <= as_of_date;
    };
    datetime <= local_cutoff
}

fn clean_body(body: &str) -> String {
    // Keep the current authored content and drop long quoted tails. Snippet and
    // attachment names remain available, so terse "PFA quotation" replies still
    // carry enough signal.
    let mut lines = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if is_quote_marker(trimmed) || is_long_disclaimer_marker(trimmed) {
            break;
        }
        if !trimmed.is_empty() {
            lines.push(trimmed);
        }
    }
    lines.join("\n")
}

fn is_quote_marker(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.starts_with("on ") && lower.ends_with("wrote:")
        || lower.starts_with("from:")
        || lower.starts_with("sent:")
        || lower.starts_with("---------- forwarded message")
        || lower.starts_with("________________________________")
}

fn is_long_disclaimer_marker(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.starts_with("disclaimer:")
        || lower.starts_with("the information contained in this")
        || lower.starts_with("this email may contain confidential")
        || lower.starts_with("caution:")
}

fn cap(text: String, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text;
    }
    let mut capped = text.chars().take(max_chars).collect::<String>();
    capped.push_str("\n[truncated]");
    capped
}

fn contains_request_keyword(text: &str) -> bool {
    [
        "quote",
        "quotation",
        "rate",
        "price",
        "offer",
        "rfq",
        "requirement",
        "please provide",
        "please share",
    ]
    .iter()
    .any(|keyword| text.contains(keyword))
}

fn contains_quote_keyword(text: &str) -> bool {
    [
        "quotation",
        "pfa",
        "prices below",
        "price below",
        "rates below",
        "please find the quotation",
        "our rates",
        "our offer",
    ]
    .iter()
    .any(|keyword| text.contains(keyword))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::AttachmentMeta;

    #[test]
    fn digest_caps_body_and_keeps_attachment_names() {
        let thread = GmailThread {
            thread_id: "t1".to_string(),
            history_id: None,
            control_label_names: Vec::new(),
            messages: vec![GmailMessage {
                id: "m1".to_string(),
                thread_id: "t1".to_string(),
                label_ids: Vec::new(),
                internal_date_ms: None,
                from: Some("Buyer <buyer@customer.test>".to_string()),
                to: Some("sales@company.test".to_string()),
                cc: None,
                date: Some("Wed, 3 Jun 2026 10:00:00 +0530".to_string()),
                subject: Some("RFQ".to_string()),
                message_id_header: None,
                snippet: Some("Please quote".to_string()),
                text_body: format!("{}\nFrom: old quoted mail", "a".repeat(2_500)),
                attachments: vec![AttachmentMeta {
                    filename: "req.xlsx".to_string(),
                    mime_type: None,
                    attachment_id: None,
                    size: None,
                }],
            }],
        };

        let digest = build_digest(
            &thread,
            "sales@company.test",
            NaiveDate::from_ymd_opt(2026, 6, 3).unwrap(),
        );

        assert_eq!(digest.messages.len(), 1);
        assert!(digest.messages[0].body_excerpt.contains("[truncated]"));
        assert_eq!(digest.messages[0].attachments, vec!["req.xlsx"]);
    }

    #[test]
    fn digest_treats_external_sender_with_hidden_recipient_as_customer_message() {
        let message = GmailMessage {
            id: "m1".to_string(),
            thread_id: "t1".to_string(),
            label_ids: Vec::new(),
            internal_date_ms: None,
            from: Some("Buyer <buyer@customer.test>".to_string()),
            to: Some("undisclosed-recipients:;".to_string()),
            cc: None,
            date: None,
            subject: Some("RFQ".to_string()),
            message_id_header: None,
            snippet: None,
            text_body: "Please quote the requested equipment".to_string(),
            attachments: Vec::new(),
        };

        assert!(matches!(
            direction(&message, "sales@company.test"),
            MessageDirection::ExternalToCompany
        ));
    }
}
