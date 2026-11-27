#!/usr/bin/env python3
"""ab-report: the A/B phases of one run (VICTORY_AB), by switch combination.

    ab-report.py <log> [first second] [last second] [least draws a frame, 3000]

VICTORY_AB=<seconds>,<switch>[,<switch>..] turns the named switches on and off
while the title runs, and logs each phase at its end ("A/B phase ..."); this
needs a diagnostics build (crates/victory-gpu/src/ab.rs). The script keeps the
phases with at least the given draws a frame (in the world, not in menus) that
ended between the given seconds after the log's first timestamp. Per
combination it prints the frames-weighted median and p90; then each switch on
against off over all phases kept (the other switches mixed alike).

After Slaktarn700/SlaktarnVI linux/ab-report.py; here the log's colour codes
are taken out first.
"""
import collections
import datetime
import re
import sys

COLOUR = re.compile(r"\x1b\[[0-9;]*m")
STAMP = re.compile(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?)Z")
PHASE = re.compile(
    r"A/B phase (\d+) bits (\d+) on \[([^\]]*)\]: (\d+) frames, mean ([\d.]+) ms, "
    r"median ([\d.]+), p90 ([\d.]+), (\d+) draws"
)


def phases(lines, lo=0.0, hi=1e9, least=3000):
    """Per combination ("(none)" for all off): (frames, mean, median, p90, second) of each phase kept."""
    start = None
    by = collections.defaultdict(list)
    for line in lines:
        line = COLOUR.sub("", line)
        m = STAMP.match(line)
        if not m:
            continue
        t = datetime.datetime.fromisoformat(m.group(1)[:26])
        if start is None:
            start = t
        p = PHASE.search(line)
        if not p:
            continue
        secs = (t - start).total_seconds()
        _, _, on, frames, mean, median, p90, draws = p.groups()
        if int(frames) == 0 or int(draws) < least or not lo <= secs <= hi:
            continue
        by[on.strip() or "(none)"].append((int(frames), float(mean), float(median), float(p90), secs))
    return by


def weighted(ps, i):
    """The frames-weighted mean of field `i` of the phases `ps`."""
    return sum(p[i] * p[0] for p in ps) / max(1, sum(p[0] for p in ps))


def report(by):
    out = []
    for on, ps in sorted(by.items(), key=lambda kv: weighted(kv[1], 2)):
        med = weighted(ps, 2)
        out.append(
            f"{on:55s} {len(ps)} phases, {sum(p[0] for p in ps)} frames: median {med:.1f} ms "
            f"({1000 / med:.1f} fps), p90 {weighted(ps, 3):.1f}; phases {[p[2] for p in ps]}"
        )
    switches = sorted({s for on in by if on != "(none)" for s in on.split()})
    if switches:
        out.append("\nper switch (frames-weighted median, on against off):")
    for s in switches:
        on = [p for k, ps in by.items() for p in ps if s in k.split()]
        off = [p for k, ps in by.items() for p in ps if s not in k.split()]
        if on and off:
            a, b = weighted(on, 2), weighted(off, 2)
            out.append(f"  {s:32s} on {a:.1f} ms ({len(on)} phases)  off {b:.1f} ms ({len(off)} phases)  on-off {a - b:+.1f} ms")
    return "\n".join(out)


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    lo = float(sys.argv[2]) if len(sys.argv) > 2 else 0.0
    hi = float(sys.argv[3]) if len(sys.argv) > 3 else 1e9
    least = int(sys.argv[4]) if len(sys.argv) > 4 else 3000
    with open(sys.argv[1], errors="replace") as log:
        by = phases(log, lo, hi, least)
    print(report(by) if by else "no A/B phase lines kept (a diagnostics build with VICTORY_AB set writes them)")
