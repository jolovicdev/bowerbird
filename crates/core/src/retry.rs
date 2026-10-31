//! Retries for requests that a flaky connection can lose: a dropped
//! connection, a timeout, or an overloaded server usually succeeds a moment
//! later, so one bad packet shouldn't cost the reader an answer.

use std::future::Future;
use std::time::Duration;

/// Pauses before the second and third attempts.
const WAITS: [Duration; 2] = [Duration::from_millis(800), Duration::from_secs(3)];

pub async fn with_retries<T, E, Fut>(
    attempt: impl FnMut() -> Fut,
    transient: impl Fn(&E) -> bool,
) -> Result<T, E>
where
    Fut: Future<Output = Result<T, E>>,
{
    retry_after(&WAITS, attempt, transient).await
}

async fn retry_after<T, E, Fut>(
    waits: &[Duration],
    mut attempt: impl FnMut() -> Fut,
    transient: impl Fn(&E) -> bool,
) -> Result<T, E>
where
    Fut: Future<Output = Result<T, E>>,
{
    let mut waits = waits.iter();
    loop {
        match attempt().await {
            Err(e) if transient(&e) => match waits.next() {
                Some(wait) => tokio::time::sleep(*wait).await,
                None => return Err(e),
            },
            done => return done,
        }
    }
}

/// A failed HTTP request worth trying again: no answer at all, or a server
/// saying it is overloaded or briefly unavailable.
pub fn transient_status(status: Option<u16>) -> bool {
    matches!(status, None | Some(502..=504))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[tokio::test]
    async fn transient_failures_are_retried_until_the_attempts_run_out() {
        let no_wait = [Duration::ZERO; 2];
        let calls = Cell::new(0);
        let flaky = || {
            calls.set(calls.get() + 1);
            let n = calls.get();
            async move { if n < 3 { Err("dropped") } else { Ok(n) } }
        };
        assert_eq!(retry_after(&no_wait, flaky, |_| true).await, Ok(3));

        calls.set(0);
        let always = || {
            calls.set(calls.get() + 1);
            async { Err::<(), _>("dropped") }
        };
        assert_eq!(
            retry_after(&no_wait, always, |_| true).await,
            Err("dropped")
        );
        assert_eq!(calls.get(), 3, "one try and two retries");

        calls.set(0);
        let refused = || {
            calls.set(calls.get() + 1);
            async { Err::<(), _>("bad key") }
        };
        assert_eq!(
            retry_after(&no_wait, refused, |_| false).await,
            Err("bad key")
        );
        assert_eq!(
            calls.get(),
            1,
            "a failure a retry can't fix is returned at once"
        );
    }

    #[test]
    fn only_lost_and_overload_failures_count_as_transient() {
        assert!(transient_status(None));
        assert!(transient_status(Some(503)));
        assert!(!transient_status(Some(401)));
        assert!(!transient_status(Some(429)));
        assert!(!transient_status(Some(404)));
    }
}
