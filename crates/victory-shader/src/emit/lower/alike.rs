//! VGPR values that are the same in every lane of a console wave holding them
//! ([`lane_uniform`]): what a `v_readfirstlane` may read in each host wave on its own.

use super::*;

/// VGPR values that are the same in every lane of a console wave holding them (`all` is
/// [`all_lane_blocks`]): written lane by lane from scalars, constants and such values, by an
/// instruction that either every arriving lane runs or that runs under a condition itself the
/// same in every lane of the console wave (then only over an old value that is such a value
/// too: a wave the condition leaves out keeps that); loaded from LDS at such an address; or
/// the thread id's x shifted right past the console wave (`wave_tid`, ids `>> 6` or more,
/// where a console wave is one run of 64 ids along x). The compiler keeps wave-uniform data
/// in VGPRs and fetches it with `v_readfirstlane`.
/// A value joined by a phi counts where every predecessor is a block every lane of the console
/// wave reaches together: no branch on a lane-dependent condition can have split them (a
/// branch on such alike values does not). Loop-carried values count only when what the loop
/// carries is made of such values; the analysis runs to a fixed point from nothing, so a value
/// that needs itself to be alike never is.
pub(super) fn lane_uniform(
    p: &Program,
    st: &Structured,
    all: &[bool],
    ordered_lds: bool,
    unstable: &HashSet<(usize, usize)>,
    wave_tid: Option<Value>,
) -> HashSet<Value> {
    let is_mask = |v: Value| st.masks.contains_key(&v) || st.highs.contains_key(&v);
    let mut same: HashSet<Value> = HashSet::new();
    loop {
        // An atom of a lane condition that every lane of a console wave holds alike.
        let alike_atom = |same: &HashSet<Value>, a: &Atom| match *a {
            Atom::Uniform(_) => true,
            Atom::Lane(v) => same.contains(&v),
            _ => false,
        };
        let alike_bool =
            |same: &HashSet<Value>, b: Bool| st.bools.support(b).iter().all(|a| alike_atom(same, a));
        // Blocks every lane of a console wave reaches together: `all`, or reached only past
        // branches on alike conditions.
        let wave_all: Vec<bool> = match lane_regions(st) {
            Some(regions) => {
                let mut w = vec![true; st.exits.len()];
                for (d, region) in regions {
                    let LaneExit::Branch { when, .. } = st.exits[d] else {
                        continue;
                    };
                    if alike_bool(&same, when) {
                        continue;
                    }
                    for (b, r) in region.iter().enumerate() {
                        if *r {
                            w[b] = false;
                        }
                    }
                }
                w
            }
            None => all.to_vec(),
        };
        let mut next: HashSet<Value> = HashSet::new();
        for b in crate::ir::reverse_postorder(&p.blocks) {
            let block = &p.blocks[b];
            let whole = block.preds.iter().all(|&q| wave_all.get(q).copied().unwrap_or(false));
            for phi in &block.phis {
                if matches!(phi.reg, Reg::V(_))
                    && whole
                    && phi.args.iter().all(|a| next.contains(a) || same.contains(a))
                {
                    next.insert(phi.value);
                }
            }
            let known = |next: &HashSet<Value>, x: &Value| next.contains(x) || same.contains(x);
            for (i, inst) in block.insts.iter().enumerate() {
                let name = inst.name();
                // under a condition: only one every lane of the console wave holds alike, and
                // over an old value that is alike too
                if let Some(g) = st.guards[b][i] {
                    if !alike_bool(&same, g) {
                        continue;
                    }
                    let old_alike = inst
                        .writes
                        .iter()
                        .filter(|w| matches!(w.0, Reg::V(_)))
                        .all(|w| inst.read(w.0).is_none_or(|old| known(&next, &old)));
                    if !old_alike {
                        continue;
                    }
                }
                let ops: Vec<Operand> = match &inst.fields {
                    Fields::Valu(v)
                        if v.dpp.is_none()
                            && v.sdwa.is_none()
                            && !name.starts_with("v_mbcnt")
                            && !name.starts_with("v_cmpx")
                            && !name.contains("lane") =>
                    {
                        v.src.clone()
                    }
                    // an LDS read at an alike address reads one value in every lane
                    Fields::Ds {
                        addr: Some(Operand::Vector { reg, dwords: 1 }),
                        gds: false,
                        ..
                    } if ordered_lds && name == "ds_read_b32" && !unstable.contains(&(b, i)) => {
                        if inst.read(Reg::V(*reg)).is_some_and(|a| known(&next, &a)) {
                            next.extend(inst.writes.iter().map(|w| w.1));
                        }
                        continue;
                    }
                    _ => continue,
                };
                // the thread id's x shifted past the console wave: one value per console wave
                // (an inline constant 6..64: operand codes 134..192)
                if let (Some(tid), "v_lshrrev_b32", [Operand::Const(k), Operand::Vector { reg, .. }, ..]) =
                    (wave_tid, name, ops.as_slice())
                    && (134..=192).contains(k)
                    && inst.read(Reg::V(*reg)) == Some(tid)
                {
                    next.extend(inst.writes.iter().filter(|w| w.0 != Reg::Scc).map(|w| w.1));
                    continue;
                }
                let alike = ops.iter().all(|op| match *op {
                    Operand::Vector { reg, .. } => {
                        // every dword of the operand the instruction reads from that register on
                        let read: Vec<Value> = inst
                            .reads
                            .iter()
                            .filter(|r| matches!(r.0, Reg::V(x) if x >= reg))
                            .map(|r| r.1)
                            .collect();
                        !read.is_empty() && read.iter().all(|x| known(&next, x))
                    }
                    // a scalar is one value; a lane mask is not, unless a compare of such values made it
                    Operand::Scalar { code, .. } if code != NULL => {
                        inst.read(Reg::S(code)).is_none_or(|x| !is_mask(x) || known(&next, &x))
                    }
                    _ => true,
                });
                if alike {
                    next.extend(inst.writes.iter().filter(|w| w.0 != Reg::Scc).map(|w| w.1));
                }
            }
        }
        if next.is_subset(&same) {
            return same;
        }
        same.extend(next);
    }
}

