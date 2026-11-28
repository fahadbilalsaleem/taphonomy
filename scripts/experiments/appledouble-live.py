#!/usr/bin/env python3
"""Every live (allocated) file whose name starts with '._' on exFAT images,
read with The Sleuth Kit, and whether it starts with the AppleDouble magic.

Usage: appledouble-live.py IMAGE...   Written for ADR-0022 Appendix H.
For each image: the exFAT volume's sector offset is found from sector 0
(0 for a bare volume; else the first MBR or GPT partition whose first
sector says "EXFAT   "), then `fls -r -p -o OFF` lists files and `icat`
reads each live '._' file's first bytes. Prints one row per file and
totals: AppleDouble (00 05 16 07, RFC 1740), AppleSingle (00 05 16 00),
empty, other.
"""
import re, struct, subprocess, sys
from collections import Counter

def offset(img):
    with open(img, 'rb') as f:
        s0 = f.read(512); s1 = f.read(512)
        if s0[3:11] == b'EXFAT   ':
            return 0
        starts = []
        if s1[:8] == b'EFI PART':
            plba, n, es = struct.unpack_from('<QII', s1, 72)
            f.seek(plba * 512); arr = f.read(min(n, 128) * es)
            starts = [struct.unpack_from('<Q', arr, i * es + 32)[0] for i in range(min(n, 128)) if any(arr[i*es:i*es+16])]
        elif s0[510:512] == b'\x55\xaa':
            starts = [struct.unpack_from('<I', s0, 446 + 16 * i + 8)[0] for i in range(4) if s0[446 + 16 * i + 4]]
        for st in starts:
            f.seek(st * 512)
            if f.read(11)[3:11] == b'EXFAT   ':
                return st
    return None

tot = Counter()
for img in sys.argv[1:]:
    off = offset(img)
    if off is None:
        print(f'{img}\tno exFAT volume found'); continue
    out = subprocess.run(['fls', '-r', '-p', '-o', str(off), img], capture_output=True, text=True).stdout
    for m in re.finditer(r'^(\S)/(\S) (\*? ?)(\d+)(?:\(realloc\))?:\s+(.*)$', out, re.M):
        if m[3].strip() or m[2] != 'r':
            continue                      # deleted, or not a regular file
        name = m[5].strip().split('/')[-1]
        if not name.startswith('._'):
            continue
        b = subprocess.run(['icat', '-o', str(off), img, m[4]], capture_output=True).stdout[:4]
        k = ('AppleDouble' if b == b'\x00\x05\x16\x07' else 'AppleSingle' if b == b'\x00\x05\x16\x00'
             else 'empty' if not b else 'other')
        tot[k] += 1
        print(f'{img.split("/")[-1]}\t{m[5].strip()}\t{b.hex()}\t{k}')
print('live ._ files:', dict(tot), 'total', sum(tot.values()))
