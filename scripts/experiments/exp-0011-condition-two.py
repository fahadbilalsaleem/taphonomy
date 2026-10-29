#!/usr/bin/env python3
"""Condition 2 for every usable deleted set on one exFAT volume, pair by pair:
ADR-0019 section 4 as its Appendix C and ADR-0022 Appendix C state it.
Written apart from Taphonomy, and from its algorithm: every pair of
claimants sharing a cluster is compared directly.

A set S is refused where another claimant sharing a cluster is an orphan
Stream Extension, or holds a creation or modification key that cannot be
compared with S's creation key (Unordered), or one at or after it (Later).
S's own modification key does not decide S's case. Two keys are compared
only where both offset bytes have OffsetValid set and are the same byte and
every field is in range; then as local times.

Claims: a usable set's allocation, a run where NoFatChain is set, otherwise
the chain from its first cluster until DataLength's clusters or the FAT
leaves the heap. An unused Stream Extension outside any set (an orphan)
claims the same way where its first cluster is in the heap and its
DataLength above 0, a run cut at the heap's end.

Also counts the pairs of claimants sharing a cluster, those whose four keys
do not all carry one valid byte, and the sets that Appendix C's "condition 2
does not hold for either", read literally, would decide otherwise.

Usage: exp-0011-condition-two.py IMAGE TABLE
  TABLE from exp-0011-scan.py without --deleted-dirs. Prints a SET line per
  set (image, cluster/slot of its File entry, verdict) and one TALLY line.
"""
import re, struct, sys

img = open(sys.argv[1], 'rb').read()
lines = open(sys.argv[2]).read().splitlines()
meta = [l for l in lines if l.startswith('#')]
rows = [l.split('\t') for l in lines if not l.startswith('#')]
hdr, rows = rows[0], rows[1:]
R = [dict(zip(hdr, r)) for r in rows]
g = dict(kv.split('=', 1) for kv in meta[0][2:].split(' ') if '=' in kv)
part = int(g['partition_lba']) * 512
bs = img[part:part + 512]
fat_off, fat_len, heap_off, ccount = struct.unpack_from('<IIII', bs, 80)
bps = 1 << bs[108]; spc = 1 << bs[109]; cb = bps * spc
last = ccount + 1

def coff(c):
    return part + (heap_off + (c - 2) * spc) * bps

def fat(c):
    return struct.unpack_from('<I', img, part + fat_off * bps + c * 4)[0]

def claim(first, dl, nofat):
    need = -(-dl // cb)
    if first < 2 or first > last or need == 0:
        return set()
    if nofat:
        return set(range(first, min(first + need, last + 1)))
    out, c = [], first
    while len(out) < need and 2 <= c <= last:
        out.append(c); c = fat(c)
    return set(out)

TS = re.compile(r'(\d{4})-(\d\d)-(\d\d)T(\d\d):(\d\d):(\d\d)\+(\d+)ms off=(\w+):(-?\d+)min')
def key(t):
    """(offset, local time) where the key can be compared, else None."""
    y, mo, d, h, mi, s, ms, valid, off = TS.fullmatch(t).groups()
    y, mo, d, h, mi, s, ms = map(int, (y, mo, d, h, mi, s, ms))
    leap = y % 4 == 0 and (y % 100 != 0 or y % 400 == 0)
    dim = [31, 29 if leap else 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    if valid != 'valid' or not (1 <= mo <= 12 and 1 <= d <= dim[mo - 1] and h <= 23
                                and mi <= 59 and s <= 58 and ms <= 1990):
        return None
    return (int(off), (y, mo, d, h, mi, s * 1000 + ms))

claimants = []
for r in R:
    if r['deleted'] == '1' and r['checksum_ok'] == '1':
        c, s = re.fullmatch(r'c(\d+) s(\d+)', r['pos']).groups()
        claimants.append(dict(pos=f'{c}/{s}', timed=True, c=key(r['ctime']), m=key(r['mtime']),
                              cs=claim(int(r['first']), int(r['dl']), r['nofat'] == '1')))
orphans = 0
for m in meta:
    mm = re.match(r'# orphan_secondary path=\S+ pos=c(\d+) s(\d+) type=0x40$', m)
    if mm:
        c, s = int(mm.group(1)), int(mm.group(2))
        e = img[coff(c) + s * 32:coff(c) + s * 32 + 32]
        if e[0] != 0x40:
            sys.exit(f'orphan at c{c} s{s} is {e[0]:#04x}, not 0x40')
        claimants.append(dict(pos=f'{c}/{s}', timed=False, c=None, m=None,
                              cs=claim(struct.unpack_from('<I', e, 20)[0],
                                       struct.unpack_from('<Q', e, 24)[0], bool(e[1] & 2))))
        orphans += 1

def comparable(a, b):
    return a is not None and b is not None and a[0] == b[0]

name = sys.argv[1].rsplit('/', 1)[-1]
tally = dict(Holds=0, Later=0, Unordered=0)
pairs = mixed = differ = 0
for i, s in enumerate(claimants):
    if s['timed']:
        verdict, literal_unordered = 'Holds', False
    for j, o in enumerate(claimants):
        if i == j or s['cs'].isdisjoint(o['cs']):
            continue
        if i < j:
            pairs += 1
            keys = [s['c'], s['m'], o['c'], o['m']]
            if None in keys or len({k[0] for k in keys}) > 1:
                mixed += 1
        if not s['timed']:
            continue
        for t in ((o['c'], o['m']) if o['timed'] else (None,)):
            if not comparable(s['c'], t):
                verdict = 'Unordered'
            elif t[1] >= s['c'][1] and verdict == 'Holds':
                verdict = 'Later'
        # Read literally, "for either" also refuses S where a pair compared
        # in O's own decision (S's keys against O's creation) cannot be.
        if o['timed'] and not (comparable(o['c'], s['c']) and comparable(o['c'], s['m'])):
            literal_unordered = True
    if s['timed']:
        tally[verdict] += 1
        literal = 'Unordered' if literal_unordered else verdict
        if literal != verdict:
            differ += 1
        print(f"SET\t{name}\t{s['pos']}\t{verdict}")
print(f"TALLY\t{name}\tsets={sum(c['timed'] for c in claimants)}\torphans={orphans}\tpairs={pairs}"
      f"\tmixed={mixed}\tholds={tally['Holds']}\tlater={tally['Later']}"
      f"\tunordered={tally['Unordered']}\tliteral-differs={differ}")
