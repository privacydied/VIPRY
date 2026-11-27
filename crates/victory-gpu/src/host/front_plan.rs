//! A submission's planning of its refreshes in front, made per list on the
//! threads beside the submitting one and joined in order (renderer port #6,
//! `VICTORY_PORT_GUARD_PLAN`, with `VICTORY_PORT_GUARD`; from
//! [`WIDE_STEPS`] steps).
//!
//! One submission a frame has most of the frame's steps, and the submitting
//! thread alone gathered them (a few ms) and made the spans of them. Each
//! list's steps are folded per copy beside it ([`fold_touches`]: the span of
//! its refreshes before its first write by shaders, whether it writes it) and
//! the folds joined in list order ([`join_keys`]), which gives what
//! `front_spans` gives of the whole submission, span for span and in order
//! (`front_tests`).

use super::*;

/// Steps of a submission's lists from which `Batch::refresh_front` makes each
/// list's part beside the submitting thread.
pub(super) const WIDE_STEPS: usize = 4096;

/// The steps of the lists, all together.
pub(super) fn steps(runs: &[&HostList]) -> usize {
    runs.iter().map(|list| list.deferred().copies.len()).sum()
}

/// One list's part of `front_spans` ([`list_front`]).
#[derive(Default)]
struct ListFront {
    /// The copies it touches, in the order of their first touch, each with
    /// the span its refreshes cover before the list's first write of it by
    /// shaders (`usize::MAX, 0`: none) and whether it writes it: what
    /// [`join_fronts`] takes.
    copies: Vec<(usize, usize, usize, bool)>,
    /// A copy the list refreshes, by address.
    shadows: Vec<(usize, std::sync::Arc<crate::shadow::Shadow>)>,
    /// The textures to bring up to date, in order.
    textures: Vec<std::sync::Arc<crate::texture::Staging>>,
}

/// A list's steps as `front_spans` takes them, folded per copy
/// ([`ListFront`]).
fn list_front(steps: &[Step], textures: bool) -> ListFront {
    let mut shadows: Vec<(usize, std::sync::Arc<crate::shadow::Shadow>)> = Vec::new();
    let mut staged: Vec<std::sync::Arc<crate::texture::Staging>> = Vec::new();
    let mut refreshed: ByCopy<()> = ByCopy::default();
    let touches = steps.iter().filter_map(|step| match step {
        Step::Refresh(shadow, from, len) => {
            let key = std::sync::Arc::as_ptr(shadow) as usize;
            if refreshed.insert(key, ()).is_none() {
                shadows.push((key, shadow.clone()));
            }
            Some(Touch::Refresh(key, *from, *from + *len))
        }
        Step::GpuWrite(shadow, ..) => Some(Touch::Write(std::sync::Arc::as_ptr(shadow) as usize)),
        Step::Texture(staging) if textures => {
            staged.push(staging.clone());
            None
        }
        _ => None,
    });
    let copies = fold_touches(touches);
    ListFront { copies, shadows, textures: staged }
}

/// Touches of one list, per copy in the order of their first touch: the span
/// of its refreshes before its first write, and whether it is written.
fn fold_touches(touches: impl Iterator<Item = Touch>) -> Vec<(usize, usize, usize, bool)> {
    let mut at: ByCopy<usize> = ByCopy::default();
    let mut copies: Vec<(usize, usize, usize, bool)> = Vec::new();
    let mut last = usize::MAX;
    for touch in touches {
        let (key, range) = match touch {
            Touch::Refresh(key, from, to) => (key, Some((from, to))),
            Touch::Write(key) => (key, None),
            _ => continue,
        };
        // (Most come several times on end.)
        let i = if copies.get(last).is_some_and(|c| c.0 == key) {
            last
        } else {
            *at.entry(key).or_insert_with(|| {
                copies.push((key, usize::MAX, 0, false));
                copies.len() - 1
            })
        };
        last = i;
        let c = &mut copies[i];
        match range {
            Some((from, to)) if !c.3 => (c.1, c.2) = (c.1.min(from), c.2.max(to)),
            Some(_) => {}
            None => c.3 = true,
        }
    }
    copies
}

/// `front_spans` of the lists whose folds are `lists` ([`fold_touches`]), a
/// new part of the submission beginning at each of `parts`: the same spans,
/// in the same order.
fn join_keys<'a>(lists: impl Iterator<Item = &'a [(usize, usize, usize, bool)]>, parts: &[usize]) -> Vec<(usize, usize, usize)> {
    let mut at: ByCopy<usize> = ByCopy::default();
    let mut spans: Vec<(usize, usize, usize)> = Vec::new();
    let mut given: Vec<u32> = Vec::new();
    let mut part = 1;
    for (n, copies) in lists.enumerate() {
        if parts.contains(&n) {
            part += 1;
        }
        for &(key, from, to, written) in copies {
            let i = *at.entry(key).or_insert_with(|| {
                spans.push((key, usize::MAX, 0));
                given.push(0);
                spans.len() - 1
            });
            // What the list refreshes before it writes the copy counts unless
            // an earlier list of this part gave the copy to shaders.
            if given[i] != part && from < to {
                (spans[i].1, spans[i].2) = (spans[i].1.min(from), spans[i].2.max(to));
            }
            if written {
                given[i] = part;
            }
        }
    }
    spans.retain(|s| s.1 < s.2);
    spans
}

