# Landing early: tell the title first, land behind a guard

Status: design only, no code yet. Switch to come: `VICTORY_LAND_EARLY` (off by default).

## Why

On the timeline row L3-tl-plain (frames over 50 ms), landing is 11-33 ms of the
critical chain a frame, and a title thread spends 12.6 ms a frame in short sleeps on
that chain. We believe that thread waits for a fence or a value that landing sets last.
That is not proved yet (see "To measure first").

Today `relay_land` (crates/victory-gpu/src/host.rs) works in this order, on the relay
thread:

1. Copies in the card's memory: the pieces shaders wrote are fetched (one GPU copy into
   a shared buffer, then a wait), then merged into title memory (`land::each`, several
   threads).
2. Copies in the PC's memory: what shaders wrote is found and merged (`hand_back`).
3. `copy_back_done` for every flight: the ranges open again for refreshes.
4. In the line's order: read-backs (`read_backs`: query results, texture read-backs),
   bottom-of-pipe writes (`Late::Write`), fence signals (`Late::Signal`). This is the
   moment the title learns that the card is done.

Title memory is complete before the title is told anything. That is correct, but the
title waits for all of 1 and 2 even when it reads none of those bytes soon.

## The change

With the switch on, a landing splits into two parts:

- **Early (relay thread, before step 1's merge):** decide which title ranges the merges
  will write (the list already exists: `pieces`, and `back` for the PC's copies). Join
  them into runs of whole pages. Close every page of those runs, on every view of it
  (`inplace::pages_of`), with `PAGE_NOACCESS`. Then do step 4: read-backs, writes and
  signals. The title is told now.
- **Late (the same thread, at once):** steps 1 and 2 as today. When a run is merged,
  give back its pages' protection (the protection they had before, through the guards'
  own state, not a fixed value), and wake whoever waits on it. Step 3 comes last, as
  today.

A title thread that touches a closed page faults. The fault handler sees that the page
is in a landing (a bit set of its own, beside `PROTECTED`). It then waits for that run
to be done, on a futex/`WaitOnAddress` word per run, bounded by a timeout that fails
loudly. Then it returns `EXCEPTION_CONTINUE_EXECUTION` so the access runs again. Reads
fault too (NOACCESS), so the title never sees a half-merged page.

## What must stay true

- **No byte read before it lands.** Every page a merge writes is closed before the
  first signal. The set comes from the same piece list the merge uses, so the two
  cannot differ. A piece added later is not possible: the list is fixed before the
  fetch.
- **No title write lost.** A title thread cannot write a closed page. It waits, so the
  merge never races it. The merge rules (`card_merge_relay`: bytes the title changed
  since the snapshot win) stay as they are.
- **Our own threads.** Refreshes, `safecopy` and the write watch may read title memory.
  The ranges stay closed to refreshes until step 3 (`copy_back_done`), as today. Any
  other reader of ours that faults on a closed page must wait like the title, not take
  the `safecopy` failure exit. The fault path checks the landing bit before
  `safecopy::failing_exit`.
- **The guards.** A page can be write-guarded (`PAGE_READONLY`, watch.rs) when it is
  closed. Closing remembers nothing new: opening again asks the guards' state for the
  protection the page should have now. That state can change while the page is closed;
  the guards' code must not reprotect a page that is in a landing. That needs one
  check in `protect_count`, which is the guard path, so agent-2 reviews it first.
- **Views.** A physical page with several views is closed on all of them, or none.
- **Order between landings.** One landing at a time, as today (one relay thread). A
  run closed by landing N is open again before landing N+1 closes anything.

## Cost, and when not to do it

Every close and open is a VirtualProtect, and Wine serializes those with faults,
VirtualQuery and GetWriteWatch on one process-wide lock. Each one costs about 157 us
under the current contention. Two calls per run:

- With R runs a landing, the cost is about 2 x R x 157 us, on the relay thread. It
  also adds to the lock that the title's faults wait on.
- So early landing pays only when R is small. At 12 landings a frame, R = 4 is about
  15 ms a frame of calls, which is already as much as the gain.

Rules:

- A landing goes early only if its runs, after joining pages that are close
  (`VICTORY_LAND_EARLY_GAP_KB`, default 64), are at most `VICTORY_LAND_EARLY_RUNS`
  (default 2). Other landings go the old way.
- The frame line counts landings early and late, runs closed, faults that waited, and
  the ms they waited.

## To measure first (before code)

1. Which part of landing is long: the existing diag line "landing per frame"
   (`victory_gpu::land=info`: GATHER, CARD, BACK, DONE, READ_BACKS, TOLD) splits it. The
   L5-elide rows log that target.
2. How many runs a landing would close at a 64 KiB gap. This is the number that decides
   the cost (see above). It can be counted from the piece list with no protection
   change: first commit, count only.
3. What the sleeping title thread waits for: the fence that `Late::Signal` sets, or a
   value from `Late::Write` or a read-back. If it waits only for the fence of a flight
   whose pieces it does not read, the gain is the whole merge time. If it reads the
   landed bytes at once, it faults and waits for them, and the gain is small.

## Steps

1. This note.
2. Count only: runs per landing, and the ms from "the card is done" to "the title is
   told". No behavior change. Measure.
3. Only if runs are few: the landing bit, closing and opening, the wait in the fault
   handler, the guards' check (reviewed by agent-2). New module
   `crates/victory-gpu/src/land/early.rs`, call lines only in host.rs and watch.rs.
   Unit tests: run joining, the set closed equals the set merged, open again
   restores the guards' protection, and a reader waits and then sees the landed bytes.
4. A/B pairs, two of them, `WINEFSYNC=1 VICTORY_FREE_LOOKS=1`, with only the switch
   different.
