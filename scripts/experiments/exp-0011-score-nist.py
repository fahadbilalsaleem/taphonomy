#!/usr/bin/env python3
"""Score ADR-0019 section 4, and the rules recommended on 2026-10-20, against
the ground truth NIST's DFR images carry in every block.

NIST's mk-file tags each 512-byte block: "File <name>" for block 0, "Block n
Segment s file <name>" after it, "Tail <name>" for the last; unwritten
sectors read "Empty Sector <n> not used". Truth for a deleted set is read from
the tags of the clusters its metadata records, up to ValidDataLength.

Truth classes: own (blocks 0, 1, 2 ... of the named file in order), own~ (in
order, where NIST tagged a non-ASCII name with an ASCII stand-in), OTHER
(another file's blocks, or out of order), empty (unwritten or zero sectors
only), mixed, nodata. A file with no NIST tag is one of macOS's own, judged
by its format instead: format-own, or FORMAT-OTHER when it holds another
format; untagged where no format is known.

Rules, each a set of flags in VARIANTS:
  adr  ADR-0019 section 4 as written: condition 2 by creation time (UTC by the
       stored offsets), ties refuse.
  m    and a rival last modified at or after this set's creation refuses it.
  o    times compared only where both offset bytes are valid and equal and
       every field is in range; otherwise the pair is unordered and refused.
  e    ValidDataLength 0 with data, and a first cluster with DataLength 0,
       refused.
  u    an orphan secondary, an unreadable deleted directory or an unusable
       deleted set refuses every set on the volume.
  mo, moe, rec   combinations; moe is the rule ADR-0022 adopts.
  moec moe, with deleted directories read for their claims and nothing in
       them recovered (on a --deleted-dirs table): what M15 does.
  adrc adr, the same way.
  lit  m, o, e, and any deleted directory at all also refuses every set:
       ADR-0019 section 4's "only if all of these hold" read literally.
Usage: exp-0011-score-nist.py IMAGE TABLE
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
heap_off = struct.unpack_from('<I', bs, 88)[0]
spc = int(g['spc']); cb = int(g['cluster_bytes'])
orphans = [m for m in meta if m.startswith('# orphan')]
unread_dirs = [m for m in meta if m.startswith('# deleted_dir_unread')]

TS = re.compile(r'(\d{4})-(\d\d)-(\d\d)T(\d\d):(\d\d):(\d\d)\+(\d+)ms off=(\w+):(-?\d+)min')

def key(t):
    """(offset or None, local key or None)."""
    m = TS.fullmatch(t)
    y, mo, d, h, mi, s, ms, valid, off = m.groups()
    y, mo, d, h, mi, s, ms = map(int, (y, mo, d, h, mi, s, ms))
    dim = [31, 29 if y % 4 == 0 and (y % 100 or y % 400 == 0) else 28, 31, 30, 31, 30,
           31, 31, 30, 31, 30, 31]
    ok = 1 <= mo <= 12 and 1 <= d <= dim[mo - 1] and h <= 23 and mi <= 59 and s <= 58 and ms <= 1990
    local = (y, mo, d, h, mi, s * 1000 + ms) if ok else None
    return (int(off) if valid == 'valid' else None), local

def utc(t):
    off, local = key(t)
    if local is None:
        return None
    y, mo, d, h, mi, ms = local
    import datetime
    return datetime.datetime(y, mo, d, h, mi) + datetime.timedelta(milliseconds=ms) - \
        datetime.timedelta(minutes=off or 0)

def clusters(r):
    out = []
    for run in filter(None, r['clusters'].split(',')):
        a, b = map(int, run.split('-')); out += range(a, b + 1)
    return out

# A set inside a deleted directory is deleted whatever its own InUse bit.
dels = [r for r in R if r['deleted'] == '1' or r.get('in_deleted_dir') == '1']
for r in dels:
    r['cl'] = clusters(r)
    r['cs'] = set(r['cl'])

def truth(r):
    """own: blocks 0, 1, 2 ... of one file in order (Tail may end it), and the
    tag names the entry where the entry's name is ASCII. NIST tagged
    non-ASCII names with ASCII stand-ins, so there only the order is checked
    ('own~'). OTHER: a block of another file, or out of order."""
    n = int(r['vdl'])
    data = b''.join(img[part + (heap_off + (c - 2) * spc) * 512:part + (heap_off + (c - 2) * spc) * 512 + cb] for c in r['cl'])[:n]
    if not data:
        return 'nodata'
    tags, empty, untagged = [], 0, 0
    for i in range(0, len(data), 512):
        b = data[i:i + 512]
        m = re.match(rb'\nDFR\n(?:File (.+?) path|Block (\d+) Segment \d+ file (.+?) path|Tail (.+?)\n)', b)
        if m:
            if m.group(1):
                tags.append((0, m.group(1)))
            elif m.group(3):
                tags.append((int(m.group(2)), m.group(3)))
            else:
                tags.append(('tail', m.group(4)))
        elif b.startswith(b'Empty Sector') or b.count(0) == len(b):
            empty += 1
            tags.append(('empty', None))
        else:
            untagged += 1
            tags.append(('untagged', None))
    real = [t for t in tags if t[0] not in ('empty', 'untagged')]
    if not real:
        return by_format(r, data) if untagged else 'empty'
    names = {t[1] for t in real}
    if len(names) > 1 or empty or untagged:
        return 'OTHER' if len(names) > 1 else f'mixed(empty={empty},untagged={untagged})'
    seq = [t[0] for t in tags]
    if seq[-1] == 'tail':
        seq = seq[:-1]
    if seq != list(range(len(seq))):
        return 'OTHER'
    tag = names.pop()
    if all(ord(ch) < 128 for ch in r['name']):
        return 'own' if tag == r['name'].encode() else 'OTHER'
    return 'own~'

def by_format(r, data):
    """Truth for the files NIST did not tag: macOS's own. An AppleDouble
    file (`._*`) begins 00 05 16 07; an fseventsd log, named by 16
    hexadecimal digits, is gzip (1f 8b); `fseventsd-uuid` is a UUID. A
    file holding another format holds another file's data."""
    n = r['name']
    if n.startswith('._'):
        expect, ok = 'AppleDouble', data[:4] == b'\x00\x05\x16\x07'
    elif n == 'fseventsd-uuid':
        expect, ok = 'UUID', re.fullmatch(rb'[0-9A-Fa-f]{8}(-[0-9A-Fa-f]{4}){3}-[0-9A-Fa-f]{12}\x00?', data) is not None
    elif re.fullmatch(r'[0-9a-f]{16}', n):
        expect, ok = 'gzip', data[:2] == b'\x1f\x8b'
    else:
        return 'untagged'
    return 'format-own' if ok else f'FORMAT-OTHER(expected {expect})'

