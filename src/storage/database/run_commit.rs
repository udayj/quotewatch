use super::{Database, DatabaseError, DatabaseResult};
use crate::domain::prepared_run::{PreparedClassification, PreparedRun};
use crate::domain::types::ThreadState;
use chrono::{Days, NaiveDate, Utc};
use libsql::{Connection, Row, params, params::IntoParams};

use super::sql;

#[derive(Debug, Clone)]
struct RunCommit {
    run_key: String,
    label: Option<String>,
    account: String,
    as_of_date: String,
    model: Option<String>,
    started_at: String,
    finished_at: String,
    threads_seen: i64,
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_creation_input_tokens: i64,
    output_tokens: i64,
    cost_usd: f64,
    cost_inr: f64,
    notes: Option<String>,
    inputs: Vec<ThreadInputRow>,
    classifications: Vec<ClassificationRow>,
    transitions: Vec<TransitionRow>,
}

#[derive(Debug, Clone)]
struct ThreadInputRow {
    thread_id: String,
    subject: String,
    raw_thread_json: String,
    digest_json: String,
}

#[derive(Debug, Clone)]
struct ClassificationRow {
    thread_id: String,
    state: String,
    requester_name: Option<String>,
    requester_email: Option<String>,
    requester_org: Option<String>,
    subject: Option<String>,
    latest_customer_message_at: Option<String>,
    latest_company_message_at: Option<String>,
    quote_sent_at: Option<String>,
    latest_relevant_message_at: Option<String>,
    extracted_requirement: Option<String>,
    short_reason: String,
    confidence: f32,
    classification_json: String,
    expires_on: Option<String>,
}

#[derive(Debug, Clone)]
struct TransitionRow {
    thread_id: String,
    previous_state: Option<String>,
    new_state: String,
    reason: String,
}

impl RunCommit {
    // Converts the domain-level prepared run into DB-ready rows and serialized JSON.
    fn from_prepared(prepared: &PreparedRun) -> DatabaseResult<Self> {
        prepared.validate()?;

        let inputs = prepared
            .inputs
            .iter()
            .map(|input| {
                Ok(ThreadInputRow {
                    thread_id: input.digest.thread_id.clone(),
                    subject: input.digest.subject.clone(),
                    raw_thread_json: serde_json::to_string(&input.thread)?,
                    digest_json: serde_json::to_string(&input.digest)?,
                })
            })
            .collect::<DatabaseResult<Vec<_>>>()?;

        let classifications = prepared
            .classifications
            .iter()
            .map(|classification| classification_row(prepared.as_of_date, classification))
            .collect::<DatabaseResult<Vec<_>>>()?;

        let transitions = prepared
            .classifications
            .iter()
            .filter_map(transition_row)
            .collect::<Vec<_>>();

        Ok(Self {
            run_key: prepared.run_key.clone(),
            label: prepared.label.clone(),
            account: prepared.account.clone(),
            as_of_date: prepared.as_of_date.to_string(),
            model: prepared.model.clone(),
            started_at: prepared.started_at.clone(),
            finished_at: prepared.finished_at.clone(),
            threads_seen: prepared.threads_seen as i64,
            input_tokens: prepared.usage.input_tokens,
            cached_input_tokens: prepared.usage.cached_input_tokens,
            cache_creation_input_tokens: prepared.usage.cache_creation_input_tokens,
            output_tokens: prepared.usage.output_tokens,
            cost_usd: prepared.usage.cost_usd,
            cost_inr: prepared.usage.cost_inr,
            notes: prepared.notes.clone(),
            inputs,
            classifications,
            transitions,
        })
    }
}

