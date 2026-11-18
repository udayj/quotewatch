use chacha20poly1305::aead::rand_core::{OsRng, RngCore};
use reqwest::header::{HeaderMap, RETRY_AFTER};
use serde_json::Value;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

// Full thread reads cost 120 units, so 40 requests/minute use 4,800 units
// and leave headroom below Gmail's 6,000-unit per-user/project limit.
const REQUEST_INTERVAL: Duration = Duration::from_millis(1_500);

// Daily fetches and active-thread refreshes create separate clients. Keep
// their pacing shared for the lifetime of this process.
static LAST_REQUEST: Mutex<Option<Instant>> = Mutex::new(None);

pub(super) fn pace_request() {
    // A previous panic cannot invalidate an optional timestamp.
    let mut last_request = LAST_REQUEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let delay = request_delay(*last_request, Instant::now());
    if !delay.is_zero() {
        thread::sleep(delay);
    }
    *last_request = Some(Instant::now());
}

fn request_delay(last_request: Option<Instant>, now: Instant) -> Duration {
    match last_request {
        Some(last_request) => REQUEST_INTERVAL.saturating_sub(now.duration_since(last_request)),
        None => Duration::ZERO,
    }
}

pub(super) fn is_quota_error(body: &str) -> bool {
    let Ok(body) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let error = &body["error"];
    let legacy_reason = error["errors"].as_array().is_some_and(|errors| {
        errors.iter().any(|error| {
            matches!(
                error["reason"].as_str(),
                Some("rateLimitExceeded" | "userRateLimitExceeded")
            )
        })
    });
    let structured_reason = error["details"].as_array().is_some_and(|details| {
        details.iter().any(|detail| {
            detail["@type"] == "type.googleapis.com/google.rpc.ErrorInfo"
                && detail["reason"] == "RATE_LIMIT_EXCEEDED"
        })
    });
    legacy_reason || structured_reason
}

pub(super) fn retry_after(headers: &HeaderMap, now: SystemTime) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let date = SystemTime::from(date);
    Some(date.duration_since(now).unwrap_or_default())
}

pub(super) fn retry_diagnostics(body: &str, retry_after: Option<Duration>) -> String {
    let mut reasons = Vec::new();
    let mut quotas = Vec::new();
    if let Ok(body) = serde_json::from_str::<Value>(body) {
        let error = &body["error"];
        for entry in error["errors"].as_array().into_iter().flatten() {
            if let Some(reason) = entry["reason"].as_str() {
                reasons.push(reason.to_string());
            }
        }
        for detail in error["details"].as_array().into_iter().flatten() {
            if detail["@type"] != "type.googleapis.com/google.rpc.ErrorInfo" {
                continue;
            }
            if let Some(reason) = detail["reason"].as_str() {
                reasons.push(reason.to_string());
            }
            // Keep only quota diagnostics, excluding account identifiers and
            // arbitrary provider content from routine retry logs.
            let mut quota = serde_json::Map::new();
            for key in [
                "quota_metric",
                "quota_limit",
                "quota_limit_value",
                "quota_unit",
                "service",
            ] {
                if let Some(value) = detail["metadata"][key].as_str() {
                    quota.insert(key.to_string(), Value::String(value.to_string()));
                }
            }
            if !quota.is_empty() {
                quotas.push(Value::Object(quota));
            }
        }
    }
    serde_json::json!({
        "reasons": reasons,
        "quotas": quotas,
        "retry_after_ms": retry_after.map(|delay| delay.as_millis()),
        "request_interval_ms": REQUEST_INTERVAL.as_millis(),
    })
    .to_string()
}

