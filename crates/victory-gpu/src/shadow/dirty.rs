//! A refresh of a host copy that costs what is dirty, not what is asked
//! (`VICTORY_REFRESH_DIRTY=1`, off by default; flippable with `VICTORY_AB`).
//!
//! The submitting thread spends 21-54 ms a frame on ~6,400 refreshes, ~5,200
//! of them known to copy nothing. Each one looked at every page of its range
//! one by one. A copy's pages ([`Pages`]) also keep a bitmask of the pages
//! never copied, so that the pages a refresh takes are found from that
//! bitmask and the sorted list of pages written: a range wholly copied
//! before costs only its written pages. The bitmask is kept up with the
//! switch off too, so the switch may change while the title runs.
//!
//! Hand-ported from mandy f6fa3a4e1, which ports TaynanGT/Vice-Tay db28bb8
//! and c289a95.

use super::PAGE;

/// Whether refreshes plan from the bitmask and the pages written.
pub(super) fn on() -> bool {
    crate::asked_for!("VICTORY_REFRESH_DIRTY")
}

/// Per page of title memory from a copy's start on: the write-watch asking
/// after which it was last copied (0: never). Read as a `[u64]`; written
/// through [`Pages::set`] only, which keeps the bitmask.
#[derive(Clone)]
pub(super) struct Pages {
    pages: Vec<u64>,
    /// Bit `i` (word `i / 64`, bit `i % 64`) is 1 iff `pages[i] == 0`. The
    /// unused high bits of the last word are 0.
    uncopied_bits: Vec<u64>,
    /// How many pages are 0.
    uncopied: usize,
}

impl std::ops::Deref for Pages {
    type Target = [u64];
    fn deref(&self) -> &[u64] {
        &self.pages
    }
}

/// What [`Pages::plan_stale`] plans: the coalesced byte runs
/// `(from, to, known)`, and `(page, taken)` of the pages taken before and
/// written since.
pub(super) type Plan = (Vec<(usize, usize, bool)>, Vec<(usize, u64)>);

impl Pages {
    pub(super) fn new(pages: usize) -> Self {
        let words = pages.div_ceil(64);
        let mut uncopied_bits = vec![u64::MAX; words];
        if pages % 64 != 0 {
            uncopied_bits[words - 1] = (1u64 << (pages % 64)) - 1;
        }
        Self { pages: vec![0; pages], uncopied_bits, uncopied: pages }
    }

    /// Page `index` was copied after asking `now` (0: never). A page that is
    /// not in the copy is left alone.
    pub(super) fn set(&mut self, index: usize, now: u64) {
        let Some(slot) = self.pages.get_mut(index) else {
            return;
        };
        if *slot == 0 && now != 0 {
            self.uncopied -= 1;
            self.uncopied_bits[index / 64] &= !(1u64 << (index % 64));
        } else if *slot != 0 && now == 0 {
            self.uncopied += 1;
            self.uncopied_bits[index / 64] |= 1u64 << (index % 64);
        }
        *slot = now;
    }

    /// Every page set to `now`.
    pub(super) fn fill(&mut self, now: u64) {
        for index in 0..self.pages.len() {
            self.set(index, now);
        }
    }

    /// How many pages were copied at least once.
    #[cfg(feature = "diagnostics")]
    pub(super) fn copied_count(&self) -> usize {
        self.pages.len() - self.uncopied
    }

    /// Whether every page index in `lo..=hi` was copied at least once.
    pub(super) fn all_copied(&self, lo: usize, hi: usize) -> bool {
        if lo > hi || self.uncopied == 0 {
            return true;
        }
        let hi = hi.min(self.pages.len().saturating_sub(1));
        if lo > hi {
            return true;
        }
        let (first_w, last_w) = (lo / 64, hi / 64);
        let lo_mask = u64::MAX << (lo % 64);
        let hi_mask = u64::MAX >> (63 - (hi % 64));
        if first_w == last_w {
            return self.uncopied_bits[first_w] & lo_mask & hi_mask == 0;
        }
        self.uncopied_bits[first_w] & lo_mask == 0
            && self.uncopied_bits[first_w + 1..last_w].iter().all(|&w| w == 0)
            && self.uncopied_bits[last_w] & hi_mask == 0
    }

    /// The lowest page index in `from..=hi` that was never copied.
    fn next_uncopied(&self, from: usize, hi: usize) -> Option<usize> {
        if from > hi || self.uncopied == 0 {
            return None;
        }
        let mut w = from / 64;
        let last_w = hi / 64;
        let mut word = self.uncopied_bits[w] & (u64::MAX << (from % 64));
        loop {
            if w == last_w {
                word &= u64::MAX >> (63 - (hi % 64));
            }
            if word != 0 {
                return Some(w * 64 + word.trailing_zeros() as usize);
            }
            if w == last_w {
                return None;
            }
            w += 1;
            word = self.uncopied_bits[w];
        }
    }

