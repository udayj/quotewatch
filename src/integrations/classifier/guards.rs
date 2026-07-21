use crate::domain::types::{
    ClassificationBatch, DigestMessage, MessageDirection, ThreadDigest, ThreadState,
};

pub(super) fn apply_deterministic_guards(
    digests: &[ThreadDigest],
    batch: &mut ClassificationBatch,
) {
    for classification in &mut batch.classifications {
        let Some(digest) = digests
            .iter()
            .find(|digest| digest.thread_id == classification.thread_id)
        else {
            continue;
        };

        // The LLM owns nuanced state selection. Rust only applies high-precision
        // workflow exclusions where the business rule is unambiguous.
        if is_supplier_procurement_thread(digest) {
            classification.state = ThreadState::Ignored;
            classification.short_reason =
                "The company is asking an external supplier/vendor for price, delivery, or technical data; outside customer quotation follow-up workflow"
                    .to_string();
            classification.confidence = classification.confidence.max(0.9);
        } else if has_customer_order_or_payment_after_company_quote(digest) {
            classification.state = ThreadState::Ignored;
            classification.short_reason =
                "Customer sent order/payment/invoice/execution message after the company's quote; quotation follow-up is no longer required"
                    .to_string();
            classification.confidence = classification.confidence.max(0.9);
        }
    }
}

fn is_supplier_procurement_thread(digest: &ThreadDigest) -> bool {
    // Supplier support is outside the customer quotation workflow: the company asks a
    // vendor/OEM for price, delivery, or technical data, and the vendor replies.
    let mut saw_supplier_request_from_company = false;

    for message in &digest.messages {
        let text = message_text(message);
        match message.direction {
            MessageDirection::CompanyToExternal if company_asks_supplier_for_support(&text) => {
                saw_supplier_request_from_company = true;
            }
            MessageDirection::ExternalToCompany
                if saw_supplier_request_from_company && external_supplier_support_reply(&text) =>
            {
                return true;
            }
            _ => {}
        }
    }

    false
}

fn company_asks_supplier_for_support(text: &str) -> bool {
    contains_any(
        text,
        &[
            "please provide",
            "please share",
            "kindly provide",
            "kindly share",
            "request you to",
        ],
    ) && contains_any(
        text,
        &[
            "best possible price",
            "best price",
            "delivery schedule",
            "delivery time",
            "datasheet",
            "data sheet",
            "tds",
        ],
    )
}

fn external_supplier_support_reply(text: &str) -> bool {
    contains_any(
        text,
        &[
            "please find attached",
            "tds",
            "datasheet",
            "data sheet",
            "delivery time",
            "delivery schedule",
            "below price",
            "below rate",
            "support with delivery",
        ],
    )
}

