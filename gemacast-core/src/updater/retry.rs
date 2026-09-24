use std::future::Future;
use std::time::Duration;

use crate::domain::error::UpdaterError;

#[derive(Clone, Copy)]
pub(crate) struct RetryPolicy {
    retries: u32,
    initial_backoff: Duration,
}

impl RetryPolicy {
    pub(crate) const fn for_network_requests() -> Self {
        Self {
            retries: 3,
            initial_backoff: Duration::from_secs(1),
        }
    }

    pub(crate) async fn run<F, FutureResult, Value>(
        &self,
        operation: F,
    ) -> Result<Value, UpdaterError>
    where
        F: Fn() -> FutureResult,
        FutureResult: Future<Output = Result<Value, UpdaterError>>,
    {
        let mut backoff = self.initial_backoff;
        let mut last_error = None;

        for attempt in 0..=self.retries {
            match operation().await {
                Ok(value) => return Ok(value),
                Err(error) => {
                    last_error = Some(error);
                    if attempt < self.retries {
                        tracing::warn!(
                            "Attempt {}/{} failed: {}. Retrying in {}ms...",
                            attempt + 1,
                            self.retries + 1,
                            last_error.as_ref().expect("retry failure is recorded"),
                            backoff.as_millis()
                        );
                        tokio::time::sleep(backoff).await;
                        backoff *= 2;
                    }
                }
            }
        }

        Err(last_error.expect("retry policy always performs one attempt"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_retry_policy_returns_the_first_successful_result() {
        let policy = RetryPolicy {
            retries: 3,
            initial_backoff: Duration::from_millis(10),
        };

        let result = policy.run(|| async { Ok::<_, UpdaterError>(42) }).await;

        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn a_retry_policy_returns_the_last_failure_after_every_attempt() {
        let policy = RetryPolicy {
            retries: 2,
            initial_backoff: Duration::from_millis(10),
        };
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempts_for_operation = attempts.clone();

        let result = policy
            .run(move || {
                let attempts = attempts_for_operation.clone();
                async move {
                    attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err::<(), _>(UpdaterError::PlatformNotPublished {
                        platform_key: "test".to_string(),
                    })
                }
            })
            .await;

        assert!(matches!(
            result.unwrap_err(),
            UpdaterError::PlatformNotPublished { .. }
        ));
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
    }
}
