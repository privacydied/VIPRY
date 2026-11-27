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
//! On by default at [`DEFAULT_MS`]: with it the list threads stopped waiting
//! the whole budget in vain in every frame, and the median frame fell from
//! 295 to 139 ms on this PC. `=0` turns it off (the wait as before); another
//! number sets the limit. Passes over the screen are not limited: they have
//! their own longer budget.

use std::sync::OnceLock;
use std::time::Duration;

/// The limit per pipeline when the switch is unset.
const DEFAULT_MS: u64 = 500;

/// The limit for the switch's value: unset or not a number is
/// [`DEFAULT_MS`], 0 is off (`None`).
fn limit(value: Option<&str>) -> Option<Duration> {
    let millis = value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_MS);
    (millis > 0).then(|| Duration::from_millis(millis))
}

/// The limit per pipeline; `None`: off.
fn each() -> Option<Duration> {
    static EACH: OnceLock<Option<Duration>> = OnceLock::new();
    *EACH.get_or_init(|| {
        let each = limit(std::env::var("VICTORY_PIPELINE_WAIT_EACH_MS").ok().as_deref());
        match each {
            Some(each) => tracing::info!(
                millis = each.as_millis() as u64,
                "host pipeline wait budget per pipeline (VICTORY_PIPELINE_WAIT_EACH_MS; =0 turns it off)"
            ),
            None => tracing::info!("no host pipeline wait budget per pipeline (VICTORY_PIPELINE_WAIT_EACH_MS=0)"),
        }
        each
    })
}

/// What a pipeline may still be waited for, once `waited` was waited for it over
/// all frames: `Duration::MAX` when off.
pub(super) fn left(waited: Duration) -> Duration {
    left_of(each(), waited)
}

fn left_of(each: Option<Duration>, waited: Duration) -> Duration {
    each.map_or(Duration::MAX, |each| each.saturating_sub(waited))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_is_the_default_and_zero_is_off() {
        assert_eq!(limit(None), Some(Duration::from_millis(DEFAULT_MS)));
        assert_eq!(
            limit(Some("not a number")),
            Some(Duration::from_millis(DEFAULT_MS))
        );
        assert_eq!(limit(Some("0")), None);
        assert_eq!(limit(Some(" 250 ")), Some(Duration::from_millis(250)));
    }

    #[test]
    fn what_is_left_shrinks_to_nothing() {
        let each = Some(Duration::from_millis(500));
        assert_eq!(left_of(each, Duration::ZERO), Duration::from_millis(500));
        assert_eq!(
            left_of(each, Duration::from_millis(300)),
            Duration::from_millis(200)
        );
        assert_eq!(left_of(each, Duration::from_secs(9)), Duration::ZERO);
        assert_eq!(left_of(None, Duration::from_secs(9)), Duration::MAX);
    }
}
