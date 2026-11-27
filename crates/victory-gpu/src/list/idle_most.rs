//! How many host lists that have run are kept idle for the next recording.

/// The most host lists kept idle, and the most upload memory they keep
/// between them. 512 lists and 256 MB unless `VICTORY_LIST_IDLE_MOST` names
/// another count (0 keeps none, as `VICTORY_LIST_POOL=0` but with the
/// resets). An idle list keeps its command allocator, and an allocator keeps
/// the memory of the biggest recording it ever held: on a PC with less
/// memory than the console's, fewer idle lists may leave the title's own
/// pages out of swap. Fewer than 128 cost 12 % of the submitting thread in
/// the opening upstream (lists thrown away and made again in one frame).
pub(super) fn most() -> (usize, u64) {
    static MOST: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let lists = *MOST.get_or_init(|| count(std::env::var("VICTORY_LIST_IDLE_MOST").ok().as_deref()));
    (lists, 256 << 20)
}

/// The count for a setting: a number, or 512.
fn count(setting: Option<&str>) -> usize {
    setting.and_then(|v| v.trim().parse().ok()).unwrap_or(512)
}

#[cfg(test)]
mod tests {
    use super::count;

    #[test]
    fn unset_or_bad_is_512() {
        assert_eq!(count(None), 512);
        assert_eq!(count(Some("x")), 512);
        assert_eq!(count(Some(" 64 ")), 64);
        assert_eq!(count(Some("0")), 0);
    }
}