    /// Where the entries of `written` (by page, in order) from page `first`
    /// on begin.
    fn written_start(written: &[(usize, u64)], first: usize) -> usize {
        if written.len() > 8 {
            written.partition_point(|&(p, _)| p < first)
        } else {
            written.iter().take_while(|&&(p, _)| p < first).count()
        }
    }

    /// Of the page indexes `lo..=hi` (`base`'s page is 0), in order, those a
    /// refresh takes from title memory, each with the asking it was taken
    /// after (0: never), to `visit`: the never copied and those `written`
    /// after their copy, found from the bitmask and the sorted `written`,
    /// not by looking at every page. `ranged` (not 0): a range that holds
    /// them was taken whole after that asking, so none is never copied, and
    /// a page's own note counts if it is later.
    fn each_stale(&self, base: usize, lo: usize, hi: usize, ranged: u64, written: &[(usize, u64)], mut visit: impl FnMut(usize, u64)) {
        let last = base + hi;
        let mut wi = Self::written_start(written, base + lo);
        // Every page was copied: only the pages written can be stale.
        if ranged != 0 || self.all_copied(lo, hi) {
            let mut seen = usize::MAX;
            for &(page, write) in &written[wi..] {
                if page > last {
                    break;
                }
                let taken = self.pages[page - base].max(ranged);
                if write > taken && page != seen {
                    seen = page;
                    visit(page - base, taken);
                }
            }
            return;
        }
        // The pages never copied (by the bitmask) merged with the entries of
        // `written` that are dirty, skipping the clean pages between them.
        let mut next_u = self.next_uncopied(lo, hi);
        loop {
            while wi < written.len() && written[wi].0 <= last {
                let (page, write) = written[wi];
                let at = self.pages[page - base];
                if at == 0 || write > at {
                    break;
                }
                wi += 1;
            }
            let next_w = (wi < written.len() && written[wi].0 <= last).then(|| written[wi].0 - base);
            let skip_entries = |wi: &mut usize, idx: usize| {
                while *wi < written.len() && written[*wi].0 == base + idx {
                    *wi += 1;
                }
            };
            let (idx, taken) = match (next_u, next_w) {
                (Some(u), Some(w)) if u < w => {
                    next_u = self.next_uncopied(u + 1, hi);
                    (u, 0)
                }
                (Some(u), Some(w)) if w < u => {
                    skip_entries(&mut wi, w);
                    (w, self.pages[w])
                }
                (Some(u), Some(_)) => {
                    next_u = self.next_uncopied(u + 1, hi);
                    skip_entries(&mut wi, u);
                    (u, 0)
                }
                (Some(u), None) => {
                    next_u = self.next_uncopied(u + 1, hi);
                    (u, 0)
                }
                (None, Some(w)) => {
                    skip_entries(&mut wi, w);
                    (w, self.pages[w])
                }
                (None, None) => break,
            };
            visit(idx, taken);
        }
    }

