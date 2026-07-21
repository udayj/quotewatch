use super::cli::DateArg;
use super::error::AppError;
use crate::error::Result;
use chrono::{FixedOffset, NaiveDate, Utc};

pub(super) fn resolve_dates(
    date: Option<DateArg>,
    start: Option<DateArg>,
    end: Option<DateArg>,
) -> Result<(NaiveDate, NaiveDate)> {
    if let Some(date) = date {
        if start.is_some() || end.is_some() {
            return Err(AppError::InvalidArgs(
                "use either --date or --start/--end, not both".to_string(),
            )
            .into());
        }
        let resolved = resolve_date_arg(date)?;
        return Ok((resolved, resolved));
    }

    match (start, end) {
        (Some(start), Some(end)) => {
            let start = resolve_date_arg(start)?;
            let end = resolve_date_arg(end)?;
            if end >= start {
                Ok((start, end))
            } else {
                Err(AppError::InvalidArgs("--end must be on or after --start".to_string()).into())
            }
        }
        (None, None) => {
            let today = today_ist()?;
            Ok((today, today))
        }
        _ => Err(AppError::InvalidArgs("provide both --start and --end".to_string()).into()),
    }
}

pub(super) fn resolve_date_arg(date: DateArg) -> Result<NaiveDate> {
    match date {
        DateArg::Date(date) => Ok(date),
        DateArg::TodayIst => today_ist(),
    }
}

fn today_ist() -> Result<NaiveDate> {
    let ist = FixedOffset::east_opt(5 * 3600 + 30 * 60).ok_or(AppError::DateOverflow)?;
    Ok(Utc::now().with_timezone(&ist).date_naive())
}
