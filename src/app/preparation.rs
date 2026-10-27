use super::cli::RunArgs;
use super::error::AppError;
use super::export::read_export;
use crate::domain::digest::build_digest;
use crate::domain::prepared_run::{
    PreparedClassification, PreparedRun, PreparedRunParts, PreparedThreadInput,
};
use crate::domain::types::{
    ClassificationBatch, GmailExport, GmailThread, LlmUsage, ThreadClassification, ThreadDigest,
    ThreadState,
};
use crate::error::Result;
use crate::integrations::classifier::Classifier;
use crate::integrations::gmail;
use crate::storage::database::Database;
use chrono::{NaiveDate, Utc};
use std::collections::HashSet;
use std::time::Instant;

pub(crate) fn prepare_run(
    db: &Database,
    classifier: Option<&Classifier>,
    args: &RunArgs,
    account: &str,
    date: NaiveDate,
) -> Result<PreparedRun> {
    if classifier.is_some() && args.batch_size == 0 {
        return Err(
            AppError::InvalidArgs("--batch-size must be greater than zero".to_string()).into(),
        );
    }

    let started_at = Utc::now().to_rfc3339();
    let fetch_started = Instant::now();
    eprintln!(
        "Preparing {date}: fetching daily threads (limit {})",
        args.limit
    );
    let export = match args.input_json.as_deref() {
        Some(path) => read_export(path)?,
        None => gmail::fetch_gmail_export(
            account,
            date,
            date,
            args.limit,
            &args.client_secret,
            &args.token,
            db.token_store(),
        )?,
    };
    eprintln!(
        "Fetched {} daily threads in {:.1}s",
        export.threads.len(),
        fetch_started.elapsed().as_secs_f64()
    );

    prepare_run_from_export(db, classifier, args, account, date, export, started_at)
}

fn prepare_run_from_export(
    db: &Database,
    classifier: Option<&Classifier>,
    args: &RunArgs,
    account: &str,
    date: NaiveDate,
    mut export: GmailExport,
    started_at: String,
) -> Result<PreparedRun> {
    append_active_threads(db, &mut export, args, account, date)?;

    let digests = export
        .threads
        .iter()
        .map(|thread| build_digest(thread, &export.account, date))
        .collect::<Vec<_>>();
    let inputs = prepared_thread_inputs(&export, &digests)?;

    let (ignored_digests, classifiable_digests): (Vec<_>, Vec<_>) =
        digests.iter().cloned().partition(|digest| {
            export
                .threads
                .iter()
                .find(|thread| thread.thread_id == digest.thread_id)
                .is_some_and(thread_has_control_label)
        });

    let mut classifications = Vec::new();
    if !ignored_digests.is_empty() {
        let ignored_batch = ClassificationBatch {
            classifications: ignored_digests
                .iter()
                .map(|digest| ignored_classification_for_digest(digest, &export.threads))
                .collect(),
        };
        classifications.extend(prepare_classifications(db, &ignored_batch)?);
    }

    let mut usage = LlmUsage::default();
    let notes = if let Some(classifier) = classifier {
        let batch_count = classifiable_digests.len().div_ceil(args.batch_size);
        eprintln!(
            "Classifying {} threads in {batch_count} batches; {} threads have control labels",
            classifiable_digests.len(),
            ignored_digests.len()
        );
        for (index, chunk) in classifiable_digests.chunks(args.batch_size).enumerate() {
            let batch_started = Instant::now();
            eprintln!("Starting classification batch {}/{batch_count}", index + 1);
            let result = classifier.classify(chunk)?;
            usage.add(result.usage);
            classifications.extend(prepare_classifications(db, &result.batch)?);
            eprintln!(
                "Completed classification batch {}/{batch_count} in {:.1}s",
                index + 1,
                batch_started.elapsed().as_secs_f64()
            );
        }
        None
    } else {
        Some("classification skipped".to_string())
    };

    PreparedRun::from_parts(PreparedRunParts {
        label: args.label.clone(),
        account: account.to_string(),
        as_of_date: date,
        model: classifier.map(|classifier| classifier.model().to_string()),
        started_at,
        finished_at: Utc::now().to_rfc3339(),
        threads_seen: export.threads.len(),
        inputs,
        classifications,
        usage,
        notes,
    })
    .map_err(Into::into)
}

fn prepared_thread_inputs(
    export: &GmailExport,
    digests: &[ThreadDigest],
) -> Result<Vec<PreparedThreadInput>> {
    digests
        .iter()
        .map(|digest| {
            let thread = export
                .threads
                .iter()
                .find(|thread| thread.thread_id == digest.thread_id)
                .ok_or_else(|| {
                    AppError::InvalidArgs(format!(
                        "digest {} has no matching Gmail thread",
                        digest.thread_id
                    ))
                })?;
            Ok(PreparedThreadInput {
                thread: thread.clone(),
                digest: digest.clone(),
            })
        })
        .collect()
}