/// [`join_keys`] of the lists' folds, with their copies.
fn join_fronts(fronts: &[ListFront], parts: &[usize]) -> Vec<(std::sync::Arc<crate::shadow::Shadow>, usize, usize)> {
    let mut shadows: ByCopy<std::sync::Arc<crate::shadow::Shadow>> = ByCopy::default();
    for front in fronts {
        for (key, shadow) in &front.shadows {
            shadows.entry(*key).or_insert_with(|| shadow.clone());
        }
    }
    join_keys(fronts.iter().map(|f| f.copies.as_slice()), parts)
        .into_iter()
        .filter_map(|(key, from, to)| Some((shadows.remove(&key)?, from, to)))
        .collect()
}

/// The spans `front_spans` gives of the submission `runs` (a new part
/// beginning at each of `parts`), made list by list on the threads beside
/// this one and joined in order; the textures to bring up to date, those
/// `texture_due` says, go to `due`, in order.
pub(super) fn spans(
    runs: &[&HostList],
    parts: &[usize],
    textures: bool,
    mut texture_due: impl FnMut(&std::sync::Arc<crate::texture::Staging>) -> bool,
    due: &mut Vec<std::sync::Arc<crate::texture::Staging>>,
) -> Vec<(std::sync::Arc<crate::shadow::Shadow>, usize, usize)> {
    #[derive(Clone, Copy)]
    struct Lists(*const *const HostList, usize);
    // SAFETY: the lists outlive the call, which waits for every job.
    unsafe impl Send for Lists {}
    unsafe impl Sync for Lists {}
    let lists: Vec<*const HostList> = runs.iter().map(|&list| std::ptr::from_ref(list)).collect();
    let at = Lists(lists.as_ptr(), lists.len());
    let fronts: std::sync::Arc<Vec<std::sync::Mutex<Option<ListFront>>>> =
        std::sync::Arc::new((0..runs.len()).map(|_| std::sync::Mutex::new(None)).collect());
    let out = fronts.clone();
    crate::shadow::wide_each(runs.len(), move |n| {
        let at = at;
        // SAFETY: as above; a list's deferred steps are behind its lock.
        let list = unsafe { &**at.0.add(n) };
        debug_assert!(n < at.1);
        let front = list_front(&list.deferred().copies, textures);
        *out[n].lock().unwrap_or_else(|e| e.into_inner()) = Some(front);
    });
    drop(lists);
    let fronts: Vec<ListFront> =
        fronts.iter().map(|m| m.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default()).collect();
    for front in &fronts {
        for staging in &front.textures {
            if texture_due(staging) {
                due.push(staging.clone());
            }
        }
    }
    join_fronts(&fronts, parts)
}

#[cfg(test)]
mod front_tests {
    use super::{fold_touches, join_keys};
    use crate::host::{Touch, front_spans};

    /// Random submissions of lists (refreshes and shaders' writes of a few
    /// copies, new parts now and then): each list folded on its own and the
    /// folds joined in order give what the whole submission's touches give,
    /// span for span, in the same order.
    #[test]
    fn lists_folded_apart_and_joined_give_the_submissions_spans() {
        let mut seed = 0x9e37_79b9_u64;
        let mut next = |n: usize| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as usize % n
        };
        for _ in 0..3000 {
            let lists = 1 + next(8);
            let copies = 1 + next(6);
            let mut parts = Vec::new();
            let mut whole: Vec<Touch> = Vec::new();
            let mut folds = Vec::new();
            for n in 0..lists {
                if n > 0 && next(4) == 0 {
                    parts.push(n);
                    whole.push(Touch::Part);
                }
                let mut mine: Vec<Touch> = Vec::new();
                for _ in 0..next(12) {
                    let key = 0x1000 * (1 + next(copies));
                    if next(5) == 0 {
                        mine.push(Touch::Write(key));
                        whole.push(Touch::Write(key));
                    } else {
                        let from = next(100);
                        let to = from + 1 + next(50);
                        mine.push(Touch::Refresh(key, from, to));
                        whole.push(Touch::Refresh(key, from, to));
                    }
                }
                folds.push(fold_touches(mine.into_iter()));
            }
            let want = front_spans(whole.into_iter());
            let got = join_keys(folds.iter().map(|f| f.as_slice()), &parts);
            assert_eq!(got, want, "parts {parts:?}");
        }
    }
}