// Builds the DB row for one classification, including serialized audit JSON and expiry.
fn classification_row(
    as_of_date: NaiveDate,
    prepared: &PreparedClassification,
) -> DatabaseResult<ClassificationRow> {
    let classification = &prepared.classification;
    Ok(ClassificationRow {
        thread_id: classification.thread_id.clone(),
        state: classification.state.as_db_value().to_string(),
        requester_name: classification.requester_name.clone(),
        requester_email: classification.requester_email.clone(),
        requester_org: classification.requester_org.clone(),
        subject: classification.subject.clone(),
        latest_customer_message_at: classification.latest_customer_message_at.clone(),
        latest_company_message_at: classification.latest_company_message_at.clone(),
        quote_sent_at: classification.quote_sent_at.clone(),
        latest_relevant_message_at: classification.latest_relevant_message_at.clone(),
        extracted_requirement: classification.extracted_requirement.clone(),
        short_reason: classification.short_reason.clone(),
        confidence: classification.confidence,
        classification_json: serde_json::to_string(classification)?,
        expires_on: expiry_for_date(as_of_date, &classification.state)?,
    })
}

// Derives an append-only transition row when the prepared state differs from stored state.
fn transition_row(prepared: &PreparedClassification) -> Option<TransitionRow> {
    let new_state = prepared.classification.state.as_db_value();
    if prepared
        .previous_state
        .as_ref()
        .map(ThreadState::as_db_value)
        == Some(new_state)
    {
        return None;
    }

    Some(TransitionRow {
        thread_id: prepared.classification.thread_id.clone(),
        previous_state: prepared
            .previous_state
            .as_ref()
            .map(ThreadState::as_db_value)
            .map(str::to_string),
        new_state: new_state.to_string(),
        reason: prepared.classification.short_reason.clone(),
    })
}

// Computes the state-specific report expiry date before the DB transaction starts.
fn expiry_for_date(as_of_date: NaiveDate, state: &ThreadState) -> DatabaseResult<Option<String>> {
    let Some(days) = state.expiry_days() else {
        return Ok(None);
    };
    let expires_on = as_of_date
        .checked_add_days(Days::new(days))
        .ok_or_else(|| DatabaseError::State("date overflow while computing expiry".to_string()))?;
    Ok(Some(expires_on.to_string()))
}

// Runs the SQL statements that publish a prepared run on an already-open transaction.
async fn write_run_commit_statements(conn: &Connection, commit: &RunCommit) -> DatabaseResult<()> {
    if query_optional_on_connection(
        conn,
        sql::SELECT_RUN_BY_KEY,
        params![commit.run_key.clone()],
        |row| row.get::<i64>(0),
    )
    .await?
    .is_some()
    {
        return Ok(());
    }

    conn.execute(
        sql::INSERT_COMMITTED_RUN,
        params![
            commit.run_key.clone(),
            commit.label.clone(),
            commit.account.clone(),
            commit.as_of_date.clone(),
            commit.started_at.clone(),
            commit.finished_at.clone(),
            commit.threads_seen,
            commit.model.clone(),
            commit.input_tokens,
            commit.cached_input_tokens,
            commit.cache_creation_input_tokens,
            commit.output_tokens,
            commit.cost_usd,
            commit.cost_inr,
            commit.notes.clone(),
        ],
    )
    .await?;
    let run_id = conn.last_insert_rowid();
    let updated_at = Utc::now().to_rfc3339();

    for input in &commit.inputs {
        conn.execute(
            sql::UPSERT_THREAD_INPUT,
            params![
                input.thread_id.clone(),
                input.subject.clone(),
                input.raw_thread_json.clone(),
                input.digest_json.clone(),
                run_id,
                updated_at.clone(),
            ],
        )
        .await?;
    }

    for classification in &commit.classifications {
        conn.execute(
            sql::UPDATE_THREAD_CLASSIFICATION,
            params![
                classification.state.clone(),
                classification.requester_name.clone(),
                classification.requester_email.clone(),
                classification.requester_org.clone(),
                classification.subject.clone(),
                classification.latest_customer_message_at.clone(),
                classification.latest_company_message_at.clone(),
                classification.quote_sent_at.clone(),
                classification.latest_relevant_message_at.clone(),
                classification.extracted_requirement.clone(),
                classification.short_reason.clone(),
                classification.confidence as f64,
                classification.classification_json.clone(),
                classification.expires_on.clone(),
                updated_at.clone(),
                classification.thread_id.clone(),
            ],
        )
        .await?;
    }

    for transition in &commit.transitions {
        conn.execute(
            sql::INSERT_TRANSITION,
            params![
                transition.thread_id.clone(),
                run_id,
                transition.previous_state.clone(),
                transition.new_state.clone(),
                transition.reason.clone(),
                updated_at.clone(),
            ],
        )
        .await?;
    }

    Ok(())
}

