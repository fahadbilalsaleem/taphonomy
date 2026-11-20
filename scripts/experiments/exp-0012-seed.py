#!/usr/bin/env python3
"""Converts a raw image into the sparse input format of EXP-0012's harness
(exp-0012-harness/fuzz_targets/sparse.rs).

Usage: exp-0012-seed.py IMAGE OUT [--recover]

Keeps every sector that is not all zero, grouped into runs of sectors
(gaps of up to GAP zero sectors kept inside a run); the analysis reads
every other sector as zero, as the image does.
Refuses an image longer than the harness's cap rather than seed a
truncated one.
"""
import struct
import sys

SECTOR = 512
MAX_SECTORS = 1 << 18
MAX_RUN = 0xFFFF
# Zero sectors kept inside a run when the gap is at most this long, so a
# structure with zero sectors in it (an exFAT boot region's OEM and
# reserved sectors) stays in one run, where the mutator can reseal it.
GAP = 16


def main():
    if len(sys.argv) not in (3, 4):
        sys.exit("usage: exp-0012-seed.py IMAGE OUT [--recover]")
    image, out = sys.argv[1], sys.argv[2]
    recover = len(sys.argv) == 4 and sys.argv[3] == "--recover"
    if len(sys.argv) == 4 and not recover:
        sys.exit("unknown option " + sys.argv[3])

    with open(image, "rb") as f:
        data = f.read()
    sectors = len(data) // SECTOR
    if sectors > MAX_SECTORS:
        sys.exit(f"{image}: {sectors} sectors exceeds the cap {MAX_SECTORS}")

    zero = bytes(SECTOR)
    body = bytearray()
    kept = 0
    s = 0
    while s < sectors:
        if data[s * SECTOR:(s + 1) * SECTOR] == zero:
            s += 1
            continue
        first = s
        last = s  # the last non-zero sector of this run
        while s < sectors and s - first < MAX_RUN and s - last <= GAP:
            if data[s * SECTOR:(s + 1) * SECTOR] != zero:
                last = s
            s += 1
        s = last + 1
        body += struct.pack("<IH", first, s - first)
        body += data[first * SECTOR:s * SECTOR]
        kept += s - first

    with open(out, "wb") as f:
        f.write(struct.pack("<IB", sectors, 1 if recover else 0))
        f.write(body)
    print(f"{image}\t{sectors}\t{kept}\t{5 + len(body)}")


main()