/// The thread id's x (`v0` on entry) of a console wave64 compute program whose console waves
/// are runs of 64 ids along x (a group 64-aligned along x, or one row): `id >> 6` and up is
/// then one value per console wave ([`lane_uniform`]).
pub(super) fn wave_tid(p: &Program, x: &Parts<'_>) -> Option<Value> {
    let StageIo::Cs(cs) = &x.native.io else {
        return None;
    };
    let [tx, ty, tz] = cs.threads;
    if !x.native.wave64 || !(tx % 64 == 0 || ty * tz == 1) {
        return None;
    }
    p.inputs.iter().find(|(r, _)| *r == Reg::V(0)).map(|(_, v)| *v)
}

/// For a test: whether the source of every `v_readfirstlane` of `p` is alike in a console wave
/// ([`lane_uniform`] with the ordered LDS reads and the thread id's x of `v0`, or without
/// the latter when `with_tid` is not set).
#[cfg(test)]
pub(in crate::emit) fn readfirstlane_sources_alike(p: &Program, with_tid: bool) -> bool {
    let st = crate::structure::structure(p).expect("structure");
    let all = all_lane_blocks(&st).expect("all-lane blocks");
    let tid = if with_tid {
        p.inputs.iter().find(|(r, _)| *r == Reg::V(0)).map(|(_, v)| *v)
    } else {
        None
    };
    let same = lane_uniform(p, &st, &all, true, &HashSet::new(), tid);
    let mut seen = 0;
    for inst in p.blocks.iter().flat_map(|b| &b.insts).filter(|i| i.name() == "v_readfirstlane_b32") {
        seen += 1;
        let Some(&(_, v)) = inst.reads.iter().find(|r| matches!(r.0, Reg::V(_))) else {
            return false;
        };
        if !same.contains(&v) {
            return false;
        }
    }
    seen > 0
}