fn prepare_classifications(
    db: &Database,
    batch: &ClassificationBatch,
) -> Result<Vec<PreparedClassification>> {
    batch
        .classifications
        .iter()
        .map(|classification| {
            Ok(PreparedClassification {
                previous_state: db.thread_state(&classification.thread_id)?,
                classification: classification.clone(),
            })
        })
        .collect()
}

fn ignored_classification_for_digest(
    digest: &ThreadDigest,
    threads: &[GmailThread],
) -> ThreadClassification {
    let label = threads
        .iter()
        .find(|thread| thread.thread_id == digest.thread_id)
        .and_then(|thread| thread.control_label_names.first())
        .cloned()
        .unwrap_or_else(|| "Ignore".to_string());

    ThreadClassification {
        thread_id: digest.thread_id.clone(),
        state: ThreadState::Ignored,
        requester_name: None,
        requester_email: None,
        requester_org: None,
        subject: Some(digest.subject.clone()),
        latest_customer_message_at: None,
        latest_company_message_at: None,
        quote_sent_at: None,
        latest_relevant_message_at: latest_digest_message_date(digest),
        extracted_requirement: None,
        short_reason: format!("Ignored because Gmail label '{label}' is present."),
        confidence: 1.0,
    }
}

fn latest_digest_message_date(digest: &ThreadDigest) -> Option<String> {
    digest
        .messages
        .iter()
        .rev()
        .find_map(|message| message.date.clone())
}

fn thread_has_control_label(thread: &GmailThread) -> bool {
    !thread.control_label_names.is_empty()
}

fn append_active_threads(
    db: &Database,
    export: &mut GmailExport,
    args: &RunArgs,
    account: &str,
    date: NaiveDate,
) -> Result<()> {
    let existing = export
        .threads
        .iter()
        .map(|thread| thread.thread_id.clone())
        .collect::<HashSet<_>>();
    let active_ids = db
        .active_thread_ids()?
        .into_iter()
        .filter(|thread_id| !existing.contains(thread_id))
        .collect::<Vec<_>>();
    let previous_digests = db.active_digest_json_by_thread_id()?;

    let refresh_started = Instant::now();
    eprintln!("Refreshing {} additional active threads", active_ids.len());
    let extra_threads = if args.input_json.is_some() {
        db.active_stored_threads()?
            .into_iter()
            .filter(|thread| active_ids.contains(&thread.thread_id))
            .collect::<Vec<_>>()
    } else {
        gmail::fetch_gmail_threads_by_id(
            account,
            &active_ids,
            &args.client_secret,
            &args.token,
            db.token_store(),
        )?
    };
    let refreshed_count = extra_threads.len();

    let changed_threads = extra_threads
        .into_iter()
        .filter(|thread| {
            active_thread_changed(
                thread,
                &export.account,
                date,
                previous_digests.get(&thread.thread_id),
            )
        })
        .collect::<Vec<_>>();

    eprintln!(
        "Refreshed {refreshed_count} active threads in {:.1}s; {} need classification",
        refresh_started.elapsed().as_secs_f64(),
        changed_threads.len()
    );
    merge_threads(export, changed_threads);
    Ok(())
}

fn active_thread_changed(
    thread: &GmailThread,
    account: &str,
    date: NaiveDate,
    previous_digest_json: Option<&String>,
) -> bool {
    if thread_has_control_label(thread) {
        return true;
    }

    let current = build_digest(thread, account, date);

    let Some(previous_digest_json) = previous_digest_json else {
        return true;
    };
    let Ok(previous) = serde_json::from_str::<ThreadDigest>(previous_digest_json) else {
        return true;
    };
    digest_message_ids(&current) != digest_message_ids(&previous)
}

fn digest_message_ids(digest: &ThreadDigest) -> Vec<&str> {
    digest
        .messages
        .iter()
        .map(|message| message.message_id.as_str())
        .collect()
}

