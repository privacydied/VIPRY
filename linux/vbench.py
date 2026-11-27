#!/usr/bin/env python3
"""vbench: summarize VIctory frame lines from a log.
usage: vbench.py <log> [min_draws=1000] [skip_seconds=0]"""
import re, statistics as st, sys
from datetime import datetime
log = sys.argv[1]; mind = float(sys.argv[2]) if len(sys.argv) > 2 else 1000; skip = float(sys.argv[3]) if len(sys.argv) > 3 else 0
ansi = re.compile(r'\x1b\[[0-9;]*m')
pat = re.compile(r'^(\S+Z).*?([\d.]+) fps, ([\d.]+) ms/frame \(([\d.]+) preparing: copies ([\d.]+) ms [\d.]+ MB, textures ([\d.]+) ms [\d.]+ MB; ([\d.]+) on the GPU; ([\d.]+) copying back ([\d.]+) MB\), (\d+) draws/frame, dropped (\d+) without a host pipeline \((\d+) being made\)')
rows = []; t0 = None
for line in open(log, errors='replace'):
    line = ansi.sub('', line)
    m = pat.search(line)
    if not m: continue
    t = datetime.fromisoformat(m.group(1).replace('Z', '+00:00')).timestamp()
    t0 = t0 or t
    r = dict(t=t - t0, fps=float(m.group(2)), ms=float(m.group(3)), prep=float(m.group(4)), copies=float(m.group(5)),
             tex=float(m.group(6)), gpu=float(m.group(7)), back=float(m.group(8)), backmb=float(m.group(9)),
             draws=int(m.group(10)), dropped=int(m.group(11)), making=int(m.group(12)))
    rows.append(r)
sel = [r for r in rows if r['draws'] >= mind and r['t'] >= skip]
print(f"frame lines {len(rows)}, selected {len(sel)} (draws>={mind:.0f}, t>={skip:.0f}s)")
if sel:
    for k in ['fps', 'ms', 'prep', 'gpu', 'back', 'copies', 'tex', 'draws', 'dropped', 'making']:
        v = [r[k] for r in sel]
        print(f"  {k:8s} median {st.median(v):8.1f}  p10 {sorted(v)[len(v)//10]:8.1f}  p90 {sorted(v)[len(v)*9//10]:8.1f}")
    worst = max(r['ms'] for r in sel); print(f"  worst 1s window {worst:.1f} ms/frame")
