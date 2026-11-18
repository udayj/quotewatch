use super::*;
use mockito::Server;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct TestResponse {
    ok: bool,
}

#[derive(Debug, Deserialize)]
struct ExpectedShape {
    #[serde(rename = "value")]
    _value: String,
}

fn gmail_test_client(max_attempts: usize) -> RetryableHttpClient {
    RetryableHttpClient::with_client(
        Client::new(),
        RetryPolicy {
            gmail: true,
            ..RetryPolicy::test(max_attempts)
        },
    )
}

#[test]
fn execute_retries_gmail_quota_errors_on_the_same_thread() {
    for body in [
        r#"{"error":{"code":403,"status":"PERMISSION_DENIED","errors":[{"reason":"rateLimitExceeded"}]}}"#,
        r#"{"error":{"errors":[{"reason":"userRateLimitExceeded"}]}}"#,
        r#"{"error":{"details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"RATE_LIMIT_EXCEEDED","metadata":{"quota_limit_value":"6000"}}]}}"#,
    ] {
        let mut server = Server::new();
        let quota_error = server
            .mock("GET", "/threads/19ed0855d6b4b952")
            .with_status(403)
            .with_body(body)
            .expect(1)
            .create();
        let success = server
            .mock("GET", "/threads/19ed0855d6b4b952")
            .with_status(200)
            .with_body(r#"{"ok":true}"#)
            .expect(1)
            .create();
        let client = gmail_test_client(2);

        let response: TestResponse = client
            .execute(
                "Gmail thread 19ed0855d6b4b952",
                client.get(format!("{}/threads/19ed0855d6b4b952", server.url())),
            )
            .unwrap();

        assert!(response.ok);
        quota_error.assert();
        success.assert();
    }
}

#[test]
fn execute_does_not_retry_other_gmail_forbidden_errors() {
    for body in [
        r#"{"error":{"errors":[{"reason":"insufficientPermissions"}]}}"#,
        r#"{"error":{"errors":[{"reason":"domainPolicy"}]}}"#,
        r#"{"error":{"errors":[{"reason":"dailyLimitExceeded"}]}}"#,
        "Forbidden: rateLimitExceeded",
    ] {
        let mut server = Server::new();
        let forbidden = server
            .mock("GET", "/forbidden")
            .with_status(403)
            .with_body(body)
            .expect(1)
            .create();
        let client = gmail_test_client(2);

        let error = client
            .execute::<TestResponse>(
                "Gmail read",
                client.get(format!("{}/forbidden", server.url())),
            )
            .unwrap_err();

        assert!(
            matches!(error, HttpError::Status { status, .. } if status == StatusCode::FORBIDDEN)
        );
        forbidden.assert();
    }
}

#[test]
fn execute_keeps_gmail_quota_retries_out_of_the_classifier() {
    let mut server = Server::new();
    let forbidden = server
        .mock("GET", "/classifier")
        .with_status(403)
        .with_body(r#"{"error":{"errors":[{"reason":"rateLimitExceeded"}]}}"#)
        .expect(1)
        .create();
    let client = RetryableHttpClient::with_client(Client::new(), RetryPolicy::llm());

    let error = client
        .execute::<TestResponse>(
            "Classifier request",
            client.get(format!("{}/classifier", server.url())),
        )
        .unwrap_err();

    assert!(matches!(error, HttpError::Status { status, .. } if status == StatusCode::FORBIDDEN));
    forbidden.assert();
}

#[test]
fn execute_preserves_quota_error_context_after_exhaustion() {
    let mut server = Server::new();
    let body = r#"{"error":{"errors":[{"reason":"rateLimitExceeded"}]}}"#;
    let quota_error = server
        .mock("GET", "/thread")
        .with_status(403)
        .with_body(body)
        .expect(3)
        .create();
    let client = gmail_test_client(3);

    let error = client
        .execute::<TestResponse>(
            "Gmail thread example",
            client.get(format!("{}/thread", server.url())),
        )
        .unwrap_err();

    let HttpError::RetryExhausted {
        operation,
        attempts,
        last_error,
    } = error
    else {
        panic!("expected retry exhaustion");
    };
    assert_eq!(operation, "Gmail thread example");
    assert_eq!(attempts, 3);
    assert!(last_error.contains("403 Forbidden"));
    assert!(last_error.contains(body));
    quota_error.assert();
}

#[test]
fn execute_does_not_retry_before_a_long_retry_after() {
    let mut server = Server::new();
    let quota_error = server
        .mock("GET", "/wait")
        .with_status(429)
        .with_header("retry-after", "86400")
        .with_body("wait until tomorrow")
        .expect(1)
        .create();
    let client = RetryableHttpClient::with_client(Client::new(), RetryPolicy::gmail());

    let error = client
        .execute::<TestResponse>("Gmail read", client.get(format!("{}/wait", server.url())))
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("Retry-After exceeds the 64s maximum wait")
    );
    quota_error.assert();
}

#[test]
fn execute_paces_gmail_across_clients_and_retries() {
    let mut server = Server::new();
    let first = server
        .mock("GET", "/daily")
        .with_status(200)
        .with_body(r#"{"ok":true}"#)
        .expect(1)
        .create();
    let retry = server
        .mock("GET", "/active")
        .with_status(429)
        .expect(1)
        .create();
    let success = server
        .mock("GET", "/active")
        .with_status(200)
        .with_body(r#"{"ok":true}"#)
        .expect(1)
        .create();
    let daily_client = gmail_test_client(1);
    let active_client = gmail_test_client(2);
    let started = std::time::Instant::now();

    daily_client
        .execute::<TestResponse>(
            "Daily fetch",
            daily_client.get(format!("{}/daily", server.url())),
        )
        .unwrap();
    active_client
        .execute::<TestResponse>(
            "Active-thread refresh",
            active_client.get(format!("{}/active", server.url())),
        )
        .unwrap();

    // Three attempts, including the retry, require two pacing intervals.
    // Only assert the lower bound so slow CI machines remain valid.
    assert!(started.elapsed() >= Duration::from_secs(3));
    first.assert();
    retry.assert();
    success.assert();
}

#[test]
fn gmail_backoff_spans_more_than_a_minute_and_remains_bounded() {
    let policy = RetryPolicy::gmail();
    let total: Duration = (1..policy.max_attempts)
        .map(|attempt| policy.delay(attempt))
        .sum();
    assert!(total > Duration::from_secs(60));
    assert!(total <= Duration::from_secs(180));
    assert_eq!(policy.delay(20), Duration::from_secs(64));
}

#[test]
fn execute_retries_after_too_many_requests() {
    let mut server = Server::new();
    let too_many_requests = server
        .mock("GET", "/retry")
        .with_status(429)
        .with_body("slow down")
        .expect(1)
        .create();
    let success = server
        .mock("GET", "/retry")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"ok":true}"#)
        .expect(1)
        .create();
    let client = RetryableHttpClient::with_client(Client::new(), RetryPolicy::test(2));

    let response: TestResponse = client
        .execute(
            "Test request",
            client.get(format!("{}/retry", server.url())),
        )
        .unwrap();

    assert!(response.ok);
    too_many_requests.assert();
    success.assert();
}

#[test]
fn execute_does_not_retry_bad_request() {
    let mut server = Server::new();
    let bad_request = server
        .mock("GET", "/bad")
        .with_status(400)
        .with_body("bad payload")
        .expect(1)
        .create();
    let client = RetryableHttpClient::with_client(Client::new(), RetryPolicy::test(2));

    let error = client
        .execute::<TestResponse>("Test request", client.get(format!("{}/bad", server.url())))
        .unwrap_err();

    assert!(matches!(error, HttpError::Status { status, .. } if status == StatusCode::BAD_REQUEST));
    bad_request.assert();
}

#[test]
fn execute_retries_after_response_decode_error() {
    let mut server = Server::new();
    let malformed = server
        .mock("GET", "/decode-retry")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"ok":"#)
        .expect(1)
        .create();
    let success = server
        .mock("GET", "/decode-retry")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"ok":true}"#)
        .expect(1)
        .create();
    let client = RetryableHttpClient::with_client(Client::new(), RetryPolicy::test(2));

    let response: TestResponse = client
        .execute(
            "Test request",
            client.get(format!("{}/decode-retry", server.url())),
        )
        .unwrap();

    assert!(response.ok);
    malformed.assert();
    success.assert();
}

#[test]
fn execute_reports_decode_details_after_retries_are_exhausted() {
    let mut server = Server::new();
    let malformed = server
        .mock("GET", "/malformed")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"unexpected":[]}"#)
        .expect(1)
        .create();
    let client = RetryableHttpClient::with_client(Client::new(), RetryPolicy::test(1));

    let error = client
        .execute::<ExpectedShape>(
            "Test request",
            client.get(format!("{}/malformed", server.url())),
        )
        .unwrap_err();

    let HttpError::RetryExhausted {
        attempts,
        last_error,
        ..
    } = error
    else {
        panic!("expected retry exhaustion");
    };
    assert_eq!(attempts, 1);
    assert!(last_error.contains("HTTP 200 OK"));
    assert!(last_error.contains("content-type=application/json"));
    assert!(last_error.contains("missing field `value`"));
    malformed.assert();
}
