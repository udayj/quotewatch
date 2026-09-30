use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::header::CONTENT_TYPE;
use serde::de::DeserializeOwned;
use std::ops::Deref;
use std::thread;
use std::time::Duration;
use thiserror::Error;

mod gmail;

#[derive(Debug, Error)]
pub(crate) enum HttpError {
    #[error("failed to build HTTP client: {0}")]
    BuildClient(#[source] reqwest::Error),

    #[error("{operation} request could not be cloned for retry")]
    RequestNotCloneable { operation: String },

    #[error("{operation} request failed: {source}")]
    Request {
        operation: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("{operation} returned {status}: {body}")]
    Status {
        operation: String,
        status: StatusCode,
        body: String,
    },

    #[error("{operation} failed after {attempts} attempts: {last_error}")]
    RetryExhausted {
        operation: String,
        attempts: usize,
        last_error: String,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct RetryableHttpClient {
    client: Client,
    policy: RetryPolicy,
}

impl RetryableHttpClient {
    pub(crate) fn gmail() -> Result<Self, HttpError> {
        Self::with_timeout(Duration::from_secs(45), RetryPolicy::gmail())
    }

    pub(crate) fn llm() -> Result<Self, HttpError> {
        Self::with_timeout(Duration::from_secs(180), RetryPolicy::llm())
    }

    fn with_timeout(timeout: Duration, policy: RetryPolicy) -> Result<Self, HttpError> {
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(HttpError::BuildClient)?;
        Ok(Self { client, policy })
    }

    pub(crate) fn execute<T>(
        &self,
        operation: &str,
        request: RequestBuilder,
    ) -> Result<T, HttpError>
    where
        T: DeserializeOwned,
    {
        for attempt in 1..=self.policy.max_attempts {
            let request = request
                .try_clone()
                .ok_or_else(|| HttpError::RequestNotCloneable {
                    operation: operation.to_string(),
                })?;

            if self.policy.gmail {
                gmail::pace_request();
            }

            match request.send() {
                Ok(response) if response.status().is_success() => {
                    let metadata = response_metadata(&response);
                    match response.json() {
                        Ok(value) => return Ok(value),
                        Err(error) => {
                            let summary = format!(
                                "{metadata}; response body decode failed: {}",
                                error_chain(&error)
                            );
                            if self.policy.should_retry_decode_error(attempt, &error) {
                                self.sleep_before_retry(operation, attempt, summary);
                                continue;
                            }
                            if self.policy.is_retryable_decode_error(&error) {
                                return Err(HttpError::RetryExhausted {
                                    operation: operation.to_string(),
                                    attempts: self.policy.max_attempts,
                                    last_error: summary,
                                });
                            }
                            return Err(HttpError::Request {
                                operation: operation.to_string(),
                                source: error,
                            });
                        }
                    }
                }
                Ok(response) => {
                    let status = response.status();
                    let retry_after = if self.policy.gmail {
                        gmail::retry_after(response.headers(), std::time::SystemTime::now())
                    } else {
                        None
                    };
                    let body = response_body(response);
                    if self.policy.is_retryable_response(status, &body) {
                        // Do not retry earlier than Gmail permits or let a server
                        // header turn a bounded retry loop into an unbounded wait.
                        if retry_after.is_some_and(|delay| delay > self.policy.max_delay) {
                            return Err(HttpError::RetryExhausted {
                                operation: operation.to_string(),
                                attempts: attempt,
                                last_error: format!(
                                    "HTTP {status}: Retry-After exceeds the {}s maximum wait: {body}",
                                    self.policy.max_delay.as_secs()
                                ),
                            });
                        }
                        if self.policy.has_attempt_remaining(attempt) {
                            let delay = self.policy.delay(attempt);
                            let delay = if self.policy.gmail {
                                gmail::retry_delay(delay, retry_after).min(self.policy.max_delay)
                            } else {
                                delay
                            };
                            self.sleep(operation, attempt, delay, format!("HTTP {status}"));
                            continue;
                        }
                        return Err(HttpError::RetryExhausted {
                            operation: operation.to_string(),
                            attempts: self.policy.max_attempts,
                            last_error: format!("HTTP {status}: {body}"),
                        });
                    }
                    return Err(HttpError::Status {
                        operation: operation.to_string(),
                        status,
                        body,
                    });
                }
                Err(error) => {
                    if self.policy.should_retry_error(attempt, &error) {
                        let summary = error.to_string();
                        self.sleep_before_retry(operation, attempt, summary);
                        continue;
                    }
                    if self.policy.is_retryable_error(&error) {
                        return Err(HttpError::RetryExhausted {
                            operation: operation.to_string(),
                            attempts: self.policy.max_attempts,
                            last_error: error.to_string(),
                        });
                    }
                    return Err(HttpError::Request {
                        operation: operation.to_string(),
                        source: error,
                    });
                }
            }
        }

        Err(HttpError::RetryExhausted {
            operation: operation.to_string(),
            attempts: self.policy.max_attempts,
            last_error: "retry loop exited unexpectedly".to_string(),
        })
    }

    fn sleep_before_retry(&self, operation: &str, attempt: usize, reason: String) {
        let delay = self.policy.delay(attempt);
        self.sleep(operation, attempt, delay, reason);
    }

    fn sleep(&self, operation: &str, attempt: usize, delay: Duration, reason: String) {
        eprintln!(
            "{operation} failed on attempt {attempt}/{}; retrying in {:.3}s: {reason}",
            self.policy.max_attempts,
            delay.as_secs_f64()
        );
        thread::sleep(delay);
    }

    #[cfg(test)]
    fn with_client(client: Client, policy: RetryPolicy) -> Self {
        Self { client, policy }
    }
}

impl Deref for RetryableHttpClient {
    type Target = Client;

    fn deref(&self) -> &Self::Target {
        &self.client
    }
}

#[derive(Clone, Copy, Debug)]
struct RetryPolicy {
    gmail: bool,
    max_attempts: usize,
    initial_delay: Duration,
    max_delay: Duration,
    retry_decode_errors: bool,
}

impl RetryPolicy {
    fn gmail() -> Self {
        Self {
            gmail: true,
            max_attempts: 7,
            initial_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(64),
            retry_decode_errors: true,
        }
    }

    fn llm() -> Self {
        Self {
            gmail: false,
            max_attempts: 5,
            initial_delay: Duration::from_secs(10),
            max_delay: Duration::from_secs(40),
            retry_decode_errors: false,
        }
    }

    #[cfg(test)]
    fn test(max_attempts: usize) -> Self {
        Self {
            gmail: false,
            max_attempts,
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            retry_decode_errors: true,
        }
    }

    fn should_retry_error(&self, attempt: usize, error: &reqwest::Error) -> bool {
        self.has_attempt_remaining(attempt) && self.is_retryable_error(error)
    }

    fn should_retry_decode_error(&self, attempt: usize, error: &reqwest::Error) -> bool {
        self.has_attempt_remaining(attempt) && self.is_retryable_decode_error(error)
    }

    fn is_retryable_response(&self, status: StatusCode, body: &str) -> bool {
        status == StatusCode::REQUEST_TIMEOUT
            || status == StatusCode::TOO_MANY_REQUESTS
            || status.is_server_error()
            || (self.gmail && status == StatusCode::FORBIDDEN && gmail::is_quota_error(body))
    }

    fn is_retryable_error(&self, error: &reqwest::Error) -> bool {
        error.is_timeout() || error.is_connect() || error.is_request()
    }

    fn is_retryable_decode_error(&self, error: &reqwest::Error) -> bool {
        self.retry_decode_errors && error.is_decode()
    }

    fn delay(&self, attempt: usize) -> Duration {
        let multiplier = 1_u64.checked_shl((attempt - 1) as u32).unwrap_or(u64::MAX);
        let seconds = self
            .initial_delay
            .as_secs()
            .saturating_mul(multiplier)
            .min(self.max_delay.as_secs());
        Duration::from_secs(seconds)
    }

    fn has_attempt_remaining(&self, attempt: usize) -> bool {
        attempt < self.max_attempts
    }
}

fn response_body(response: Response) -> String {
    response
        .text()
        .unwrap_or_else(|error| format!("failed to read response body: {error}"))
}

fn response_metadata(response: &Response) -> String {
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("missing");
    let content_length = response
        .content_length()
        .map_or_else(|| "unknown".to_string(), |length| length.to_string());
    format!(
        "HTTP {}; content-type={content_type}; content-length={content_length}",
        response.status()
    )
}

fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut messages = Vec::new();
    let mut current = Some(error);
    while let Some(cause) = current {
        let message = cause.to_string();
        if messages.last() != Some(&message) {
            messages.push(message);
        }
        current = cause.source();
    }
    messages.join(": ")
}

#[cfg(test)]
mod tests;
