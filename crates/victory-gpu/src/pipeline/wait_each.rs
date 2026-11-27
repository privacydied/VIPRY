//! `VICTORY_PIPELINE_WAIT_EACH_MS=<n>`: a first use waits for one pipeline
//! at most `n` ms over all frames, not again in every frame.
//!
//! Why: the frame's budget ([`super::make`], half a second) is given anew
//! in each frame. A pipeline that is never made (on this PC seven compute
//! pipelines whose workers die of a stack overflow) or whose build outlasts
//! the budget takes what is left of it in every frame that binds it. Past
//! this budget a draw with it is dropped at once, as one past the frame's
//! budget, and finds the pipeline when a worker has made it. The same limit
//! is in another fork of the same base (fxceless/victory-fork#1181).
//!
//! Off by default (unset or 0). Passes over the screen are not limited:
//! they have their own longer budget.

use std::sync::OnceLock;
use std::time::Duration;

/// The limit per pipeline; `None`: off.
fn each() -> Option<Duration> {
    static EACH: OnceLock<Option<Duration>> = OnceLock::new();
    *EACH.get_or_init(|| {
        let millis: u64 = std::env::var("VICTORY_PIPELINE_WAIT_EACH_MS")
            .ok()?
            .trim()
            .parse()
            .ok()?;
        (millis > 0).then(|| {
            tracing::info!(
                millis,
                "host pipeline wait budget per pipeline (VICTORY_PIPELINE_WAIT_EACH_MS)"
            );
            Duration::from_millis(millis)
        })
    })
}

/// What a pipeline may still be waited for, once `waited` was waited for it over
/// all frames: `Duration::MAX` when off.
pub(super) fn left(waited: Duration) -> Duration {
    each().map_or(Duration::MAX, |each| each.saturating_sub(waited))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_by_default_leaves_no_limit() {
        if std::env::var_os("VICTORY_PIPELINE_WAIT_EACH_MS").is_none() {
            assert_eq!(each(), None);
            assert_eq!(left(Duration::from_secs(60)), Duration::MAX);
        }
    }
}
