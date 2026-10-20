#!/usr/bin/env python3
"""Apply ADR-0019 section 4's four conditions to every deleted set in a scan
table (exp-0011-scan.py output) of an EXP-0009 image, and score each against
ground truth: the EXP-0009 generator's content, re-implemented here from its
documented format and checked against EXP-0009's four target digests.

Rules:
  adr-0019  section 4 as written: condition 2 by creation time in UTC.
  c2m       and a rival last modified at or after this set's creation.
  moe       ADR-0019 Appendix C and ADR-0022 Decision F: c2m, with times
            compared only under equal, valid offset bytes and in-range
            fields, and the two shapes of Decision F refused.
Usage: exp-0011-score-exp9.py IMAGE TABLE
"""
import hashlib, re, struct, sys
from datetime import datetime, timedelta

BLOCK = 512

def content(name, size):
    out = bytearray(); i = 0
    while len(out) < size:
        h = f"TAPHONOMY EXP-0009 FILE={name} BLOCK={i:06d}\n".encode()
        pad = (name.encode() + b" ") * BLOCK
        out += h + pad[:BLOCK - len(h)]; i += 1
    return bytes(out[:size])

for n, s, d in [("KEEP.txt", 5000, "799230d4"), ("CONTIG.bin", 262144, "afbc355c"),
                ("SMALL.txt", 1000, "ac61dcd7"), ("FRAG.bin", 245760, "446a6a9c")]:
    assert hashlib.sha256(content(n, s)).hexdigest().startswith(d), n

img = open(sys.argv[1], 'rb').read()
rows = [l.rstrip('\n').split('\t') for l in open(sys.argv[2]) if not l.startswith('#')]
hdr, rows = rows[0], rows[1:]
R = [dict(zip(hdr, r)) for r in rows]
part = struct.unpack_from('<I', img, 454)[0] * 512
bs = img[part:part + 512]
heap_off = struct.unpack_from('<I', bs, 88)[0]
cb = 512 * 8

def when(t):
    stamp, rest = t.split('+', 1)
    ms = int(rest.split('ms')[0]); off = int(rest.split(':')[-1].replace('min', ''))
    return datetime.fromisoformat(stamp) + timedelta(milliseconds=ms) - timedelta(minutes=off)

TS = re.compile(r'(\d{4})-(\d\d)-(\d\d)T(\d\d):(\d\d):(\d\d)\+(\d+)ms off=(\w+):(-?\d+)min')

def key(t):
    """(offset or None, local key or None), as ADR-0019 Appendix C reads them."""
    y, mo, d, h, mi, s, ms, valid, off = TS.fullmatch(t).groups()
    y, mo, d, h, mi, s, ms = map(int, (y, mo, d, h, mi, s, ms))
    dim = [31, 29 if y % 4 == 0 and (y % 100 or y % 400 == 0) else 28, 31, 30, 31, 30,
           31, 31, 30, 31, 30, 31]
    ok = 1 <= mo <= 12 and 1 <= d <= dim[mo - 1] and h <= 23 and mi <= 59 and s <= 58 and ms <= 1990
    return (int(off) if valid == 'valid' else None), ((y, mo, d, h, mi, s * 1000 + ms) if ok else None)

def clusters(r):
    out = []
    for run in filter(None, r['clusters'].split(',')):
        a, b = map(int, run.split('-')); out += range(a, b + 1)
    return out

dels = [r for r in R if r['deleted'] == '1']
for r in dels:
    r['cl'] = clusters(r); r['cs'] = set(r['cl'])
    r['c'] = when(r['ctime']); r['m'] = when(r['mtime'])

def data_of(r):
    return b''.join(img[part + (heap_off + (c - 2) * 8) * 512:part + (heap_off + (c - 2) * 8) * 512 + cb]
                    for c in r['cl'])[:int(r['vdl'])]

def truth(r):
    data = data_of(r)
    if not data:
        return 'nodata'
    if data.count(0) == len(data):
        return 'zeros'
    return 'own' if data == content(r['name'], int(r['vdl'])) else 'other'

def decide(r, rule):
    dl, vdl, fc = int(r['dl']), int(r['vdl']), int(r['first'])
    if rule == 'moe' and fc != 0 and dl == 0:
        return 'refuse:c4'
    if not r['cl']:
        return 'nodata'
    if r['chain_end'] not in ('contig', 'EOC'):
        return 'refuse:c4'
    if r['allocated_clusters']:
        return 'refuse:c1'
    for o in dels:
        if o is r or r['cs'].isdisjoint(o['cs']):
            continue
        if rule == 'adr-0019':
            if o['c'] >= r['c']:
                return 'refuse:c2'
        elif rule == 'c2m':
            if o['c'] >= r['c'] or o['m'] >= r['c']:
                return 'refuse:c2'
        else:
            mine = key(r['ctime'])
            for t in (o['ctime'], o['mtime']):
                theirs = key(t)
                if None in (mine[0], theirs[0], mine[1], theirs[1]) or mine[0] != theirs[0]:
                    return 'refuse:c2'
                if theirs[1] >= mine[1]:
                    return 'refuse:c2'
    if rule == 'moe' and vdl == 0:
        return 'refuse:c3'
    if truth(r) == 'zeros':
        return 'refuse:c3'
    return 'recover'

for rule in ('adr-0019', 'c2m', 'moe'):
    tally = {}
    for r in dels:
        k = (decide(r, rule), truth(r))
        tally[k] = tally.get(k, 0) + 1
    print('rule', rule)
    for k in sorted(tally):
        print(f'  {tally[k]:4d}  decision={k[0]:10s} truth={k[1]}')
    print('  recovered by chain:', [r['name'] for r in dels if decide(r, rule) == 'recover' and r['nofat'] == '0'])
print('unusable deleted sets:', sum(1 for r in dels if r['checksum_ok'] != '1'))