fn message_text(message: &DigestMessage) -> String {
    format!(
        "{}\n{}\n{}",
        message.subject.as_deref().unwrap_or(""),
        message.snippet.as_deref().unwrap_or(""),
        message.body_excerpt
    )
    .to_lowercase()
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn has_customer_order_or_payment_after_company_quote(digest: &ThreadDigest) -> bool {
    // Once the same thread has moved from quotation to order/payment/execution,
    // follow-up reminders are no longer useful for this workflow.
    let mut request_seen = false;
    let mut quote_after_request = false;

    for message in &digest.messages {
        let text = message_text(message);
        match message.direction {
            MessageDirection::ExternalToCompany if quote_after_request && order_like(&text) => {
                return true;
            }
            MessageDirection::ExternalToCompany if quote_request_like(&text) => {
                request_seen = true;
                quote_after_request = false;
            }
            MessageDirection::CompanyToExternal
                if request_seen && company_quote_reply_like(message, &text) =>
            {
                quote_after_request = true;
            }
            MessageDirection::ExternalToCompany
                if quote_after_request && quote_request_like(&text) =>
            {
                quote_after_request = false;
            }
            _ => {}
        }
    }

    false
}

fn quote_request_like(text: &str) -> bool {
    contains_any(
        text,
        &[
            "please quote",
            "quotation",
            "best rate",
            "best price",
            "request for quotation",
            "rfq",
            "request for rates",
            "please give rates",
            "please share your best quotation",
            "please provide your best price",
            "please submit your best",
            "requirement",
            "enquiry",
        ],
    )
}

fn company_quote_reply_like(message: &DigestMessage, text: &str) -> bool {
    contains_any(
        text,
        &[
            "please find the quotation",
            "quotation attached",
            "pfa quotation",
            "please find our rates",
            "our rates below",
            "rates below",
            "price below",
            "our offer",
            "validity",
        ],
    ) || message.attachments.iter().any(|attachment| {
        let attachment = attachment.to_lowercase();
        contains_any(&attachment, &["quote", "quotation", "offer"])
    })
}

fn order_like(text: &str) -> bool {
    contains_any(
        text,
        &[
            "purchase order",
            "p/order",
            " po ",
            "order confirmation",
            "final order",
            "final order qty",
            "please proceed",
            "proceed with",
            "dispatch",
            "payment receipt",
            "bank transaction",
            "invoice",
            "ledger",
            "e-waybill",
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::{DigestMessage, OmittedKeywordSummary};
    use chrono::NaiveDate;

    #[test]
    fn detects_supplier_procurement_support_thread() {
        let digest = digest_with_messages(vec![
            message(
                MessageDirection::ExternalToCompany,
                "Component Price",
                "Dear Sir, Below component price.",
            ),
            message(
                MessageDirection::CompanyToExternal,
                "Re: Component Price",
                "Please provide earliest delivery schedule, best possible prices and datasheet.",
            ),
            message(
                MessageDirection::ExternalToCompany,
                "RE: Component Price",
                "Please find attached TDS. Please support with delivery time.",
            ),
        ]);

        assert!(is_supplier_procurement_thread(&digest));
    }

    #[test]
    fn does_not_treat_customer_quote_thread_as_supplier_procurement() {
        let digest = digest_with_messages(vec![
            message(
                MessageDirection::ExternalToCompany,
                "RFQ Industrial Components",
                "Please quote your best rate for industrial components.",
            ),
            message(
                MessageDirection::CompanyToExternal,
                "Re: RFQ Industrial Components",
                "Please find attached quotation. Delivery ready stock subject to prior sale.",
            ),
        ]);

        assert!(!is_supplier_procurement_thread(&digest));
    }

    #[test]
    fn does_not_force_followup_for_normal_customer_quote_sequence() {
        let digest = digest_with_messages(vec![
            message(
                MessageDirection::ExternalToCompany,
                "RFQ Industrial Components",
                "Please quote your best rate for industrial components.",
            ),
            message(
                MessageDirection::CompanyToExternal,
                "Re: RFQ Industrial Components",
                "Dear Sir, please find our rates below. Validity 3 days.",
            ),
        ]);

        assert!(!has_customer_order_or_payment_after_company_quote(&digest));
    }

    #[test]
    fn does_not_force_pending_for_customer_revision_after_quote() {
        let digest = digest_with_messages(vec![
            message(
                MessageDirection::ExternalToCompany,
                "RFQ Industrial Components",
                "Please quote your best rate for industrial components.",
            ),
            message(
                MessageDirection::CompanyToExternal,
                "Re: RFQ Industrial Components",
                "Dear Sir, please find our rates below. Validity 3 days.",
            ),
            message(
                MessageDirection::ExternalToCompany,
                "Re: RFQ Industrial Components",
                "Your price is high. Please give best price for the components.",
            ),
        ]);

        assert!(!has_customer_order_or_payment_after_company_quote(&digest));
    }

    #[test]
    fn detects_customer_order_or_payment_after_quote() {
        let digest = digest_with_messages(vec![
            message(
                MessageDirection::ExternalToCompany,
                "RFQ Components",
                "Please quote your best rate for the components.",
            ),
            message(
                MessageDirection::CompanyToExternal,
                "Re: RFQ Components",
                "Dear Sir, please find our rates below. Validity 3 days.",
            ),
            message(
                MessageDirection::ExternalToCompany,
                "Re: RFQ Components",
                "Here the final order qty shared with you. Please generate invoice and share ledger.",
            ),
        ]);

        assert!(has_customer_order_or_payment_after_company_quote(&digest));
    }

    #[test]
    fn does_not_close_plain_request_without_company_quote() {
        let digest = digest_with_messages(vec![message(
            MessageDirection::ExternalToCompany,
            "Component offer for project",
            "External party mentions an offer but there is no company quote in sequence.",
        )]);

        assert!(!has_customer_order_or_payment_after_company_quote(&digest));
    }

    fn digest_with_messages(messages: Vec<DigestMessage>) -> ThreadDigest {
        ThreadDigest {
            thread_id: "t1".to_string(),
            subject: "subject".to_string(),
            as_of_date: NaiveDate::from_ymd_opt(2026, 6, 3).unwrap(),
            account: "sales@example.com".to_string(),
            messages_kept: messages.len(),
            messages_omitted_before_digest: 0,
            omitted_keyword_summary: OmittedKeywordSummary::default(),
            messages,
        }
    }

    fn message(direction: MessageDirection, subject: &str, body: &str) -> DigestMessage {
        DigestMessage {
            message_id: "m".to_string(),
            date: None,
            internal_date_ms: None,
            direction,
            from: None,
            to: None,
            subject: Some(subject.to_string()),
            snippet: Some(body.to_string()),
            body_excerpt: body.to_string(),
            attachments: Vec::new(),
        }
    }
}