// Reads one optional row from the transaction connection without using outer retry wrappers.
async fn query_optional_on_connection<T, P, F>(
    conn: &Connection,
    sql: &str,
    params: P,
    map: F,
) -> DatabaseResult<Option<T>>
where
    P: IntoParams,
    F: Fn(Row) -> libsql::Result<T>,
{
    let params = params.into_params()?;
    let mut rows = conn.query(sql, params).await?;
    let row = rows.next().await?;
    row.map(map).transpose().map_err(Into::into)
}

impl Database {
    // Persists one prepared run by converting it into a DB-ready transaction payload.
    pub fn commit_prepared_run(&self, prepared: &PreparedRun) -> DatabaseResult<()> {
        let commit = RunCommit::from_prepared(prepared)?;
        self.write_run_commit_with_retries(&commit)
    }

    // Retries the whole commit transaction so ambiguous network failures can resolve by run key.
    fn write_run_commit_with_retries(&self, commit: &RunCommit) -> DatabaseResult<()> {
        super::retry::retry_database_operation("commit prepared run", || {
            self.rt
                .block_on(async { self.write_run_commit_once(commit).await })
        })
    }

    // Owns transaction commit/rollback behavior for one prepared-run write attempt.
    async fn write_run_commit_once(&self, commit: &RunCommit) -> DatabaseResult<()> {
        let tx = self.conn.transaction().await?;
        let write_result = write_run_commit_statements(&tx, commit).await;
        match write_result {
            Ok(()) => {
                tx.commit().await?;
                Ok(())
            }
            Err(error) => {
                if let Err(rollback_error) = tx.rollback().await {
                    eprintln!(
                        "database rollback failed after commit prepared run error: {rollback_error}"
                    );
                }
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::prepared_run::{
        PreparedClassification, PreparedRunParts, PreparedThreadInput,
    };
    use crate::domain::types::{
        DigestMessage, GmailMessage, GmailThread, LlmUsage, MessageDirection,
        OmittedKeywordSummary, ThreadClassification, ThreadDigest,
    };
    use std::path::PathBuf;

    #[test]
    fn commit_prepared_run_is_idempotent() {
        let db = test_database("commit-idempotent");
        let prepared = test_prepared_run("thread-1");

        db.commit_prepared_run(&prepared)
            .expect("commit prepared run");
        db.commit_prepared_run(&prepared)
            .expect("repeat prepared run");

        assert_eq!(row_count(&db, "runs"), 1);
        assert_eq!(row_count(&db, "threads"), 1);
        assert_eq!(row_count(&db, "transitions"), 1);

        let usage = db
            .latest_run_cost_summary()
            .expect("latest usage")
            .expect("usage row");
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.cached_input_tokens, 25);
        assert_eq!(usage.cache_creation_input_tokens, 10);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cost_inr, 12.5);

        let report_items = db.report_items().expect("report items");
        assert_eq!(report_items.len(), 1);
        assert_eq!(report_items[0].thread_id, "thread-1");
        assert_eq!(
            report_items[0].account.as_deref(),
            Some("sales@example.com")
        );
        assert_eq!(report_items[0].state, ThreadState::QuotationReplyPending);
    }

    #[test]
    fn failed_commit_rolls_back_all_rows() {
        let db = test_database("commit-rollback");
        let prepared = test_prepared_run("thread-1");
        db.exec(
            "
            create trigger fail_thread_insert
            before insert on threads
            begin
              select raise(abort, 'forced thread insert failure');
            end
            ",
            (),
        )
        .expect("create failure trigger");

        let error = db
            .commit_prepared_run(&prepared)
            .expect_err("commit failure");

        assert!(error.to_string().contains("forced thread insert failure"));
        assert_eq!(row_count(&db, "runs"), 0);
        assert_eq!(row_count(&db, "threads"), 0);
        assert_eq!(row_count(&db, "transitions"), 0);
    }

    fn test_database(name: &str) -> Database {
        Database::open_local_for_test(&test_db_path(name)).expect("open test database")
    }

    fn row_count(db: &Database, table: &str) -> i64 {
        let sql = format!("select count(*) from {table}");
        db.query_collect(&sql, (), |row| row.get::<i64>(0))
            .expect("count rows")
            .into_iter()
            .next()
            .expect("count row")
    }

    fn test_prepared_run(thread_id: &str) -> PreparedRun {
        PreparedRun::from_parts(PreparedRunParts {
            label: None,
            account: "sales@example.com".to_string(),
            as_of_date: date(),
            model: Some("model-a".to_string()),
            started_at: "2026-07-04T00:00:00+00:00".to_string(),
            finished_at: "2026-07-04T00:00:03+00:00".to_string(),
            threads_seen: 1,
            inputs: vec![PreparedThreadInput {
                thread: test_thread(thread_id),
                digest: test_digest(thread_id),
            }],
            classifications: vec![PreparedClassification {
                previous_state: None,
                classification: ThreadClassification {
                    thread_id: thread_id.to_string(),
                    state: ThreadState::QuotationReplyPending,
                    requester_name: Some("A Customer".to_string()),
                    requester_email: Some("customer@example.com".to_string()),
                    requester_org: None,
                    subject: Some("RFQ".to_string()),
                    latest_customer_message_at: Some("2026-07-04T09:00:00+05:30".to_string()),
                    latest_company_message_at: None,
                    quote_sent_at: None,
                    latest_relevant_message_at: Some("2026-07-04T09:00:00+05:30".to_string()),
                    extracted_requirement: Some("Needs a quote".to_string()),
                    short_reason: "Customer is waiting for a quotation reply.".to_string(),
                    confidence: 0.92,
                },
            }],
            usage: LlmUsage {
                input_tokens: 100,
                cached_input_tokens: 25,
                cache_creation_input_tokens: 10,
                output_tokens: 50,
                cost_usd: 0.15,
                cost_inr: 12.5,
            },
            notes: None,
        })
        .expect("valid prepared run")
    }

    fn test_thread(thread_id: &str) -> GmailThread {
        GmailThread {
            thread_id: thread_id.to_string(),
            history_id: None,
            control_label_names: Vec::new(),
            messages: vec![GmailMessage {
                id: "msg-1".to_string(),
                thread_id: thread_id.to_string(),
                label_ids: Vec::new(),
                internal_date_ms: None,
                from: Some("customer@example.com".to_string()),
                to: Some("sales@example.com".to_string()),
                cc: None,
                date: Some("Sat, 04 Jul 2026 09:00:00 +0530".to_string()),
                subject: Some("RFQ".to_string()),
                message_id_header: None,
                snippet: Some("Need quote".to_string()),
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
                date: Some("Sat, 04 Jul 2026 09:00:00 +0530".to_string()),
                internal_date_ms: None,
                direction: MessageDirection::ExternalToCompany,
                from: Some("customer@example.com".to_string()),
                to: Some("sales@example.com".to_string()),
                subject: Some("RFQ".to_string()),
                snippet: Some("Need quote".to_string()),
                body_excerpt: "Need quote".to_string(),
                attachments: Vec::new(),
            }],
        }
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 7, 4).expect("valid date")
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
