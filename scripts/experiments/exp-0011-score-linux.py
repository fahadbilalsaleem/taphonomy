#!/usr/bin/env python3
"""Compare every deleted file in a scan table of forensics-samples' fs.exfat
(exp-0011-scan.py --deleted-dirs output) with the published original of the
same directory and name, by SHA-256 of the bytes up to ValidDataLength.
Usage: exp-0011-score-linux.py IMAGE TABLE ORIGINALS_DIR
"""
import hashlib, os, struct, sys

img = open(sys.argv[1], 'rb').read()
lines = open(sys.argv[2]).read().splitlines()
g = dict(kv.split('=', 1) for kv in lines[0][2:].split(' ') if '=' in kv)
part = int(g['partition_lba']) * 512
spc = int(g['spc']); cb = int(g['cluster_bytes'])
heap_off = struct.unpack_from('<I', img, part + 88)[0]

originals = {}
for d, _, files in os.walk(sys.argv[3]):
    for f in files:
        with open(os.path.join(d, f), 'rb') as fh:
            originals[(os.path.basename(d), f)] = hashlib.sha256(fh.read()).hexdigest()

rows = [l.split('\t') for l in lines if not l.startswith('#')]
hdr = rows[0]
tally = {}
for r in (dict(zip(hdr, x)) for x in rows[1:]):
    if r['deleted'] != '1' and r['in_deleted_dir'] != '1':
        continue
    if int(r['attrs'], 16) & 0x10:
        kind = 'deleted directory'
    else:
        cl = []
        for run in filter(None, r['clusters'].split(',')):
            a, b = map(int, run.split('-')); cl += range(a, b + 1)
        data = b''.join(img[part + (heap_off + (c - 2) * spc) * 512:
                            part + (heap_off + (c - 2) * spc) * 512 + cb] for c in cl)[:int(r['vdl'])]
        want = originals.get((r['path'].split('/')[-1].replace('[deleted]', ''), r['name']))
        if want is None:
            kind = 'no original'
        elif hashlib.sha256(data).hexdigest() == want:
            kind = 'equal to original'
        else:
            kind = 'DIFFERS from original'
        print(f"{r['path']}/{r['name']}\t{kind}\tnofat={r['nofat']} runs={r['clusters']} "
              f"allocated={r['allocated_clusters'] or 'none'} offsets={r['raw_offsets']}")
    tally[kind] = tally.get(kind, 0) + 1
for k in sorted(tally):
    print(f'TALLY {k}: {tally[k]}')
