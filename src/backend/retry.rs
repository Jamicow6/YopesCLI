//! Retry policy: sending a request with backoff while the failure looks
//! temporary, and reporting each retry to the interface.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use reqwest::{Response, StatusCode};
use tokio::sync::mpsc;

use super::StreamEvent;

/// How long to wait between retries, doubling up to the cap.
const RETRY_MIN_DELAY: Duration = Duration::from_secs(1);
const RETRY_MAX_DELAY: Duration = Duration::from_secs(30);
/// How many times a request is tried before the error is reported.
///
/// Without a cap a provider that stays down hangs the turn forever, because
/// every failure looks temporary.
const MAX_ATTEMPTS: u32 = 5;

/// Whether a failed response is worth another try.
///
/// Client errors are the provider rejecting the request (bad key, unknown
/// model, malformed body), so retrying cannot change the answer. Timeouts and
/// rate limits are also 4xx, but they explicitly ask the caller to try again.
fn retryable_status(status: StatusCode) -> bool {
    status.is_server_error()
        || status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
}

/// The `Retry-After` header, in seconds, when the provider sends one.
fn retry_after(response: &Response) -> Option<Duration> {
    let value = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?;
    let seconds: u64 = value.trim().parse().ok()?;
    Some(Duration::from_secs(seconds.clamp(1, 60)))
}

/// Send a request, retrying while the failure looks temporary.
///
/// DNS failures and refused connections are usually a dropped network or a
/// sleeping server, so giving up wastes a turn the user can do nothing about.
/// Server errors and rate limits are just as temporary. A rejected request
/// (a 4xx other than 408/429) will not change on a retry, so it is returned
/// to the caller.
///
/// Each retry is reported through `events`, so the interface can show what is
/// happening instead of the request hanging in silence.
pub(super) async fn send_with_retry<F>(
    provider: &str,
    request_id: u64,
    events: &mpsc::UnboundedSender<StreamEvent>,
    build: F,
) -> Result<Response>
where
    F: Fn() -> reqwest::RequestBuilder + Send,
{
    let mut delay = RETRY_MIN_DELAY;
    let mut attempt: u32 = 0;
    loop {
        attempt = attempt.saturating_add(1);
        let (error, wait) = match build().send().await {
            Ok(response) if response.status().is_success() => return Ok(response),
            Ok(response) => {
                let status = response.status();
                let wait = retry_after(&response);
                let body = response.text().await.unwrap_or_default();
                let detail = response_error_detail(&body);
                let error = anyhow!("{provider} returned {}: {detail}", status.as_u16());
                if !retryable_status(status) {
                    return Err(error);
                }
                (error, wait)
            }
            Err(error) => {
                let transient = error.is_connect() || error.is_timeout() || error.is_request();
                if !transient {
                    return Err(error).with_context(|| format!("connecting to {provider}"));
                }
                (error.into(), None)
            }
        };
        if attempt >= MAX_ATTEMPTS {
            return Err(error)
                .with_context(|| format!("{provider} failed after {attempt} attempts"));
        }
        let wait = wait.unwrap_or(delay);
        // The interface shows the wait, so a slow provider does not look hung.
        let _ = events.send(StreamEvent::Retrying {
            request_id,
            attempt,
            delay_secs: wait.as_secs(),
            reason: format!("{error:#}"),
        });
        tokio::time::sleep(wait).await;
        delay = (delay * 2).min(RETRY_MAX_DELAY);
    }
}

pub(super) fn response_error_detail(body: &str) -> String {
    let value = serde_json::from_str::<serde_json::Value>(body).ok();
    let detail = value
        .as_ref()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("error"))
                .or_else(|| value.get("message"))
        })
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| body.to_owned());
    let detail = detail.trim();
    if detail.is_empty() {
        return "empty error response".to_owned();
    }
    detail.chars().take(500).collect()
}

#[cfg(test)]
mod tests {
    use super::retryable_status;

    #[test]
    fn rejected_requests_are_not_retried() {
        use reqwest::StatusCode;

        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
            StatusCode::UNPROCESSABLE_ENTITY,
        ] {
            assert!(!retryable_status(status), "{status} should not be retried");
        }
        for status in [
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            assert!(retryable_status(status), "{status} should be retried");
        }
    }
}
