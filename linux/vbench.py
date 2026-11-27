#!/usr/bin/env python3
"""vbench: summarize VIctory frame lines from a log.
usage: vbench.py <log> [min_draws=1000] [skip_seconds=0]
From the card clock line printed just before each frame line: queued = ms a frame the card
had our work queued (frame ms minus the clock's idle; an upper bound of its busy time),
behind = card idle while the submitting thread worked, untitled = card idle while the title
had not submitted."""
import re, statistics as st, sys
from datetime import datetime
log = sys.argv[1]; mind = float(sys.argv[2]) if len(sys.argv) > 2 else 1000; skip = float(sys.argv[3]) if len(sys.argv) > 3 else 0
ansi = re.compile(r'\x1b\[[0-9;]*m')
pat = re.compile(r'^(\S+Z).*?([\d.]+) fps, ([\d.]+) ms/frame \(([\d.]+) preparing: copies ([\d.]+) ms [\d.]+ MB, textures ([\d.]+) ms [\d.]+ MB; ([\d.]+) on the GPU; ([\d.]+) copying back ([\d.]+) MB\), (\d+) draws/frame, dropped (\d+) without a host pipeline \((\d+) being made\)')
clock = re.compile(r'^(\S+Z).*nothing of ours queued ([\d.]+) ms .*?, ([\d.]+) ms of that while the submitting thread was working')
stamp = lambda s: datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp()
rows = []; t0 = None; last = None
for line in open(log, errors='replace'):
    line = ansi.sub('', line)
    c = clock.search(line)
    if c:
        last = (stamp(c.group(1)), float(c.group(2)), float(c.group(3)))
        continue
    m = pat.search(line)
    if not m: continue
    t = stamp(m.group(1))
    t0 = t0 or t
    r = dict(t=t - t0, fps=float(m.group(2)), ms=float(m.group(3)), prep=float(m.group(4)), copies=float(m.group(5)),
             tex=float(m.group(6)), gpu=float(m.group(7)), back=float(m.group(8)), backmb=float(m.group(9)),
             draws=int(m.group(10)), dropped=int(m.group(11)), making=int(m.group(12)))
    # The clock line of the same frame line comes a few milliseconds before it.
    if last and 0 <= t - last[0] < 0.2:
        r.update(queued=max(r['ms'] - last[1], 0.0), behind=last[2], untitled=max(last[1] - last[2], 0.0))
    rows.append(r)
sel = [r for r in rows if r['draws'] >= mind and r['t'] >= skip]
print(f"frame lines {len(rows)}, selected {len(sel)} (draws>={mind:.0f}, t>={skip:.0f}s)")
if sel:
    for k in ['fps', 'ms', 'prep', 'gpu', 'back', 'copies', 'tex', 'draws', 'dropped', 'making', 'queued', 'behind', 'untitled']:
        v = sorted(r[k] for r in sel if k in r)
        if not v: continue
        print(f"  {k:8s} median {st.median(v):8.1f}  p10 {v[len(v)//10]:8.1f}  p90 {v[len(v)*9//10]:8.1f}")
    worst = max(r['ms'] for r in sel); print(f"  worst 1s window {worst:.1f} ms/frame")
