#!/usr/bin/env python3
"""Every recovered exFAT file whose name starts with '._', and its first bytes.

Usage: appledouble-recovered.py RUNS_DIR   Written for ADR-0022 Appendix H.
Reads each <image>.txt report (taphonomy --recover --output) in RUNS_DIR,
pairs each deleted set line ("cX sY  NAME  ... passed") with the
"written <path>" line that follows it, and for every recovered file
prints image, entry, name, size, first 4 bytes and a format guess:
AppleDouble (00 05 16 07, RFC 1740), AppleSingle (00 05 16 00), gzip
(1f 8b), zeros, other. Then counts, for '._' names and for all others.
"""
import os, re, sys
from collections import Counter

runs = sys.argv[1]
# A long name leaves one space before the decision, so split on the
# decision's first word, not on a run of spaces.
SET = re.compile(r'^\s+(c\d+ s\d+)\s+(\S.*?)\s+((?:cluster \d+|REFUSED|deleted directory|holds no data|no data|NOT ).*)$')
def kind(b):
    if b[:4] == b'\x00\x05\x16\x07': return 'AppleDouble'
    if b[:4] == b'\x00\x05\x16\x00': return 'AppleSingle'
    if b[:2] == b'\x1f\x8b': return 'gzip'
    if not b.strip(b'\0'): return 'zeros'
    return 'other'
dot, rest = Counter(), Counter()
for f in sorted(os.listdir(runs)):
    if not f.endswith('.txt'): continue
    cur = None
    for line in open(os.path.join(runs, f), errors='replace'):
        m = SET.match(line)
        if m:
            cur = (m[1], m[2].strip()) if 'passed' in m[3] else None
            continue
        if cur and 'written ' in line:
            p = line.split('written ', 1)[1].split()[0]
            data = open(p, 'rb').read()
            k = kind(data[:4096])
            if cur[1].startswith('._'):
                dot[k] += 1
                print(f'{f[:-4]}\t{cur[0]}\t{cur[1]}\t{len(data)}\t{data[:4].hex()}\t{k}')
            else:
                rest[k] += 1
            cur = None
print('names starting ._ :', dict(dot), 'total', sum(dot.values()))
print('other names       :', dict(rest), 'total', sum(rest.values()))
