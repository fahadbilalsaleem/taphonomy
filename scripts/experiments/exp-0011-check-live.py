#!/usr/bin/env python3
"""For one image: what the bitmap check of ADR-0022 Appendix A looks up,
counted by a walk written apart from Taphonomy's, and whether the bitmap
marks all of it in use.

Walks the root and every live directory (a File set whose SetChecksum is
valid and whose Directory attribute is set). Counts the clusters read as
directories, the bitmap's and the up-case table's chains, and every live
set's allocation: a run where NoFatChain is set, otherwise the chain,
until it has the clusters DataLength needs or leaves the heap. Prints one
line: those counts, ClusterCount, the live clusters per cluster of the
volume, the up-case entries and the table's length and checksum, the
bitmap's length against the length ClusterCount needs, and the clusters
of structure and of live allocations the bitmap marks free.
The volume is found at byte 0, through a GPT, or through the first MBR
entry. The script assumes a well-formed volume, as every one it is run on
is: one bitmap entry and at least one up-case entry in the root (the
first is used), and a root chain without a cycle. It reads the whole
image into memory. Usage: exp-0011-check-live.py IMAGE"""
import struct
import sys

img = open(sys.argv[1], 'rb').read()
if img[3:11] == b'EXFAT   ':
    lba = 0
elif img[450] == 0xEE and img[512:520] == b'EFI PART':
    lba = struct.unpack_from('<Q', img, 1056)[0]
else:
    lba = struct.unpack_from('<I', img, 454)[0]
b = lba * 512
fo, fl, ho, cc, root = struct.unpack_from('<IIIII', img, b + 80)
bps = 1 << img[b + 108]
spc = 1 << img[b + 109]
cb = bps * spc


def fat(c):
    return struct.unpack_from('<I', img, b + fo * bps + c * 4)[0]


def off(c):
    return b + (ho + (c - 2) * spc) * bps


def chain(c, need=None):
    out = []
    while 2 <= c <= cc + 1 and (need is None or len(out) < need):
        out.append(c)
        c = fat(c)
    return out


def checksum(raw):
    s = 0
    for i, x in enumerate(raw):
        if i not in (2, 3):
            s = ((((s & 1) << 15) | (s >> 1)) + x) & 0xFFFF
    return s


upcase, bitmap, dirs, live = [], None, set(), []


def walk(cl, length, is_root):
    global bitmap
    raw = b''.join(img[off(c):off(c) + cb] for c in cl)
    if length is not None:
        raw = raw[:length]
    n, i, subs = len(raw) // 32, 0, []
    while i < n:
        e = raw[i * 32:i * 32 + 32]
        t = e[0]
        if t == 0:
            break
        if is_root and t in (0x81, 0x82):
            v = (struct.unpack_from('<I', e, 20)[0], struct.unpack_from('<Q', e, 24)[0],
                 struct.unpack_from('<I', e, 4)[0])
            if t == 0x81:
                bitmap = v
            else:
                upcase.append(v)
        if t == 0x85:
            k = e[1]
            s = raw[i * 32:(i + 1 + k) * 32]
            if len(s) == (1 + k) * 32 and checksum(s) == struct.unpack_from('<H', e, 2)[0]:
                st = s[32:64]
                fc = struct.unpack_from('<I', st, 20)[0]
                dl = struct.unpack_from('<Q', st, 24)[0]
                need = -(-dl // cb)
                if fc and dl:
                    got = list(range(fc, fc + need)) if st[1] & 2 else chain(fc, need)
                    live.append(got)
                    if struct.unpack_from('<H', e, 4)[0] & 0x10:
                        subs.append((got, dl))
            i += 1 + k
            continue
        i += 1
    for got, dl in subs:
        dirs.update(got)
        walk(got, dl, False)


rc = chain(root)
dirs.update(rc)
walk(rc, None, True)
bfc, bln, _ = bitmap
bcl = chain(bfc, -(-bln // cb))
bm = b''.join(img[off(c):off(c) + cb] for c in bcl)[:bln]


def allocated(c):
    return (bm[(c - 2) >> 3] >> ((c - 2) & 7)) & 1


ufc, uln, usum = upcase[0]
ucl = chain(ufc, -(-uln // cb))
structure = dirs | set(bcl) | set(ucl)
live_clusters = sum(len(got) for got in live)
live_free = sorted({c for got in live for c in got if not allocated(c)})
print(f"{sys.argv[1].rsplit('/', 1)[-1]}\tdirectory_clusters={len(dirs)}"
      f"\tbitmap_clusters={len(bcl)}\tup_case_clusters={len(ucl)}"
      f"\tlive_clusters={live_clusters}\tcluster_count={cc}"
      f"\tlive_per_cluster={live_clusters / cc:.4f}"
      f"\tup_case_entries={len(upcase)}\tup_case_length={uln}"
      f"\tup_case_checksum={usum:#010x}"
      f"\tbitmap_length={bln}\tbitmap_needed={-(-cc // 8)}"
      f"\tstructure_free={sorted(c for c in structure if not allocated(c))}"
      f"\tlive_free={live_free}")
