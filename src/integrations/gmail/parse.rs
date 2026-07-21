use crate::domain::types::{AttachmentMeta, GmailMessage, GmailThread};
use base64::Engine;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};

pub(super) fn parse_thread(
    thread: Value,
    control_label_names_by_id: &HashMap<String, String>,
) -> GmailThread {
    // Gmail's thread response contains thread metadata plus a list of full
    // message objects. This parser keeps the raw Gmail identifiers, parses each
    // message independently, and derives workflow control labels from message
    // label IDs instead of trusting arbitrary label names from the API response.
    let thread_id = thread
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let history_id = thread
        .get("historyId")
        .and_then(Value::as_str)
        .map(str::to_string);

    let empty_messages = Vec::new();
    let messages = thread
        .get("messages")
        .and_then(Value::as_array)
        .unwrap_or(&empty_messages)
        .iter()
        .map(parse_message)
        .collect::<Vec<_>>();
    let control_label_names = messages
        .iter()
        .flat_map(|message| &message.label_ids)
        .filter_map(|label_id| control_label_names_by_id.get(label_id))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    GmailThread {
        thread_id,
        history_id,
        control_label_names,
        messages,
    }
}

fn parse_message(message: &Value) -> GmailMessage {
    // A Gmail message stores headers and MIME body data under `payload`, while
    // IDs, labels, snippet, and internalDate live on the message object itself.
    // Missing fields are kept as None/defaults so one partial message does not
    // stop the whole export.
    let payload = message.get("payload").unwrap_or(&Value::Null);
    let headers = headers(payload);
    let mut bodies = Vec::new();
    let mut attachments = Vec::new();
    collect_parts(payload, &mut bodies, &mut attachments);

    GmailMessage {
        id: string_field(message, "id"),
        thread_id: string_field(message, "threadId"),
        label_ids: string_array_field(message, "labelIds"),
        internal_date_ms: opt_string_field(message, "internalDate"),
        from: header(&headers, "From"),
        to: header(&headers, "To"),
        cc: header(&headers, "Cc"),
        date: header(&headers, "Date"),
        subject: header(&headers, "Subject"),
        message_id_header: header(&headers, "Message-ID")
            .or_else(|| header(&headers, "Message-Id")),
        snippet: opt_string_field(message, "snippet"),
        text_body: bodies.join("\n\n").trim().to_string(),
        attachments,
    }
}

fn string_array_field(value: &Value, field: &str) -> Vec<String> {
    let empty_values = Vec::new();
    value
        .get(field)
        .and_then(Value::as_array)
        .unwrap_or(&empty_values)
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn headers(payload: &Value) -> HashMap<String, String> {
    // Gmail headers are returned as [{"name": "...", "value": "..."}]. Convert
    // them to a map and do case-insensitive lookup later because email header
    // names are case-insensitive.
    let empty_headers = Vec::new();
    payload
        .get("headers")
        .and_then(Value::as_array)
        .unwrap_or(&empty_headers)
        .iter()
        .filter_map(|h| {
            Some((
                h.get("name")?.as_str()?.to_string(),
                h.get("value")?.as_str()?.to_string(),
            ))
        })
        .collect()
}

fn header(headers: &HashMap<String, String>, name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.to_string())
}

fn collect_parts(payload: &Value, bodies: &mut Vec<String>, attachments: &mut Vec<AttachmentMeta>) {
    // MIME payloads are trees: multipart containers have child `parts`, while
    // leaves may carry body data or attachment metadata. Walk recursively so
    // nested alternatives/attachments are not lost.
    let mime_type = payload.get("mimeType").and_then(Value::as_str);
    let filename = payload
        .get("filename")
        .and_then(Value::as_str)
        .unwrap_or("");
    let body = payload.get("body").unwrap_or(&Value::Null);

    if !filename.is_empty() {
        attachments.push(AttachmentMeta {
            filename: filename.to_string(),
            mime_type: mime_type.map(str::to_string),
            attachment_id: body
                .get("attachmentId")
                .and_then(Value::as_str)
                .map(str::to_string),
            size: body.get("size").and_then(Value::as_i64),
        });
    }

    if let Some(data) = body.get("data").and_then(Value::as_str)
        && matches!(mime_type, Some("text/plain") | Some("text/html"))
        && let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(data)
    {
        // Gmail encodes inline text bodies as URL-safe base64. Keep plain text
        // as-is; strip simple HTML tags so the classifier sees readable text.
        let text = String::from_utf8_lossy(&bytes).to_string();
        if mime_type == Some("text/html") {
            bodies.push(strip_html(&text));
        } else {
            bodies.push(text);
        }
    }

    if let Some(parts) = payload.get("parts").and_then(Value::as_array) {
        for part in parts {
            collect_parts(part, bodies, attachments);
        }
    }
}

fn strip_html(html: &str) -> String {
    // This is intentionally a small fallback, not a full HTML parser. It is good
    // enough for classifier context where rough readable text beats raw markup.
    let mut out = String::new();
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn string_field(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn opt_string_field(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_thread_keeps_only_known_control_labels() {
        let mut labels = HashMap::new();
        labels.insert("Label_1".to_string(), "Ignore".to_string());
        labels.insert("Label_2".to_string(), "Order Closed".to_string());

        let thread = parse_thread(
            json!({
                "id": "thread-1",
                "messages": [
                    {
                        "id": "msg-1",
                        "threadId": "thread-1",
                        "labelIds": ["Label_1", "CATEGORY_PERSONAL"],
                        "payload": {"headers": []}
                    },
                    {
                        "id": "msg-2",
                        "threadId": "thread-1",
                        "labelIds": ["Label_2", "Label_1"],
                        "payload": {"headers": []}
                    }
                ]
            }),
            &labels,
        );

        assert_eq!(
            thread.control_label_names,
            vec!["Ignore".to_string(), "Order Closed".to_string()]
        );
    }
}
