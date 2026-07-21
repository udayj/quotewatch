use super::{Database, DatabaseResult, sql};

pub(crate) fn migrate(database: &Database) -> DatabaseResult<()> {
    database.batch(sql::CREATE_SCHEMA)?;
    add_column_if_missing(database, "runs", "run_key", "text")?;
    add_column_if_missing(database, "threads", "expires_on", "text")?;
    add_column_if_missing(database, "runs", "input_tokens", "integer default 0")?;
    add_column_if_missing(database, "runs", "cached_input_tokens", "integer default 0")?;
    add_column_if_missing(
        database,
        "runs",
        "cache_creation_input_tokens",
        "integer default 0",
    )?;
    add_column_if_missing(database, "runs", "output_tokens", "integer default 0")?;
    add_column_if_missing(database, "runs", "cost_usd", "real default 0")?;
    add_column_if_missing(database, "runs", "cost_inr", "real default 0")?;
    database.exec(sql::CREATE_RUN_KEY_INDEX, ())?;
    Ok(())
}

fn add_column_if_missing(
    database: &Database,
    table: &str,
    column: &str,
    column_type: &str,
) -> DatabaseResult<()> {
    let existing = database.query_collect(&format!("pragma table_info({table})"), (), |row| {
        row.get::<String>(1)
    })?;
    if existing.iter().any(|name| name == column) {
        return Ok(());
    }
    database.exec(
        &format!("alter table {table} add column {column} {column_type}"),
        (),
    )?;
    Ok(())
}
