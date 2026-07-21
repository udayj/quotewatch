use super::format::{
    DateField, display_name, format_datetime, format_duration, gmail_thread_url, html_escape,
    item_date, split_report_items, truncate,
};
use crate::domain::types::{ReportItem, RunCostSummary};

pub fn render_concise_report_html(items: &[ReportItem], cost: Option<&RunCostSummary>) -> String {
    let (pending, followups) = split_report_items(items);
    let amount = cost.map(|cost| cost.cost_inr).unwrap_or_default();
    let elapsed = cost
        .and_then(|cost| cost.elapsed_seconds)
        .map(format_duration)
        .unwrap_or_else(|| "Unknown".to_string());

    let mut out = String::new();
    out.push_str("<!doctype html><html><body style=\"margin:0;padding:0;background:#f6f7f9;color:#111827;font-family:Arial,Helvetica,sans-serif;\">");
    out.push_str("<div style=\"max-width:760px;margin:0 auto;padding:24px 16px;\">");
    out.push_str("<div style=\"background:#ffffff;border:1px solid #e5e7eb;border-radius:10px;overflow:hidden;\">");
    out.push_str("<div style=\"padding:22px 24px;border-bottom:1px solid #e5e7eb;\">");
    out.push_str("<h1 style=\"margin:0 0 8px;font-size:24px;line-height:1.25;color:#111827;\">Quotation Follow-up Report</h1>");
    out.push_str(&format!(
        "<div style=\"font-size:13px;color:#4b5563;\">Cost: <strong>{}</strong> &nbsp; Runtime: <strong>{}</strong></div>",
        html_escape(&format!("Rs. {:.2}", amount)),
        html_escape(&elapsed)
    ));
    out.push_str("</div>");

    out.push_str("<div style=\"padding:18px 24px;border-bottom:1px solid #e5e7eb;\">");
    out.push_str("<div style=\"font-size:12px;font-weight:700;letter-spacing:0.04em;text-transform:uppercase;color:#6b7280;margin-bottom:10px;\">Summary</div>");
    out.push_str("<table role=\"presentation\" cellspacing=\"0\" cellpadding=\"0\" style=\"width:100%;border-collapse:collapse;\"><tr>");
    out.push_str(&summary_card(
        "Quotation Pending",
        pending.len(),
        "#b42318",
        "#fff4f2",
        "#ffd6d1",
    ));
    out.push_str("<td style=\"width:12px;\"></td>");
    out.push_str(&summary_card(
        "Follow-ups Pending",
        followups.len(),
        "#175cd3",
        "#eff8ff",
        "#b2ddff",
    ));
    out.push_str("</tr></table>");
    out.push_str("</div>");

    out.push_str(&format_html_section(
        "Quotations Pending",
        "Customers waiting for a quotation or commercial reply.",
        &pending,
        DateField::Received,
        "#b42318",
    ));
    out.push_str(&format_html_section(
        "Follow-ups Pending",
        "Quotations sent where no customer response is visible yet.",
        &followups,
        DateField::QuoteSent,
        "#175cd3",
    ));

    out.push_str("</div></div></body></html>");
    out
}

fn summary_card(label: &str, count: usize, color: &str, background: &str, border: &str) -> String {
    format!(
        "<td style=\"width:50%;padding:14px 16px;border:1px solid {border};border-radius:8px;background:{background};\"><div style=\"font-size:13px;color:#374151;font-weight:700;\">{}</div><div style=\"font-size:30px;line-height:1.15;font-weight:800;color:{color};margin-top:4px;\">{count}</div></td>",
        html_escape(label)
    )
}

fn format_html_section(
    title: &str,
    intro: &str,
    items: &[&ReportItem],
    date_field: DateField,
    accent: &str,
) -> String {
    let mut out = String::new();
    out.push_str("<div style=\"padding:22px 24px;border-bottom:1px solid #e5e7eb;\">");
    out.push_str(&format!(
        "<h2 style=\"margin:0;font-size:18px;line-height:1.3;color:{};\">{}</h2>",
        accent,
        html_escape(title)
    ));
    out.push_str(&format!(
        "<p style=\"margin:4px 0 16px;font-size:13px;line-height:1.45;color:#4b5563;\">{}</p>",
        html_escape(intro)
    ));

    if items.is_empty() {
        out.push_str("<div style=\"padding:12px 14px;border:1px solid #e5e7eb;border-radius:8px;color:#4b5563;background:#f9fafb;\">None.</div>");
    } else {
        for (index, item) in items.iter().enumerate() {
            out.push_str(&format_html_item(index + 1, item, date_field, accent));
        }
    }

    out.push_str("</div>");
    out
}

fn format_html_item(
    index: usize,
    item: &ReportItem,
    date_field: DateField,
    accent: &str,
) -> String {
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
    let display_name = html_escape(&display_name(item));
    let linked_name = gmail_thread_url(item)
        .map(|url| {
            format!(
                "<a href=\"{}\" style=\"color:#111827;text-decoration:underline;\">{}</a>",
                html_escape(&url),
                display_name
            )
        })
        .unwrap_or(display_name);

    format!(
        "<div style=\"padding:14px 0;border-top:1px solid #eef0f3;\"><div style=\"display:flex;align-items:flex-start;gap:10px;\"><span style=\"display:inline-block;min-width:26px;height:26px;line-height:26px;text-align:center;border-radius:50%;background:{};color:#ffffff;font-size:13px;font-weight:700;\">{}</span><div style=\"display:inline-block;vertical-align:top;\"><div style=\"font-size:15px;line-height:1.35;font-weight:800;color:#111827;\">{}</div><div style=\"font-size:13px;line-height:1.5;color:#374151;margin-top:4px;\"><strong>{}:</strong> {}</div><div style=\"font-size:13px;line-height:1.5;color:#374151;\"><strong>Subject:</strong> {}</div><div style=\"font-size:13px;line-height:1.5;color:#374151;\"><strong>Requirement:</strong> {}</div></div></div></div>",
        accent,
        index,
        linked_name,
        html_escape(date_label),
        html_escape(&date),
        html_escape(subject),
        html_escape(&requirement)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::ThreadState;

    #[test]
    fn report_links_to_the_thread_in_the_correct_gmail_account() {
        let html = render_concise_report_html(
            &[ReportItem {
                thread_id: "18f0123456789abc".to_string(),
                account: Some("sales+quotes@example.com".to_string()),
                state: ThreadState::QuotationReplyPending,
                requester_name: Some("Customer".to_string()),
                requester_email: None,
                requester_org: None,
                subject: Some("Quotation request".to_string()),
                latest_customer_message_at: None,
                latest_company_message_at: None,
                quote_sent_at: None,
                extracted_requirement: None,
                short_reason: None,
                confidence: 1.0,
                expires_on: None,
            }],
            None,
        );

        assert!(html.contains(
            "href=\"https://mail.google.com/mail?authuser=sales%2Bquotes%40example.com#all/18f0123456789abc\""
        ));
        assert!(html.contains(">Customer</a>"));
        assert!(!html.contains("Open in Gmail"));
    }
}
