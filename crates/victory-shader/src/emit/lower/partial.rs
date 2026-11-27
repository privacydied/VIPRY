//! A partial final group (`USER_DISPATCH_THREADS_X`) whose console waves are two host waves:
//! the threads past the count stay for the exchange's group barriers, so they cannot leave at
//! entry. [`Lower::partial_active`] says which threads the console launched; memory, EXEC's
//! ballots and the wave's reductions leave the others out.

use super::*;

impl Lower<'_> {
    /// [`Self::guard`] of a memory access: not by the threads a partial final group only
    /// keeps for its barriers.
    pub(super) fn memory_guard(&mut self, b: usize, i: usize) -> Result<Option<V>, Why> {
        let g = self.guard(b, i)?;
        Ok(match (g, self.partial_active) {
            (Some(g), Some(a)) => Some(self.s.m.bin(self.cur, Bin::And, g, a)),
            (g, a) => g.or(a),
        })
    }

    /// `on` (the lanes of a mask, at `at`) without the threads a partial final group keeps
    /// for its barriers, when the mask is of EXEC's lanes (`Structured::active`).
    pub(super) fn launched(&mut self, at: usize, on: V, mask: Value) -> V {
        match self.partial_active {
            Some(a) if self.x.st.active.contains(&mask) => self.s.m.bin(at, Bin::And, on, a),
            _ => on,
        }
    }

    /// `set` (a condition of the threads at `at`) without the threads the console did not
    /// launch: one of them says nothing.
    pub(super) fn launched_only(&mut self, at: usize, set: V) -> V {
        match self.partial_active {
            Some(a) => self.s.m.bin(at, Bin::And, set, a),
            None => set,
        }
    }

    /// Whether a console wave in two host waves keeps the threads past the count (they stay
    /// for the group barriers) instead of leaving at entry.
    pub(super) fn keeps_partial(&self) -> bool {
        self.halves.is_some() || self.exchange.is_some()
    }
}
