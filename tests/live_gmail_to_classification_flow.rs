use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
#[ignore = "hits Gmail and paid classifier APIs"]
fn live_gmail_to_classification_flow() {
    if env::var("RUN_LIVE_GMAIL_E2E").as_deref() != Ok("1") {
        eprintln!("skipping live test; set RUN_LIVE_GMAIL_E2E=1 to run it");
        return;
    }

    let date = env::var("RUN_LIVE_GMAIL_E2E_DATE")
        .expect("set RUN_LIVE_GMAIL_E2E_DATE to a YYYY-MM-DD date with test mail");
    let limit = env::var("RUN_LIVE_GMAIL_E2E_LIMIT").unwrap_or_else(|_| "100".to_string());
    let db_path = temp_db_path();
    let output = Command::new(env!("CARGO_BIN_EXE_quotewatch"))
        .env("DATABASE_URL", "")
        .env("REPORT_EMAIL_TO", "")
        .args([
            "--db",
            db_path.to_str().expect("temp db path should be UTF-8"),
            "run",
            "--date",
            &date,
            "--limit",
            &limit,
        ])
        .output()
        .expect("run QuoteWatch live e2e command");

    if !output.status.success() {
        panic!(
            "live e2e command failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let escaped_date = date.replace('\'', "''");
    let threads_seen = sqlite_scalar_usize(
        &db_path,
        &format!(
            "select coalesce(max(threads_seen), 0) from runs where as_of_date = '{escaped_date}'"
        ),
    );
    assert!(
        threads_seen > 0,
        "choose a live-test date containing at least one Gmail thread"
    );
    assert_eq!(
        sqlite_scalar_usize(&db_path, "select count(*) from threads"),
        threads_seen
    );
    assert_eq!(
        sqlite_scalar_usize(
            &db_path,
            "select count(*) from threads where raw_thread_json is not null and raw_thread_json != '{}'"
        ),
        threads_seen
    );
    assert_eq!(
        sqlite_scalar_usize(
            &db_path,
            "select count(*) from threads where digest_json is not null"
        ),
        threads_seen
    );
    assert_eq!(
        sqlite_scalar_usize(
            &db_path,
            "select count(*) from threads where classification_json is not null"
        ),
        threads_seen
    );
    assert!(
        sqlite_scalar_usize(
            &db_path,
            "select coalesce(sum(input_tokens + output_tokens), 0) from runs"
        ) > 0
    );

    let counts = state_counts(&db_path);
    assert_eq!(
        counts.values().sum::<usize>(),
        threads_seen,
        "state counts should account for every fetched thread: {counts:?}"
    );
    assert!(
        counts.keys().all(|state| matches!(
            state.as_str(),
            "FOLLOWUP_PENDING" | "IGNORED" | "QUOTATION_REPLY_PENDING"
        )),
        "live run produced an unknown thread state: {counts:?}"
    );

    let _ = fs::remove_file(db_path);
}

fn temp_db_path() -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before Unix epoch")
        .as_nanos();
    env::temp_dir().join(format!(
        "quotewatch-live-e2e-{}-{suffix}.sqlite",
        std::process::id()
    ))
}

fn state_counts(db_path: &Path) -> BTreeMap<String, usize> {
    sqlite_rows(
        db_path,
        "select state, count(*) from threads group by state order by state",
    )
    .into_iter()
    .map(|row| {
        let count = row[1]
            .parse::<usize>()
            .expect("state count should be numeric");
        (row[0].clone(), count)
    })
    .collect()
}

fn sqlite_scalar_usize(db_path: &Path, sql: &str) -> usize {
    sqlite_rows(db_path, sql)
        .first()
        .and_then(|row| row.first())
        .expect("query should return a scalar")
        .parse()
        .expect("query scalar should be numeric")
}

fn sqlite_rows(db_path: &Path, sql: &str) -> Vec<Vec<String>> {
    let output = Command::new("sqlite3")
        .args(["-batch", "-noheader", "-separator", "\t"])
        .arg(db_path)
        .arg(sql)
        .output()
        .expect("run sqlite3");

    if !output.status.success() {
        panic!(
            "sqlite3 query failed: {sql}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.split('\t').map(str::to_string).collect())
        .collect()
}
