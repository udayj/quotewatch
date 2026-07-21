use super::{DatabaseError, DatabaseResult};
use std::thread;
use std::time::Duration;

pub(super) fn retry_database_operation<T, F>(
    operation: &'static str,
    mut action: F,
) -> DatabaseResult<T>
where
    F: FnMut() -> DatabaseResult<T>,
{
    const MAX_RETRIES: usize = 6;
    const MAX_ATTEMPTS: usize = MAX_RETRIES + 1;

    for attempt in 1..=MAX_ATTEMPTS {
        match action() {
            Ok(value) => return Ok(value),
            Err(error) if attempt < MAX_ATTEMPTS && is_retryable_database_error(&error) => {
                let delay = retry_delay(attempt);
                eprintln!(
                    "database {operation} failed on attempt {attempt}/{MAX_ATTEMPTS}; retry {attempt}/{MAX_RETRIES} in {}s: {error}",
                    delay.as_secs()
                );
                thread::sleep(delay);
            }
            Err(error) if is_retryable_database_error(&error) => {
                return Err(DatabaseError::RetryExhausted {
                    operation,
                    attempts: MAX_ATTEMPTS,
                    source: Box::new(error),
                });
            }
            Err(error) => return Err(error),
        }
    }

    Err(DatabaseError::State(format!(
        "database retry loop for {operation} exited unexpectedly"
    )))
}

fn retry_delay(attempt: usize) -> Duration {
    let seconds = match attempt {
        1 => 1,
        2 => 2,
        3 => 4,
        4 => 8,
        5 => 16,
        _ => 30,
    };
    Duration::from_secs(seconds)
}

fn is_retryable_database_error(error: &DatabaseError) -> bool {
    match error {
        DatabaseError::Libsql(error) => is_retryable_libsql_error(error),
        DatabaseError::RetryExhausted { .. }
        | DatabaseError::Runtime(_)
        | DatabaseError::Json(_)
        | DatabaseError::Date(_)
        | DatabaseError::Token(_)
        | DatabaseError::PreparedRun(_)
        | DatabaseError::MissingEnv(_)
        | DatabaseError::State(_) => false,
    }
}

fn is_retryable_libsql_error(error: &libsql::Error) -> bool {
    match error {
        libsql::Error::ConnectionFailed(_)
        | libsql::Error::Hrana(_)
        | libsql::Error::WriteDelegation(_)
        | libsql::Error::Replication(_)
        | libsql::Error::Sync(_)
        | libsql::Error::WalConflict
        | libsql::Error::TransactionalBatchError(_) => true,
        libsql::Error::SqliteFailure(code, message)
        | libsql::Error::RemoteSqliteFailure(code, _, message) => {
            matches!(*code, 5 | 6 | 261 | 262) || retryable_error_message(message)
        }
        other => retryable_error_message(&other.to_string()),
    }
}

fn retryable_error_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "database is locked",
        "database table is locked",
        "busy",
        "timeout",
        "timed out",
        "temporarily unavailable",
        "connection",
        "network",
        "transport",
        "rate limit",
        "too many requests",
        "429",
        "500",
        "502",
        "503",
        "504",
        "temporary",
        "error trying to connect",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}
