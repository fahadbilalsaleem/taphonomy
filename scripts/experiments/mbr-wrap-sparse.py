#!/usr/bin/env python3
"""Wraps a volume that has no partition table in an MBR: one partition at
sector 2048, type 0x07 for an exFAT volume (OEM "EXFAT   ") and 0x0C
otherwise, the volume's bytes copied unchanged after it. Zero 1 MiB
blocks are skipped with a seek, so the copy is sparse. Then reads the
copy back and checks that its partition region's SHA-256 equals the
source's. Written for ADR-0024's acceptance, independently of Taphonomy.

Usage: mbr-wrap-sparse.py SOURCE OUT     (refuses an existing OUT; exit 1
on any failure)
"""
import hashlib
import os
import struct
import sys

START = 2048
BLOCK = 1 << 20


def main(src, out):
    if os.path.exists(out):
        sys.exit(f'FAIL {out} exists')
    size = os.path.getsize(src)
    if size == 0 or size % 512:
        sys.exit(f'FAIL {src}: size {size} is not a whole number of sectors')
    with open(src, 'rb') as f:
        boot = f.read(512)
    ptype = 0x07 if boot[3:11] == b'EXFAT   ' else 0x0C
    mbr = bytearray(512)
    struct.pack_into('<I', mbr, 440, 0x5CA1AB1E)
    mbr[446 + 4] = ptype
    struct.pack_into('<II', mbr, 446 + 8, START, size // 512)
    mbr[510:512] = b'\x55\xaa'
    want = hashlib.sha256()
    zero = bytes(BLOCK)
    with open(src, 'rb') as f, open(out, 'xb') as o:
        o.write(mbr)
        o.seek(START * 512)
        while True:
            block = f.read(BLOCK)
            if not block:
                break
            want.update(block)
            if block == zero[:len(block)]:
                o.seek(len(block), os.SEEK_CUR)
            else:
                o.write(block)
        o.truncate(START * 512 + size)
    got = hashlib.sha256()
    with open(out, 'rb') as o:
        o.seek(START * 512)
        while True:
            block = o.read(BLOCK)
            if not block:
                break
            got.update(block)
    if got.hexdigest() != want.hexdigest():
        sys.exit(f'FAIL {out}: partition region differs from {src}')
    print(f'{os.path.basename(src)}\ttype {ptype:#04x}\tsectors {size // 512}\t{want.hexdigest()}')


if len(sys.argv) != 3:
    sys.exit(__doc__)
main(sys.argv[1], sys.argv[2])