pub(super) fn retry_delay(backoff: Duration, retry_after: Option<Duration>) -> Duration {
    // Reuse the existing OS random source; jitter does not need to fail the
    // request if randomness is temporarily unavailable.
    let mut bytes = [0; 2];
    let jitter = if OsRng.try_fill_bytes(&mut bytes).is_ok() {
        Duration::from_millis(u64::from(u16::from_ne_bytes(bytes) % 1_000))
    } else {
        Duration::ZERO
    };
    backoff.max(retry_after.unwrap_or_default()) + jitter
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn diagnostics_include_quota_context_without_unrelated_content() {
        let body = r#"{"error":{"message":"private content","errors":[{"reason":"userRateLimitExceeded"}],"details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"RATE_LIMIT_EXCEEDED","metadata":{"quota_limit_value":"6000","quota_metric":"gmail.googleapis.com/read_requests","consumer":"private account"}}]}}"#;
        let diagnostics = retry_diagnostics(body, Some(Duration::from_secs(3)));
        let value: Value = serde_json::from_str(&diagnostics).unwrap();
        assert_eq!(value["reasons"][0], "userRateLimitExceeded");
        assert_eq!(value["reasons"][1], "RATE_LIMIT_EXCEEDED");
        assert_eq!(value["quotas"][0]["quota_limit_value"], "6000");
        assert_eq!(value["retry_after_ms"], 3000);
        assert_eq!(value["request_interval_ms"], 1500);
        assert!(!diagnostics.contains("private"));
    }

    #[test]
    fn diagnostics_handle_missing_or_non_json_error_details() {
        for body in ["upstream unavailable", "{}", "null"] {
            let value: Value = serde_json::from_str(&retry_diagnostics(body, None)).unwrap();
            assert_eq!(value["reasons"], serde_json::json!([]));
            assert_eq!(value["quotas"], serde_json::json!([]));
            assert!(value["retry_after_ms"].is_null());
        }
    }

    #[test]
    fn pacing_waits_only_for_the_remaining_interval() {
        let now = Instant::now();
        assert_eq!(request_delay(None, now), Duration::ZERO);
        assert_eq!(request_delay(Some(now), now), REQUEST_INTERVAL);
        assert_eq!(
            request_delay(Some(now), now + Duration::from_millis(250)),
            Duration::from_millis(1_250)
        );
        assert_eq!(
            request_delay(Some(now), now + REQUEST_INTERVAL),
            Duration::ZERO
        );
        assert_eq!(
            request_delay(Some(now), now + Duration::from_secs(5)),
            Duration::ZERO
        );
    }

    #[test]
    fn full_thread_reads_leave_twenty_percent_quota_headroom() {
        let requests_per_minute =
            Duration::from_secs(60).as_millis() / REQUEST_INTERVAL.as_millis();
        let full_thread_quota_units = requests_per_minute * 120;
        assert!(full_thread_quota_units <= 4_800);
    }

    #[test]
    fn quota_detection_requires_a_structured_rate_limit_reason() {
        for body in [
            r#"{"error":{"errors":[{"reason":"rateLimitExceeded"}]}}"#,
            r#"{"error":{"errors":[{"reason":"userRateLimitExceeded"}]}}"#,
            r#"{"error":{"details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"RATE_LIMIT_EXCEEDED"}]}}"#,
        ] {
            assert!(is_quota_error(body), "{body}");
        }
        for body in [
            "rateLimitExceeded",
            "<html>Forbidden</html>",
            "{}",
            "null",
            r#"{"error":{"message":"rateLimitExceeded","status":"PERMISSION_DENIED"}}"#,
            r#"{"error":{"errors":[{"reason":"insufficientPermissions"}]}}"#,
            r#"{"error":{"errors":[{"reason":"domainPolicy"}]}}"#,
            r#"{"error":{"errors":[{"reason":"dailyLimitExceeded"}]}}"#,
            r#"{"error":{"details":[{"reason":"RATE_LIMIT_EXCEEDED"}]}}"#,
        ] {
            assert!(!is_quota_error(body), "{body}");
        }
    }

    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        let now = SystemTime::from(
            chrono::DateTime::parse_from_rfc2822("Fri, 04 Sep 2026 12:00:00 GMT").unwrap(),
        );
        let mut headers = HeaderMap::new();
        assert_eq!(retry_after(&headers, now), None);
        for (value, expected) in [
            ("30", Some(Duration::from_secs(30))),
            (" 30 ", Some(Duration::from_secs(30))),
            ("0", Some(Duration::ZERO)),
            (
                "Fri, 04 Sep 2026 12:00:45 GMT",
                Some(Duration::from_secs(45)),
            ),
            ("Fri, 04 Sep 2026 11:59:00 GMT", Some(Duration::ZERO)),
            ("-1", None),
            ("invalid", None),
        ] {
            headers.insert(RETRY_AFTER, HeaderValue::from_str(value).unwrap());
            assert_eq!(retry_after(&headers, now), expected, "{value}");
        }
    }

    #[test]
    fn retry_delay_respects_backoff_and_retry_after_with_bounded_jitter() {
        for (backoff, retry_after, minimum) in [(2, None, 2), (2, Some(30), 30), (32, Some(10), 32)]
        {
            let delay = retry_delay(
                Duration::from_secs(backoff),
                retry_after.map(Duration::from_secs),
            );
            assert!(delay >= Duration::from_secs(minimum));
            assert!(delay < Duration::from_secs(minimum + 1));
        }
    }
}
