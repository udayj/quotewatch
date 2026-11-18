use super::*;

#[test]
fn refresh_reads_skip_expired_rows_and_preserve_history() {
    let db = test_database();
    let before = stored_rows(&db);

    assert_refresh_ids(
        &db,
        date(15),
        &["expires-on-date", "future-followup", "legacy-no-expiry"],
    );

    assert_eq!(stored_rows(&db), before);
    assert_eq!(before.len(), 6);
}

#[test]
fn refresh_uses_processing_date_for_backfills_and_future_runs() {
    let db = test_database();
    // A newer committed run must not determine eligibility for a backfill.
    db.exec(
        "insert into runs(account, as_of_date, started_at) values(?1, ?2, ?3)",
        params!["sales@example.com", "2100-01-01", "2100-01-01T00:00:00Z"],
    )
    .expect("insert newer run");

    assert_refresh_ids(
        &db,
        date(14),
        &[
            "expired-followup",
            "expired-quote",
            "expires-on-date",
            "future-followup",
            "legacy-no-expiry",
        ],
    );
    assert_refresh_ids(&db, date(16), &["future-followup", "legacy-no-expiry"]);
    assert_refresh_ids(&db, date(17), &["legacy-no-expiry"]);
}

#[test]
fn expired_snapshots_are_excluded_before_json_decoding() {
    let db = test_database();
    db.exec(
        "update threads set raw_thread_json = 'invalid JSON' where thread_id = 'expired-quote'",
        (),
    )
    .expect("corrupt expired snapshot");

    assert_refresh_ids(
        &db,
        date(15),
        &["expires-on-date", "future-followup", "legacy-no-expiry"],
    );
}

fn test_database() -> Database {
    let db = Database::open_local_for_test(Path::new(":memory:")).expect("open test database");
    for (thread_id, state, expiry) in [
        (
            "expired-quote",
            "QUOTATION_REPLY_PENDING",
            Some("2000-06-14"),
        ),
        ("expired-followup", "FOLLOWUP_PENDING", Some("2000-06-14")),
        (
            "expires-on-date",
            "QUOTATION_REPLY_PENDING",
            Some("2000-06-15"),
        ),
        ("future-followup", "FOLLOWUP_PENDING", Some("2000-06-16")),
        ("legacy-no-expiry", "FOLLOWUP_PENDING", None),
        ("ignored", "IGNORED", Some("2000-06-16")),
    ] {
        let thread = GmailThread {
            thread_id: thread_id.to_string(),
            history_id: None,
            control_label_names: Vec::new(),
            messages: Vec::new(),
        };
        db.exec(
            "insert into threads(thread_id, state, expires_on, raw_thread_json, digest_json, updated_at)
             values(?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                thread_id,
                state,
                expiry,
                serde_json::to_string(&thread).expect("serialize snapshot"),
                "{}",
                "2000-06-01T00:00:00Z"
            ],
        )
        .expect("insert test thread");
    }
    db
}

fn assert_refresh_ids(db: &Database, as_of_date: NaiveDate, expected: &[&str]) {
    let mut ids = db.active_thread_ids(as_of_date).expect("refresh IDs");
    ids.sort();
    assert_eq!(ids, expected);

    let mut snapshot_ids = db
        .active_stored_threads(as_of_date)
        .expect("refresh snapshots")
        .into_iter()
        .map(|thread| thread.thread_id)
        .collect::<Vec<_>>();
    snapshot_ids.sort();
    assert_eq!(snapshot_ids, expected);

    let mut digest_ids = db
        .active_digest_json_by_thread_id(as_of_date)
        .expect("refresh digests")
        .into_keys()
        .collect::<Vec<_>>();
    digest_ids.sort();
    assert_eq!(digest_ids, expected);
}

fn stored_rows(db: &Database) -> Vec<(String, String, Option<String>, String, String)> {
    db.query_collect(
        "select thread_id, state, expires_on, raw_thread_json, digest_json from threads order by thread_id",
        (),
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    )
    .expect("read stored history")
}

fn date(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2000, 6, day).expect("valid processing date")
}