def zeros(r):
    n = int(r['vdl'])
    data = b''.join(img[part + (heap_off + (c - 2) * spc) * 512:part + (heap_off + (c - 2) * spc) * 512 + cb] for c in r['cl'])[:n]
    return data.count(0) == len(data)

unseen = bool(orphans) or bool(unread_dirs) or any(r['checksum_ok'] == '0' for r in dels)

VARIANTS = {
    'adr': set(),
    'm': {'modified'},          # rival's last modification counts too
    'o': {'offsets'},           # order only equal, valid offsets and fields in range
    'u': {'unseen'},            # any unseen claim refuses every set
    'e': {'edges'},             # VDL 0 with data; first cluster with DL 0
    'mo': {'modified', 'offsets'},
    'moe': {'modified', 'offsets', 'edges'},
    'rec': {'modified', 'offsets', 'edges', 'unseen'},
    # The audit's proposal as worded: any deleted directory on the volume,
    # read or not, also counts as an unseen claim.
    'lit': {'modified', 'offsets', 'edges', 'unseen-literal'},
    # moe, with deleted directories read for their claims only: nothing
    # inside a deleted directory is recovered. Meaningful on a table made
    # with --deleted-dirs.
    'moec': {'modified', 'offsets', 'edges', 'claims-only'},
    # ADR-0019 section 4 as written, with deleted directories read for
    # their claims only.
    'adrc': {'claims-only'},
}

def decide(r, rule):
    f = VARIANTS[rule]
    if r['checksum_ok'] == '0':
        return 'unusable'
    if 'claims-only' in f and r.get('in_deleted_dir') == '1':
        return 'inside-deleted-directory'
    dl, vdl, fc = int(r['dl']), int(r['vdl']), int(r['first'])
    if 'edges' in f and fc != 0 and dl == 0:
        return 'refuse:c4-first-cluster-no-data'
    if not r['cl']:
        return 'nodata'
    if int(r['attrs'], 16) & 0x10:
        return 'directory'
    if r['chain_end'] not in ('contig', 'EOC'):
        return 'refuse:c4'
    if r['allocated_clusters']:
        return 'refuse:c1'
    if 'unseen' in f and unseen:
        return 'refuse:c2-unseen'
    if 'unseen-literal' in f and (unseen or any(int(o['attrs'], 16) & 0x10 for o in dels)):
        return 'refuse:c2-unseen'
    mine = r['cs']
    for o in dels:
        if o is r or mine.isdisjoint(o['cs']):
            continue
        times = (o['ctime'], o['mtime']) if 'modified' in f else (o['ctime'],)
        for t in times:
            if 'offsets' in f:
                mk, ok = key(r['ctime']), key(t)
                if mk[0] is None or ok[0] is None or mk[0] != ok[0] or mk[1] is None or ok[1] is None:
                    return 'refuse:c2-unordered'
                if ok[1] >= mk[1]:
                    return 'refuse:c2'
            else:
                a, b = utc(t), utc(r['ctime'])
                if a is None or b is None or a >= b:
                    return 'refuse:c2'
    if 'edges' in f and vdl == 0:
        return 'refuse:c3-no-valid-data'
    if zeros(r):
        return 'refuse:c3'
    return 'recover'

name = sys.argv[1].rsplit('/', 1)[-1]
print(f'== {name}: {len(dels)} deleted sets, orphans {len(orphans)}, unread deleted dirs {len(unread_dirs)}, unseen claims {unseen}')
for r in dels:
    t = truth(r)
    a, b = decide(r, 'adr'), decide(r, 'rec')
    flag = ''
    for rule, d in (('adr', a), ('rec', b)):
        if d == 'recover' and t not in ('own', 'own~', 'format-own', 'untagged', 'nodata'):
            flag += f' WRONG-{rule}'
    print(f"  {r['path']}/{r['name']}\tdl={r['dl']} vdl={r['vdl']} nofat={r['nofat']} "
          f"c={r['ctime']} m={r['mtime']} offs={r['raw_offsets']}\tadr={a}\trec={b}\ttruth={t}{flag}")

from collections import Counter
tally = Counter()
for r in dels:
    t = truth(r)
    # Recovered unwritten sectors are wrong: the file's data never reached
    # them. Only files NIST did not tag are unjudged.
    t = 'right' if t in ('own', 'own~', 'format-own') else ('unjudged' if t in ('untagged', 'nodata') else 'wrong')
    for rule in VARIANTS:
        d = decide(r, rule)
        d = 'recover' if d == 'recover' else 'refuse'
        tally[(rule, d, t)] += 1
for k in sorted(tally):
    print(f'  TALLY {k[0]} {k[1]} {k[2]} {tally[k]}')
