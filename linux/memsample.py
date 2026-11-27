#!/usr/bin/env python3
"""Sample the VIctory process's memory every 15 s: smaps by kind + meminfo. Usage: memsample.py <out.jsonl> [secs]"""
import json, re, subprocess, sys, time
out, secs = sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 900
t0 = time.time()
def pid():
    r = subprocess.run(['pgrep', '-f', r'victory\.exe run'], capture_output=True, text=True).stdout.split()
    # the Wine process whose comm is main/victory: pick largest RSS
    best = None
    for p in r:
        try:
            rss = int(re.search(r'VmRSS:\s+(\d+)', open(f'/proc/{p}/status').read()).group(1))
        except Exception:
            continue
        if best is None or rss > best[1]:
            best = (p, rss)
    return best[0] if best else None
while time.time() - t0 < secs:
    p = pid()
    rec = {'t': round(time.time() - t0)}
    if p:
        agg = {}
        cur = None
        try:
            for line in open(f'/proc/{p}/smaps'):
                if re.match(r'^[0-9a-f]+-[0-9a-f]+ ', line):
                    parts = line.split(None, 5)
                    name = parts[5].strip() if len(parts) > 5 else '[anon]'
                    k = 'memfd' if 'memfd' in name else 'nvidia' if 'nvidia' in name else 'anon' if name in ('[anon]', '[heap]') else 'file'
                    cur = agg.setdefault(k, {'rss': 0, 'swap': 0, 'pss': 0})
                else:
                    k, _, v = line.partition(':')
                    if k == 'Rss': cur['rss'] += int(v.split()[0]) >> 10
                    elif k == 'Swap': cur['swap'] += int(v.split()[0]) >> 10
                    elif k == 'Pss': cur['pss'] += int(v.split()[0]) >> 10
        except Exception as e:
            rec['err'] = str(e)
        rec['pid'] = p
        rec['kinds'] = agg
    mi = dict((l.split(':')[0], int(l.split()[1]) >> 10) for l in open('/proc/meminfo'))
    rec['avail'] = mi['MemAvailable']; rec['swapused'] = mi['SwapTotal'] - mi['SwapFree']
    with open(out, 'a') as f:
        f.write(json.dumps(rec) + '\n')
    time.sleep(15)
