
//! Повторы запросов к Apple: 429 и 5xx, паузы 2 с, 5 с, 10 с.
use crate::errors::AppError;
use std::future::Future;
use std::time::Duration;

/// Паузы в бою; число пауз = число повторов.
pub const APPLE_DELAYS: [Duration; 3] = [Duration::from_secs(2), Duration::from_secs(5), Duration::from_secs(10)];

/// Вызывает `f`; повторяет только при `err.retryable()`.
/// Вызовов не больше `delays.len() + 1`.
pub async fn with_retry<T, F, Fut>(delays: &[Duration], mut f: F) -> Result<T, AppError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, AppError>>,
{
    let mut attempt = 0;
    loop {
        match f().await {
            Err(e) if e.retryable() && attempt < delays.len() => {
                tokio::time::sleep(delays[attempt]).await;
                attempt += 1;
            }
            r => return r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const ZERO: [Duration; 3] = [Duration::ZERO; 3];
    fn unavailable() -> AppError {
        AppError::AppleUnavailable { status: Some(503) }
    }

    #[tokio::test]
    async fn succeeds_after_two_failures() {
        let n = Cell::new(0);
        let r = with_retry(&ZERO, || {
            n.set(n.get() + 1);
            let k = n.get();
            async move { if k <= 2 { Err(unavailable()) } else { Ok(k) } }
        })
        .await;
        assert_eq!(r, Ok(3));
        assert_eq!(n.get(), 3);
    }

    #[tokio::test]
    async fn gives_up_after_three_retries() {
        let n = Cell::new(0);
        let r: Result<(), _> = with_retry(&ZERO, || {
            n.set(n.get() + 1);
            async { Err(unavailable()) }
        })
        .await;
        assert_eq!(r, Err(unavailable()));
        assert_eq!(n.get(), 4);
    }

    #[tokio::test]
    async fn non_retryable_runs_once() {
        let n = Cell::new(0);
        let r: Result<(), _> = with_retry(&ZERO, || {
            n.set(n.get() + 1);
            async { Err(AppError::WrongCredentials) }
        })
        .await;
        assert_eq!(r, Err(AppError::WrongCredentials));
        assert_eq!(n.get(), 1);
    }

    #[test]
    fn production_delays() {
        assert_eq!(APPLE_DELAYS.map(|d| d.as_secs()), [2, 5, 10]);
    }
}
