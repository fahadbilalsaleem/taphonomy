#!/usr/bin/env python3
"""Compare the extent of every set recovered under rule "moe" with the
"Sector List" NIST's layout document gives for the file of that name.

Second method for EXP-0011: the sector list is NIST's record of where each
file's data lies, independent of the block tags exp-0011-score-nist.py
reads. The list comes from exp-0011-pdf-text.py output, which is partial.
A file absent from the extracted list is "unlisted". A difference is
printed, so that it can be read for extraction artefacts.
Usage: exp-0011-sector-list.py TEXT SCORER IMAGE TABLE
"""
import contextlib, io, re, runpy, sys

text, scorer, image, table = sys.argv[1:5]
flat = re.sub(r'\s+', '', open(text, encoding='latin1').read())
starts = [(m.start(), m.group(1)) for m in re.finditer(r'TestImagexfat-(\d\d(?:-[a-z]+)?)DFR', flat)]
sections = {}
for k, (pos, name) in enumerate(starts):
    end = starts[k + 1][0] if k + 1 < len(starts) else pos + 400000
    sections.setdefault(name, flat[pos:end])

key = image.rsplit('/', 1)[-1].replace('dfr-', '').replace('-xfat.dd', '')
sec = sections.get(key, '')
listed = {}
i = sec.find('SectorList')
if i >= 0:
    for m in re.finditer(r'([^,\d][^,]*?\.(?:txt|TXT))((?:\d+-\d+,?)+)', sec[i + len('SectorList'):]):
        name, ranges = m.group(1), m.group(2)
        runs = [tuple(map(int, r.split('-'))) for r in ranges.strip(',').split(',')]
        listed[name] = None if name in listed else runs

sys.argv = ['exp-0011-score-nist.py', image, table]
with contextlib.redirect_stdout(io.StringIO()):
    g = runpy.run_path(scorer)
lba, heap, spc = g['part'] // 512, g['heap_off'], g['spc']

tally = {'match': 0, 'differ': 0, 'unlisted': 0, 'ambiguous': 0}
for r in g['dels']:
    if g['decide'](r, 'moe') != 'recover':
        continue
    sectors = []
    for c in r['cl']:
        s0 = lba + heap + (c - 2) * spc
        sectors += range(s0, s0 + spc)
    sectors = sectors[:-(-int(r['vdl']) // 512)]
    runs = []
    for s in sectors:
        if runs and runs[-1][1] == s - 1:
            runs[-1][1] = s
        else:
            runs.append([s, s])
    runs = [tuple(x) for x in runs]
    want = listed.get(r['name'], 'unlisted')
    if want == 'unlisted':
        tally['unlisted'] += 1
    elif want is None:
        tally['ambiguous'] += 1
    elif want == runs:
        tally['match'] += 1
    else:
        tally['differ'] += 1
        print(f"DIFFER {key} {r['name']} recovered={runs} listed={want}")
print(f"TALLY {key} " + ' '.join(f'{k}={v}' for k, v in tally.items()))
