#!/usr/bin/env python3
"""For one image: is the bitmap's chain exactly ceil(DataLength/cluster) long
and ended by end-of-chain; and is every cluster used as structure (root and
live directory chains, bitmap, up-case table) marked in use in the bitmap?
Usage: exp-0011-check-bitmap.py IMAGE"""
import struct, sys
img = open(sys.argv[1], 'rb').read()
if img[450] == 0xEE and img[512:520] == b'EFI PART':
    lba = struct.unpack_from('<Q', img, 1056)[0]
else:
    lba = struct.unpack_from('<I', img, 454)[0]
b = lba * 512
fo, fl, ho, cc, root = struct.unpack_from('<IIIII', img, b + 80)
bps = 1 << img[b + 108]; spc = 1 << img[b + 109]; cb = bps * spc
fat = lambda c: struct.unpack_from('<I', img, b + fo * bps + c * 4)[0]
off = lambda c: b + (ho + (c - 2) * spc) * bps
def chain(c, limit=1 << 20):
    out = []
    while 2 <= c <= cc + 1 and len(out) < limit:
        out.append(c); c = fat(c)
    return out, c
structure = {}
def walk(cl, is_root, dl):
    raw = b''.join(img[off(c):off(c) + cb] for c in cl)
    if not is_root: raw = raw[:dl]
    i = 0; subs = []
    while i < len(raw) // 32:
        e = raw[i * 32:i * 32 + 32]; t = e[0]
        if t == 0: break
        if t in (0x81, 0x82):
            fc, ln = struct.unpack_from('<I', e, 20)[0], struct.unpack_from('<Q', e, 24)[0]
            c2, end = chain(fc); need = -(-ln // cb)
            structure['bitmap' if t == 0x81 else 'upcase'] = (c2[:need], len(c2) == need and end == 0xFFFFFFFF, fc, ln)
        if t == 0x85:
            n = e[1]; st = raw[(i + 1) * 32:(i + 2) * 32]
            if struct.unpack_from('<H', e, 4)[0] & 0x10:
                fc = struct.unpack_from('<I', st, 20)[0]; dl = struct.unpack_from('<Q', st, 24)[0]
                need = -(-dl // cb)
                dcl = list(range(fc, fc + need)) if st[1] & 2 else chain(fc, need)[0]
                subs.append((dcl, dl))
            i += 1 + n; continue
        i += 1
    for s in subs:
        structure.setdefault('dirs', []).extend(s[0]); walk(s[0], False, s[1])
rc, _ = chain(root); structure['root'] = rc; walk(rc, True, 0)
bcl, bok, bfc, bln = structure['bitmap']
bm = b''.join(img[off(c):off(c) + cb] for c in bcl)[:bln]
alloc = lambda c: (bm[(c - 2) >> 3] >> ((c - 2) & 7)) & 1
contig = bcl == list(range(bfc, bfc + len(bcl)))
used = set(rc) | set(structure.get('dirs', [])) | set(bcl) | set(structure['upcase'][0])
free_structure = sorted(c for c in used if not alloc(c))
print(f"{sys.argv[1].rsplit('/',1)[-1]}\tbitmap_chain_ok={bok}\tbitmap_contiguous={contig}\t"
      f"bitmap_clusters={len(bcl)}\tupcase_chain_ok={structure['upcase'][1]}\t"
      f"structure_clusters={len(used)}\tmarked_free={free_structure}")
