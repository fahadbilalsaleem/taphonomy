#!/usr/bin/env python3
"""Deletes a root directory of an exFAT volume as macOS leaves it, for
the volumes of ADR-0022 Appendix D (exp-0011-deleted-dirs-images.sh).

Usage: exp-0011-mac-delete.py IMAGE NAME

The directory's own set in the root is marked deleted (85h to 05h, C0h to
40h, C1h to 41h; the set checksum covers the entries with InUse set, so it
is unchanged), and every cluster of its tree (its own, its files', its
subdirectories' and theirs) is cleared in the allocation bitmap. The
entries inside it are left as they are, in use. Prints each cluster freed.
Stops on anything it does not expect.
"""
import struct
import sys


def main():
    path, name = sys.argv[1], sys.argv[2]
    img = bytearray(open(path, "rb").read())
    part = struct.unpack_from("<I", img, 446 + 8)[0] * 512
    bs = img[part:part + 512]
    if bs[3:11] != b"EXFAT   ":
        sys.exit("no exFAT volume in the first partition")
    fat_off, fat_len, heap_off, ccount, root = struct.unpack_from("<IIIII", bs, 80)
    bps, spc = 1 << bs[108], 1 << bs[109]
    cb = bps * spc

    def fat(c):
        return struct.unpack_from("<I", img, part + fat_off * bps + 4 * c)[0]

    def coff(c):
        return part + (heap_off + (c - 2) * spc) * bps

    def clusters(first, length, nofat, whole_chain=False):
        if first == 0:
            return []
        need = -(-length // cb)
        if nofat:
            return list(range(first, first + need))
        out, c = [], first
        while 2 <= c <= ccount + 1 and (whole_chain or len(out) < need):
            out.append(c)
            c = fat(c)
        return out

    def entries(cl, length=None):
        raw = b"".join(bytes(img[coff(c):coff(c) + cb]) for c in cl)
        if length is not None:
            raw = raw[:length]
        return [(cl[i * 32 // cb], (i * 32) % cb, raw[i * 32:i * 32 + 32]) for i in range(len(raw) // 32)]

    def sets(ents):
        i = 0
        while i < len(ents):
            t = ents[i][2][0]
            if t == 0:
                return
            if t in (0x85, 0x05):
                n = ents[i][2][1]
                group = ents[i:i + n + 1]
                st = group[1][2]
                nl = st[3]
                nm = b"".join(e[2][2:32] for e in group[2:]).decode("utf-16le")[:nl]
                yield dict(name=nm, entries=group,
                           attrs=struct.unpack_from("<H", group[0][2], 4)[0],
                           nofat=bool(st[1] & 2),
                           first=struct.unpack_from("<I", st, 20)[0],
                           length=struct.unpack_from("<Q", st, 24)[0], deleted=t == 0x05)
                i += n + 1
                continue
            i += 1

    root_ents = entries(clusters(root, 0, False, whole_chain=True))
    bitmap = next(e for _, _, e in root_ents if e[0] == 0x81)
    bm_first, bm_len = struct.unpack_from("<I", bitmap, 20)[0], struct.unpack_from("<Q", bitmap, 24)[0]
    bm_cl = clusters(bm_first, bm_len, False)

    def clear(c):
        k = c - 2
        byte = k >> 3
        at = coff(bm_cl[byte // cb]) + byte % cb
        if not img[at] & (1 << (k & 7)):
            sys.exit(f"cluster {c} already free")
        img[at] &= ~(1 << (k & 7)) & 0xFF
        print(f"FREED\t{c}")

    target = [s for s in sets(root_ents) if s["name"] == name and not s["deleted"]]
    if len(target) != 1 or not target[0]["attrs"] & 0x10:
        sys.exit(f"{name}: not exactly one live directory in the root")
    d = target[0]

    def tree(s):
        cl = clusters(s["first"], s["length"], s["nofat"])
        yield from cl
        if s["attrs"] & 0x10:
            for inner in sets(entries(cl, s["length"])):
                if not inner["deleted"]:
                    yield from tree(inner)

    for c in tree(d):
        clear(c)
    for clus, off, e in d["entries"]:
        at = coff(clus) + off
        img[at] &= 0x7F
    print(f"MARKED\t{name}\t{len(d['entries'])} entries")
    open(path, "wb").write(img)


main()