    /// What the page-by-page walk of `Shadow::copy_stale` decides over
    /// `[from, to)` of a copy of `[start, end)`, without walking the pages:
    /// the coalesced byte runs `(from, to, known)` to take from title
    /// memory, and `(page, taken)` of the pages taken before and written
    /// since. The pages the range holds whole are noted copied at `now`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn plan_stale(&mut self, base: usize, start: usize, end: usize, from: usize, to: usize, now: u64, ranged: u64, written: &[(usize, u64)]) -> Plan {
        let (mut runs, mut again_at) = (Vec::new(), Vec::new());
        if from >= to {
            return (runs, again_at);
        }
        let (lo, hi) = (from / PAGE - base, (to - 1) / PAGE - base);
        let mut stale: Vec<(usize, u64)> = Vec::new();
        self.each_stale(base, lo, hi, ranged, written, |idx, taken| stale.push((idx, taken)));
        let cut = |runs: &mut Vec<(usize, usize, bool)>, (a, b, known): (usize, usize, bool)| {
            runs.push(((a * PAGE).max(from), (b * PAGE).min(to), known));
        };
        let mut run: Option<(usize, usize, bool)> = None;
        for (idx, taken) in stale {
            let page = base + idx;
            // A page only partly in the range is copied in part: it stays
            // stale for the rest.
            if (page * PAGE).max(start) >= from && ((page + 1) * PAGE).min(end) <= to {
                self.set(idx, now);
            }
            // Cut where pages taken before meet pages never taken.
            let again = taken != 0;
            if again {
                again_at.push((page, taken));
            }
            run = match run {
                Some((a, b, known)) if page == b && known == again => Some((a, page + 1, known)),
                Some(done) => {
                    cut(&mut runs, done);
                    Some((page, page + 1, again))
                }
                None => Some((page, page + 1, again)),
            };
        }
        // Cut to the range: next to it shaders may be writing the copy.
        if let Some(done) = run {
            cut(&mut runs, done);
        }
        (runs, again_at)
    }

    /// `stale_pages_in` of `[a, e)` without walking the pages.
    pub(super) fn stale_pages_in(&self, base: usize, a: usize, e: usize, written: &[(usize, u64)]) -> Vec<usize> {
        let mut out = Vec::new();
        if let Some((lo, hi)) = self.span(base, a, e) {
            self.each_stale(base, lo, hi, 0, written, |idx, _| out.push(base + idx));
        }
        out
    }

    /// Whether [`Pages::stale_pages_in`] holds a page, without making the
    /// list.
    pub(super) fn has_stale_in(&self, base: usize, a: usize, e: usize, written: &[(usize, u64)]) -> bool {
        let Some((lo, hi)) = self.span(base, a, e) else {
            return false;
        };
        if !self.all_copied(lo, hi) {
            return true;
        }
        let from = Self::written_start(written, base + lo);
        written[from..].iter().take_while(|&&(page, _)| page <= base + hi).any(|&(page, write)| write > self.pages[page - base])
    }

    /// The page indexes (`base`'s page is 0) of the part of `[a, e)` that is
    /// in the copy.
    fn span(&self, base: usize, a: usize, e: usize) -> Option<(usize, usize)> {
        if a >= e {
            return None;
        }
        let first = (a / PAGE).max(base);
        let last_excl = e.div_ceil(PAGE).min(base.saturating_add(self.pages.len()));
        (first < last_excl).then(|| (first - base, last_excl - 1 - base))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page-by-page walk `Shadow::copy_stale` makes with the switch off.
    fn dense(pages: &mut Pages, base: usize, start: usize, end: usize, from: usize, to: usize, now: u64, ranged: u64, written: &[(usize, u64)]) -> Plan {
        let mut writes = written.iter().peekable();
        let (mut runs, mut run) = (Vec::new(), None::<(usize, bool)>);
        let mut again_at = Vec::new();
        let last = (to - 1) / PAGE;
        for page in from / PAGE..=last {
            let write = match writes.peek() {
                Some(&&(p, w)) if p == page => {
                    writes.next();
                    w
                }
                _ => 0,
            };
            let taken = pages[page - base].max(ranged);
            if taken == 0 || write > taken {
                if (page * PAGE).max(start) >= from && ((page + 1) * PAGE).min(end) <= to {
                    pages.set(page - base, now);
                }
                let again = taken != 0;
                if again {
                    again_at.push((page, taken));
                }
                if let Some((first, known)) = run
                    && known != again
                {
                    runs.push(((first * PAGE).max(from), (page * PAGE).min(to), known));
                    run = None;
                }
                run.get_or_insert((page, again));
            } else if let Some((first, known)) = run.take() {
                runs.push(((first * PAGE).max(from), (page * PAGE).min(to), known));
            }
        }
        if let Some((first, known)) = run {
            runs.push(((first * PAGE).max(from), ((last + 1) * PAGE).min(to), known));
        }
        (runs, again_at)
    }

    #[test]
    fn plans_as_the_walk() {
        // A small random walk over copies of 1..200 pages, seeded.
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n.max(1)
        };
        for _ in 0..400 {
            let count = 1 + next(200) as usize;
            let base = 1000 + next(50) as usize;
            let start = base * PAGE + next(PAGE as u64) as usize;
            let end = ((base + count) * PAGE - next(PAGE as u64) as usize).max(start + 1);
            let (mut sparse, mut walked) = (Pages::new(count), Pages::new(count));
            for round in 1..6u64 {
                let now = round * 10;
                let from = start + next((end - start) as u64) as usize;
                let to = (from + 1 + next((end - from) as u64) as usize).min(end);
                let mut written: Vec<(usize, u64)> =
                    (0..next(count as u64 + 1)).map(|_| (base + next(count as u64) as usize, now - 1 - next(8))).collect();
                written.sort();
                written.dedup_by_key(|w| w.0);
                // (`watch::writes` of `[from, to)` names its pages only.)
                written.retain(|w| (from / PAGE..to.div_ceil(PAGE)).contains(&w.0));
                let ranged = if next(4) == 0 { now - 5 } else { 0 };
                let a = sparse.plan_stale(base, start, end, from, to, now, ranged, &written);
                let b = dense(&mut walked, base, start, end, from, to, now, ranged, &written);
                assert_eq!(a, b);
                assert_eq!(&*sparse, &*walked);
                let (lo, hi) = (from / PAGE - base, (to - 1) / PAGE - base);
                assert_eq!(sparse.all_copied(lo, hi), sparse[lo..=hi].iter().all(|&at| at != 0));
                let slow: Vec<usize> = (from / PAGE..to.div_ceil(PAGE))
                    .filter(|&p| match sparse.get(p - base) {
                        None => false,
                        Some(0) => true,
                        Some(&at) => written.binary_search_by_key(&p, |&(q, _)| q).is_ok_and(|i| written[i].1 > at),
                    })
                    .collect();
                assert_eq!(sparse.stale_pages_in(base, from, to, &written), slow);
                assert_eq!(sparse.has_stale_in(base, from, to, &written), !slow.is_empty());
            }
        }
    }
}
