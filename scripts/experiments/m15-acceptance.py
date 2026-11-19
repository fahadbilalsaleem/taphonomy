#!/usr/bin/env python3
"""M15 acceptance, ADR-0022 section 14 item 3 and Appendix F, for one image.

Usage: m15-acceptance.py SCRIPTS_DIR IMAGE TABLE STDOUT OUTDIR [ORIGINALS_DIR]

TABLE is exp-0011-scan.py --deleted-dirs output for IMAGE; STDOUT is the
binary's stdout for `IMAGE --recover --output OUTDIR`. For every deleted set
the scripts decide (not inside a deleted directory), the binary's decision,
read from its per-set line by position, must equal exp-0011-score-nist.py's
decide(r, 'moec'). Every set the binary recovers must have one file in
OUTDIR whose bytes equal the clusters the scanner lists, up to
ValidDataLength, then zeros to DataLength; and must be right by ground truth:
NIST's block tags (or, for macOS's own files, their format) through the
scorer's truth(), or, with ORIGINALS_DIR, the published original of the same
directory and name. Prints one summary line and every failure; exits 1 on
any failure.
"""
import contextlib, hashlib, io, os, re, runpy, sys

scripts, image, table, stdout, outdir = sys.argv[1:6]
originals_dir = sys.argv[6] if len(sys.argv) > 6 else None

sys.argv = [os.path.join(scripts, 'exp-0011-score-nist.py'), image, table]
with contextlib.redirect_stdout(io.StringIO()):
    g = runpy.run_path(sys.argv[0])
img, part, heap_off, spc, cb = g['img'], g['part'], g['heap_off'], g['spc'], g['cb']

def data_of(r):
    raw = b''.join(img[part + (heap_off + (c - 2) * spc) * 512:
                       part + (heap_off + (c - 2) * spc) * 512 + cb] for c in r['cl'])
    vdl, dl = int(r['vdl']), int(r['dl'])
    return raw[:vdl] + bytes(dl - vdl)

PHRASES = [  # binary text -> scorer class; first match wins, most specific first
    ('conditions 4, 1, 2 and 3 passed', 'recover'),
    ('no content: no cluster and no data', 'nodata'),
    ('deleted directory, not decided', 'directory'),
    ('with no data', 'refuse:c4-first-cluster-no-data'),
    ('REFUSED, condition 4', 'refuse:c4'),
    ('REFUSED, condition 1', 'refuse:c1'),
    ('REFUSED, condition 2: claims not ordered', 'refuse:c2-unordered'),
    ('REFUSED, condition 2', 'refuse:c2'),
    ('REFUSED, condition 3: no valid data', 'refuse:c3-no-valid-data'),
    ('REFUSED, condition 3', 'refuse:c3'),
    ('NOT ASSESSED', 'not-assessed'),
]
binary, in_block = {}, False
for line in open(stdout, encoding='utf-8', errors='replace'):
    line = line.rstrip('\n')
    if line == '    deleted entry sets':
        in_block = True
        continue
    if in_block and not line.startswith('      '):
        in_block = False
    m = re.match(r'^      (c\d+ s\d+) ', line) if in_block else None
    if m:
        cls = next((c for p, c in PHRASES if p in line), 'UNPARSED: ' + line)
        if m.group(1) in binary:
            binary[m.group(1)] = 'DUPLICATE'
        else:
            binary[m.group(1)] = cls

files = {}
for f in os.listdir(outdir):
    m = re.fullmatch(r'p\d+-c(\d+)-s(\d+)-first-(\d+)\.bin', f)
    files[f'c{m.group(1)} s{m.group(2)}' if m else f] = f

originals = {}
if originals_dir:
    for d, _, fs in os.walk(originals_dir):
        for f in fs:
            with open(os.path.join(d, f), 'rb') as fh:
                originals[(os.path.basename(d), f)] = hashlib.sha256(fh.read()).hexdigest()

def right(r, written):
    if originals_dir:
        key = (r['path'].rstrip('/').rsplit('/', 1)[-1], r['name'])
        want = originals.get(key)
        return want is not None and hashlib.sha256(written).hexdigest() == want, want and 'original'
    t = g['truth'](r)
    return t in ('own', 'own~', 'format-own'), t

fail, n, rec, ok_right = [], 0, 0, 0
decided = [r for r in g['dels'] if r['deleted'] == '1' and r.get('in_deleted_dir') != '1']
for r in decided:
    want = g['decide'](r, 'moec')
    if want == 'unusable':
        continue  # the binary records no unusable set; counted in its summary
    n += 1
    got = binary.pop(r['pos'], 'MISSING')
    if got != want:
        fail.append(f"DECISION {r['pos']} {r['path']}/{r['name']}: scripts {want}, binary {got}")
    if got == 'recover':
        rec += 1
        f = files.pop(r['pos'], None)
        if f is None:
            fail.append(f"NO FILE {r['pos']} {r['name']}")
            continue
        written = open(os.path.join(outdir, f), 'rb').read()
        if written != data_of(r):
            fail.append(f"BYTES {r['pos']} {r['name']}: written file is not the scanner's data")
        good, why = right(r, written)
        ok_right += good
        if not good:
            fail.append(f"WRONG {r['pos']} {r['path']}/{r['name']}: truth {why}")
for pos, cls in binary.items():
    fail.append(f"EXTRA binary line {pos}: {cls}")
for pos, f in files.items():
    fail.append(f"EXTRA file {f}")

print(f"{os.path.basename(image)}: {n} sets decided, {n - sum(x.startswith('DECISION') for x in fail)} agree, "
      f"{rec} recovered, {ok_right} right, {len(fail)} failures")
for x in fail:
    print('  ' + x)
sys.exit(1 if fail else 0)
