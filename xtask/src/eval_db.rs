use crate::Result;
use crate::types::{ExpectedTransition, ThreadActual, UsageTotals};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

// Reads the recorded thread count for one scenario date.
pub(crate) fn threads_seen_for_date(db_path: &Path, date: &str) -> Result<usize> {
    sqlite_scalar_usize(
        db_path,
        &format!("select coalesce(max(threads_seen), 0) from runs where as_of_date = '{date}'"),
    )
}

// Returns the current DB state distribution after a scenario date has been replayed.
pub(crate) fn state_counts(db_path: &Path) -> Result<BTreeMap<String, usize>> {
    sqlite_rows(
        db_path,
        "select state, count(*) from threads group by state order by state",
    )?
    .into_iter()
    .map(|row| Ok((row[0].clone(), row[1].parse::<usize>()?)))
    .collect::<Result<_>>()
}

// Returns selected final-thread fields used for regression checks.
pub(crate) fn final_threads(db_path: &Path) -> Result<BTreeMap<String, ThreadActual>> {
    sqlite_rows(
        db_path,
        "select thread_id, state, coalesce(quote_sent_at, ''), coalesce(latest_customer_message_at, ''), coalesce(latest_company_message_at, '') from threads order by thread_id",
    )?
    .into_iter()
    .map(|row| {
        Ok((
            row[0].clone(),
            ThreadActual {
                state: row[1].clone(),
                quote_sent_at: non_empty(row.get(2)),
                latest_customer_message_at: non_empty(row.get(3)),
                latest_company_message_at: non_empty(row.get(4)),
            },
        ))
    })
    .collect::<Result<_>>()
}

// Returns transition history with the run date attached so multi-day scenarios can compare workflow movement.
pub(crate) fn transitions(db_path: &Path) -> Result<Vec<ExpectedTransition>> {
    sqlite_rows(
        db_path,
        "select transitions.thread_id, coalesce(transitions.previous_state, ''), transitions.new_state, runs.as_of_date from transitions join runs on runs.id = transitions.run_id order by transitions.id",
    )?
    .into_iter()
    .map(|row| {
        Ok(ExpectedTransition {
            thread_id: row[0].clone(),
            previous_state: non_empty(row.get(1)),
            new_state: row[2].clone(),
            as_of_date: row[3].clone(),
        })
    })
    .collect()
}

// Reads the most recent model name so eval reports identify the provider/model under test.
pub(crate) fn latest_model(db_path: &Path) -> Result<String> {
    sqlite_scalar_string(
        db_path,
        "select coalesce(model, '') from runs order by id desc limit 1",
    )
}

// Sums token and cost columns across all replayed dates.
pub(crate) fn usage_totals(db_path: &Path) -> Result<UsageTotals> {
    let row = sqlite_rows(
        db_path,
        "select coalesce(sum(input_tokens), 0), coalesce(sum(cached_input_tokens), 0), coalesce(sum(cache_creation_input_tokens), 0), coalesce(sum(output_tokens), 0), coalesce(sum(cost_usd), 0), coalesce(sum(cost_inr), 0) from runs",
    )?
    .into_iter()
    .next()
    .ok_or("usage query returned no rows")?;
    Ok(UsageTotals {
        input_tokens: row[0].parse()?,
        cached_input_tokens: row[1].parse()?,
        cache_creation_input_tokens: row[2].parse()?,
        output_tokens: row[3].parse()?,
        cost_usd: row[4].parse()?,
        cost_inr: row[5].parse()?,
    })
}

// Runs a query that should return one numeric value.
fn sqlite_scalar_usize(db_path: &Path, sql: &str) -> Result<usize> {
    Ok(sqlite_scalar_string(db_path, sql)?.parse()?)
}

// Runs a query that should return one string value.
fn sqlite_scalar_string(db_path: &Path, sql: &str) -> Result<String> {
    sqlite_rows(db_path, sql)?
        .first()
        .and_then(|row| row.first())
        .cloned()
        .ok_or_else(|| format!("query returned no scalar: {sql}").into())
}

// Executes sqlite3 and returns rows split on tabs; xtask stays independent from app storage internals.
fn sqlite_rows(db_path: &Path, sql: &str) -> Result<Vec<Vec<String>>> {
    let output = Command::new("sqlite3")
        .args(["-batch", "-noheader", "-separator", "\t"])
        .arg(db_path)
        .arg(sql)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "sqlite3 query failed: {sql}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.split('\t').map(str::to_string).collect())
        .collect())
}

// Converts empty SQLite strings back into absent optional fields.
fn non_empty(value: Option<&String>) -> Option<String> {
    value.filter(|value| !value.is_empty()).cloned()
}
