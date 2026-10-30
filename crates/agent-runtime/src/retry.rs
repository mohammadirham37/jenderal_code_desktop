//! Retry opening a model stream on transient provider errors (rate limits,
//! overload, 5xx, network), with exponential backoff. Auth, bad-request and
//! not-found errors fail immediately.

use futures::stream::BoxStream;
use jcode_common::{ModelRequest, StreamEvent};
use jcode_provider_core::AiProvider;
use jcode_tool_terminal::CancelToken;
use std::time::Duration;

/// Waits before attempts 2, 3, ... (about one minute in total).
pub const RETRY_DELAYS: [Duration; 5] = [
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(30),
];

/// Is this provider error worth retrying?
pub fn is_retryable(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    if let Some(code) = status_code(&lower) {
        return matches!(code, 408 | 409 | 425 | 429) || (500..600).contains(&code);
    }
    [
        "connecting to",
        "connection",
        "timed out",
        "timeout",
        "overloaded",
        "rate limit",
        "temporarily unavailable",
        "error sending request",
    ]
    .iter()
    .any(|k| lower.contains(k))
}

/// First `(NNN` HTTP status in the message, e.g. "provider error (429)" or
/// "Anthropic error (529 <unknown status code>)".
fn status_code(message: &str) -> Option<u16> {
    message.match_indices('(').find_map(|(i, _)| {
        let digits: String = message[i + 1..].chars().take_while(char::is_ascii_digit).collect();
        (digits.len() == 3).then(|| digits.parse().ok()).flatten()
    })
}

/// Open the stream, retrying transient failures. `on_retry(attempt, delay,
/// error)` is awaited before each wait (attempt counts from 2). Returns
/// `Ok(None)` if cancelled while waiting.
pub async fn open_stream<F, Fut>(
    provider: &dyn AiProvider,
    request: &ModelRequest,
    cancel: &CancelToken,
    delays: &[Duration],
    mut on_retry: F,
) -> anyhow::Result<Option<BoxStream<'static, StreamEvent>>>
where
    F: FnMut(usize, Duration, String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut attempt = 1;
    loop {
        match provider.stream(request.clone()).await {
            Ok(s) => return Ok(Some(s)),
            Err(e) => {
                let message = format!("{e:#}");
                let Some(delay) = delays.get(attempt - 1).copied().filter(|_| is_retryable(&message)) else {
                    return Err(e);
                };
                attempt += 1;
                on_retry(attempt, delay, message).await;
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = cancel.cancelled() => return Ok(None),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::StreamExt;
    use jcode_common::ModelInfo;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn classifies_errors() {
        for retry in [
            "provider error (429) while calling chat completions: slow down",
            "Anthropic error (529 <unknown status code>): overloaded",
            "provider error (503) while calling chat completions: x",
            "connecting to provider: error sending request",
            "operation timed out",
        ] {
            assert!(is_retryable(retry), "{retry}");
        }
        for fail in [
            "provider error (401) while calling chat completions: bad key",
            "provider error (400) while calling chat completions: invalid model (gpt-x)",
            "Anthropic requires an API key",
        ] {
            assert!(!is_retryable(fail), "{fail}");
        }
    }

    struct Flaky {
        failures: usize,
        error: &'static str,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl AiProvider for Flaky {
        fn id(&self) -> &str {
            "flaky"
        }
        fn name(&self) -> &str {
            "Flaky"
        }
        async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
            Ok(vec![])
        }
        async fn stream(&self, _r: ModelRequest) -> anyhow::Result<BoxStream<'static, StreamEvent>> {
            if self.calls.fetch_add(1, Ordering::SeqCst) < self.failures {
                anyhow::bail!(self.error);
            }
            Ok(futures::stream::iter([StreamEvent::TextDelta("ok".into())]).boxed())
        }
    }

    fn request() -> ModelRequest {
        ModelRequest {
            model: "m".into(),
            messages: vec![],
            tools: vec![],
            system: None,
            temperature: None,
            max_tokens: None,
        }
    }

    const FAST: [Duration; 3] = [Duration::from_millis(1); 3];

    #[tokio::test]
    async fn retries_transient_errors_then_succeeds() {
        let p = Flaky { failures: 2, error: "provider error (503) while calling x: busy", calls: AtomicUsize::new(0) };
        let mut retries = Vec::new();
        let s = open_stream(&p, &request(), &CancelToken::new(), &FAST, |a, _, _| {
            retries.push(a);
            async {}
        }).await.unwrap();
        assert!(s.is_some());
        assert_eq!(retries, [2, 3]);
    }

    #[tokio::test]
    async fn gives_up_after_the_delays_or_on_permanent_errors() {
        let p = Flaky { failures: 10, error: "provider error (503) while calling x: busy", calls: AtomicUsize::new(0) };
        assert!(open_stream(&p, &request(), &CancelToken::new(), &FAST, |_, _, _| async {}).await.is_err());
        assert_eq!(p.calls.load(Ordering::SeqCst), 4, "1 try + 3 retries");

        let p = Flaky { failures: 10, error: "provider error (401) while calling x: bad key", calls: AtomicUsize::new(0) };
        assert!(open_stream(&p, &request(), &CancelToken::new(), &FAST, |_, _, _| async {}).await.is_err());
        assert_eq!(p.calls.load(Ordering::SeqCst), 1);
    }
}
