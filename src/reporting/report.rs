use super::format::{
    DateField, format_concise_cost, format_concise_item, format_detailed_cost,
    format_detailed_item, format_summary_counts, split_report_items,
};
use crate::domain::types::{ReportItem, RunCostSummary};

pub use super::html::render_concise_report_html;

pub fn render_report(
    items: &[ReportItem],
    format: ReportFormat,
    cost: Option<&RunCostSummary>,
) -> String {
    match format {
        ReportFormat::Concise => render_concise_report(items, cost),
        ReportFormat::Detailed => render_detailed_report(items, cost),
    }
}

pub fn render_concise_report(items: &[ReportItem], cost: Option<&RunCostSummary>) -> String {
    let (pending, followups) = split_report_items(items);

    let mut out = String::new();
    out.push_str("# Quotation Follow-up Report\n\n");
    out.push_str(&format_concise_cost(cost));
    out.push_str(&format_summary_counts(pending.len(), followups.len()));
    out.push_str("## Quotations Pending To Be Sent\n\n");
    if pending.is_empty() {
        out.push_str("None.\n\n");
    } else {
        for (index, item) in pending.iter().enumerate() {
            out.push_str(&format_concise_item(index + 1, item, DateField::Received));
        }
        out.push('\n');
    }

    out.push_str("## Follow-ups Pending\n\n");
    if followups.is_empty() {
        out.push_str("None.\n");
    } else {
        for (index, item) in followups.iter().enumerate() {
            out.push_str(&format_concise_item(index + 1, item, DateField::QuoteSent));
        }
    }

    out
}

fn render_detailed_report(items: &[ReportItem], cost: Option<&RunCostSummary>) -> String {
    let (pending, followups) = split_report_items(items);

    let mut out = String::new();
    out.push_str("# Quotation Follow-up Report\n\n");
    out.push_str(&format_detailed_cost(cost));
    out.push_str(&format_summary_counts(pending.len(), followups.len()));
    out.push_str("## Quotations Pending To Be Sent\n\n");
    if pending.is_empty() {
        out.push_str("None.\n\n");
    } else {
        for (index, item) in pending.iter().enumerate() {
            out.push_str(&format_detailed_item(index + 1, item, DateField::Received));
        }
        out.push('\n');
    }

    out.push_str("## Follow-ups Pending\n\n");
    if followups.is_empty() {
        out.push_str("None.\n");
    } else {
        for (index, item) in followups.iter().enumerate() {
            out.push_str(&format_detailed_item(index + 1, item, DateField::QuoteSent));
        }
    }

    out
}

#[derive(Debug, Clone, Copy)]
pub enum ReportFormat {
    Concise,
    Detailed,
}