fn merge_threads(export: &mut GmailExport, extra_threads: Vec<GmailThread>) {
    let mut seen = export
        .threads
        .iter()
        .map(|thread| thread.thread_id.clone())
        .collect::<HashSet<_>>();
    for thread in extra_threads {
        if seen.insert(thread.thread_id.clone()) {
            export.threads.push(thread);
        }
    }
    export.thread_count = export.threads.len();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::{
        DigestMessage, GmailMessage, MessageDirection, OmittedKeywordSummary,
    };
    use std::path::PathBuf;

    #[test]
    fn control_label_creates_ignored_classification() {
        let digest = test_digest("thread-1");
        let threads = vec![GmailThread {
            thread_id: "thread-1".to_string(),
            history_id: None,
            control_label_names: vec!["Order Received".to_string()],
            messages: Vec::new(),
        }];

        let classification = ignored_classification_for_digest(&digest, &threads);

        assert_eq!(classification.state, ThreadState::Ignored);
        assert_eq!(
            classification.short_reason,
            "Ignored because Gmail label 'Order Received' is present."
        );
    }

    #[test]
    fn control_label_marks_active_thread_changed() {
        let thread = GmailThread {
            thread_id: "thread-1".to_string(),
            history_id: None,
            control_label_names: vec!["Ignore".to_string()],
            messages: Vec::new(),
        };
        let previous_digest = "{}".to_string();

        assert!(active_thread_changed(
            &thread,
            "sales@example.com",
            date(),
            Some(&previous_digest)
        ));
    }

    #[test]
    fn preparing_skip_classify_run_builds_object_without_db_writes() {
        let db_path = test_db_path("prepare-no-writes");
        let db = Database::open_local_for_test(&db_path).expect("open test db");
        let args = test_run_args(true);
        let export = GmailExport {
            account: "sales@example.com".to_string(),
            query: "newer_than:1d".to_string(),
            date_window_start: date(),
            date_window_end: date(),
            generated_at: "2026-07-04T00:00:00Z".to_string(),
            thread_count: 1,
            threads: vec![test_thread("thread-1", Vec::new())],
        };

        assert_eq!(row_count(&db, "runs"), 0);
        assert_eq!(row_count(&db, "threads"), 0);
        assert_eq!(row_count(&db, "transitions"), 0);

        let prepared = prepare_run_from_export(
            &db,
            None,
            &args,
            "sales@example.com",
            date(),
            export,
            "2026-07-04T00:00:00Z".to_string(),
        )
        .expect("prepare run");

        assert_eq!(prepared.account, "sales@example.com");
        assert_eq!(prepared.threads_seen, 1);
        assert_eq!(prepared.inputs.len(), 1);
        assert!(prepared.classifications.is_empty());
        assert_eq!(prepared.notes.as_deref(), Some("classification skipped"));
        assert_eq!(prepared.usage.input_tokens, 0);
        assert_eq!(row_count(&db, "runs"), 0);
        assert_eq!(row_count(&db, "threads"), 0);
        assert_eq!(row_count(&db, "transitions"), 0);
    }

    fn row_count(db: &Database, table: &str) -> i64 {
        let sql = format!("select count(*) from {table}");
        db.query_collect(&sql, (), |row| row.get::<i64>(0))
            .expect("count rows")
            .into_iter()
            .next()
            .expect("count row")
    }

    fn test_run_args(skip_classify: bool) -> RunArgs {
        RunArgs {
            date: None,
            start: None,
            end: None,
            limit: 50,
            client_secret: PathBuf::from("client_secret.json"),
            token: PathBuf::from("token.json"),
            input_json: Some(PathBuf::from("input.json")),
            label: None,
            batch_size: 10,
            skip_classify,
        }
    }

    fn test_thread(thread_id: &str, control_label_names: Vec<String>) -> GmailThread {
        GmailThread {
            thread_id: thread_id.to_string(),
            history_id: None,
            control_label_names,
            messages: vec![GmailMessage {
                id: "msg-1".to_string(),
                thread_id: thread_id.to_string(),
                label_ids: Vec::new(),
                internal_date_ms: None,
                from: Some("customer@example.com".to_string()),
                to: Some("sales@example.com".to_string()),
                cc: None,
                date: Some("Sun, 21 Jun 2026 10:00:00 +0530".to_string()),
                subject: Some("RFQ".to_string()),
                message_id_header: None,
                snippet: None,
                text_body: "Need quote".to_string(),
                attachments: Vec::new(),
            }],
        }
    }

    fn test_digest(thread_id: &str) -> ThreadDigest {
        ThreadDigest {
            thread_id: thread_id.to_string(),
            subject: "RFQ".to_string(),
            as_of_date: date(),
            account: "sales@example.com".to_string(),
            messages_kept: 1,
            messages_omitted_before_digest: 0,
            omitted_keyword_summary: OmittedKeywordSummary::default(),
            messages: vec![DigestMessage {
                message_id: "msg-1".to_string(),
                date: Some("Sun, 21 Jun 2026 10:00:00 +0530".to_string()),
                internal_date_ms: None,
                direction: MessageDirection::ExternalToCompany,
                from: None,
                to: None,
                subject: Some("RFQ".to_string()),
                snippet: None,
                body_excerpt: String::new(),
                attachments: Vec::new(),
            }],
        }
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 6, 21).expect("valid date")
    }

    fn test_db_path(name: &str) -> PathBuf {
        let now = Utc::now()
            .timestamp_nanos_opt()
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        std::env::temp_dir().join(format!(
            "quotewatch-{name}-{}-{now}.sqlite",
            std::process::id()
        ))
    }
}
