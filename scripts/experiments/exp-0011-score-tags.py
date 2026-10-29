#!/usr/bin/env python3
"""Scores the volumes exp-0011-claims-images.sh makes against the tags their
writer put in every 512-byte block of every file: "TAG <path> <block> ",
padded with '.'.

Deleted sets failing their checksum are counted and not scored.
For each deleted set, truth is 'own' when every block up to ValidDataLength
holds its own tag, in order, and 'OTHER' otherwise. Each set is decided in
ADR-0022 section 6's order (conditions 4, 1, 2, 3) under four rules:
  decided           ADR-0019 Appendix C as ADR-0022 Appendix C states it:
                    a set is judged by its rivals' keys against its own
                    creation key; orphan Stream Extensions claim, with no
                    time; keys compared only under the same valid offset
  literal           the same, but an unorderable pair refuses both sets, as
                    Appendix C's "for either" reads literally
  no-orphan-claims  decided, with orphan Stream Extensions claiming nothing
  valid-offsets     decided, with every offset byte taken as valid and the
                    same: what the files would get had their writer
                    recorded an offset
Usage: exp-0011-score-tags.py IMAGE TABLE   (TABLE from exp-0011-scan.py)
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
def key(t, force_valid):
    y, mo, d, h, mi, s, ms, valid, off = TS.fullmatch(t).groups()
    y, mo, d, h, mi, s, ms = map(int, (y, mo, d, h, mi, s, ms))
    if valid != 'valid' and not force_valid:
        return None
    leap = y % 4 == 0 and (y % 100 != 0 or y % 400 == 0)
    dim = [31, 29 if leap else 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    if not (1 <= mo <= 12 and 1 <= d <= dim[mo - 1] and h <= 23 and mi <= 59
            and s <= 58 and ms <= 1990):
        return None
    return (0 if force_valid else int(off), (y, mo, d, h, mi, s * 1000 + ms))

def truth(path, name, vdl, cl):
    tag_path = (path.lstrip('/') + '/' if path != '/' else '') + name
    data = b''.join(img[coff(c):coff(c) + cb] for c in cl)[:vdl]
    for b in range(-(-vdl // 512)):
        if not data[b * 512:].startswith(f'TAG {tag_path} {b} '.encode()):
            return 'OTHER'
    return 'own'

def runs_to_list(s):
    out = []
    for run in filter(None, s.split(',')):
        a, b = map(int, run.split('-')); out += range(a, b + 1)
    return out

orphans = []
for m in meta:
    mm = re.match(r'# orphan_secondary path=(\S+) pos=c(\d+) s(\d+) type=0x40$', m)
    if mm:
        c, s = int(mm.group(2)), int(mm.group(3))
        e = img[coff(c) + s * 32:coff(c) + s * 32 + 32]
        if e[0] != 0x40:
            sys.exit(f'orphan at c{c} s{s} is {e[0]:#04x}, not 0x40')
        orphans.append(claim(struct.unpack_from('<I', e, 20)[0],
                             struct.unpack_from('<Q', e, 24)[0], bool(e[1] & 2)))

dels = [r for r in R if r['deleted'] == '1' and r['checksum_ok'] == '1']
# Sets failing their checksum are not usable and are not scored: counted,
# so that none is dropped unseen.
failing = sum(1 for r in R if r['deleted'] == '1' and r['checksum_ok'] != '1')
for r in dels:
    r['cl'] = runs_to_list(r['clusters'])
    r['cs'] = claim(int(r['first']), int(r['dl']), r['nofat'] == '1')
    r['truth'] = truth(r['path'], r['name'], int(r['vdl']), r['cl']) if r['cl'] else 'nodata'

def decide(r, rule):
    if not r['cl']:
        return 'nodata'
    if r['chain_end'] not in ('contig', 'EOC'):
        return 'refuse:c4'
    if r['allocated_clusters']:
        return 'refuse:c1'
    fv = rule == 'valid-offsets'
    sc, sm = key(r['ctime'], fv), key(r['mtime'], fv)
    for o in dels:
        if o is r or r['cs'].isdisjoint(o['cs']):
            continue
        oc, om = key(o['ctime'], fv), key(o['mtime'], fv)
        if rule == 'literal':
            ks = [sc, sm, oc, om]
            if None in ks or len({k[0] for k in ks}) > 1:
                return 'refuse:c2-unordered'
        for t in (oc, om):
            if t is None or sc is None or t[0] != sc[0]:
                return 'refuse:c2-unordered'
        if oc[1] >= sc[1] or om[1] >= sc[1]:
            return 'refuse:c2-later'
    if rule != 'no-orphan-claims':
        if any(not r['cs'].isdisjoint(o) for o in orphans):
            return 'refuse:c2-orphan'
    if int(r['vdl']) == 0:
        return 'refuse:c3-no-valid-data'
    return 'recover'

name = sys.argv[1].rsplit('/', 1)[-1]
print(f'== {name}: {len(dels)} deleted sets, {failing} failing their checksum,'
      f' {len(orphans)} orphan streams')
for rule in ('decided', 'literal', 'no-orphan-claims', 'valid-offsets'):
    right = wrong = own_refused = 0
    for r in dels:
        d = decide(r, rule)
        if d == 'recover':
            right += r['truth'] == 'own'
            wrong += r['truth'] != 'own'
        elif r['truth'] == 'own':
            own_refused += 1
        print(f"SET\t{name}\t{rule}\t{r['path']}\t{r['name']}\ttruth={r['truth']}\t{d}")
    print(f'TALLY\t{name}\t{rule}\trecovered-right={right}\trecovered-wrong={wrong}'
          f'\town-but-refused={own_refused}')
