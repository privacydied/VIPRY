//! Objects let go after their submissions ran, released on a thread of their own
//! (`VICTORY_RELEASE_THREAD=1`, off by default).
//!
//! Releasing a D3D12 resource is a kernel call. [`super::bury`] drops what waited for the
//! submissions that finished on the thread that saw them finish: the submitting or landing
//! thread. Upstream measured about 5 % of the title's submitting thread in such releases. With
//! the switch, what is due goes to a `victory-release` thread instead; nothing in flight uses it.
//!
//! After Marvisak/VIctory 3e10052c, reimplemented here behind a switch (the retired objects
//! only; the submission's own batch keeps its drop).

use std::sync::mpsc::Sender;
use std::sync::OnceLock;

type Due = Box<dyn std::any::Any + Send>;

/// Whether released objects go to the release thread (`VICTORY_RELEASE_THREAD=1`).
pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_RELEASE_THREAD").is_some_and(|v| v == "1"))
}

/// Drops `what` on the release thread. If the thread cannot start, the send fails and `what`
/// is dropped here, as without the switch.
pub(super) fn later(what: Due) {
    static RELEASE: OnceLock<Sender<Due>> = OnceLock::new();
    let release = RELEASE.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Due>();
        let _ = std::thread::Builder::new()
            .name("victory-release".into())
            .spawn(move || rx.into_iter().for_each(drop));
        tx
    });
    let _ = release.send(what);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    /// What goes to the release thread is dropped there, soon.
    #[test]
    fn what_goes_later_is_dropped() {
        let held = Arc::new(());
        super::later(Box::new(Arc::clone(&held)));
        let since = std::time::Instant::now();
        while Arc::strong_count(&held) > 1 && since.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::yield_now();
        }
        assert_eq!(Arc::strong_count(&held), 1);
    }
}
