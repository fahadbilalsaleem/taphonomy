#!/usr/bin/env python3
"""EXP-0012 screen (b): hand-made inputs for canaries no trial triggered.

Usage: exp-0012-screen-b.py SEEDS_DIR OUT_DIR
Decodes a seed (the harness's sparse format) into a raw image, makes one
targeted change per canary, reseals what the change breaks (exFAT boot
checksum, entry-set checksums), and writes RAW images to OUT_DIR as
<canary>.img; inputs are made from them with exp-0012-seed.py
(exp-0012-screen-b.sh runs both).
"""
import os
import struct
import sys

SECTOR = 512


def decode(path):
    data = open(path, 'rb').read()
    sectors = struct.unpack_from('<I', data, 0)[0]
    img = bytearray(sectors * SECTOR)
    at = 5
    while at + 6 <= len(data):
        first, count = struct.unpack_from('<IH', data, at)
        at += 6
        chunk = data[at:at + count * SECTOR]
        at += len(chunk)
        img[first * SECTOR:first * SECTOR + len(chunk)] = chunk
    return img


# ---- exFAT -----------------------------------------------------------------

def exfat(img, base=0):
    b = img[base:base + SECTOR]
    assert b[3:11] == b'EXFAT   ', 'not an exFAT boot sector'
    bps = 1 << b[108]
    spc = 1 << b[109]
    return dict(base=base, bps=bps, cb=bps * spc,
                fat_off=struct.unpack_from('<I', b, 80)[0],
                fat_len=struct.unpack_from('<I', b, 84)[0],
                heap=struct.unpack_from('<I', b, 88)[0],
                count=struct.unpack_from('<I', b, 92)[0],
                root=struct.unpack_from('<I', b, 96)[0])


def reseal_boot(img, g):
    """Recomputes the main boot region's checksum (section 3.4)."""
    bps, base = g['bps'], g['base']
    s = 0
    for i, byte in enumerate(img[base:base + 11 * bps]):
        if i in (106, 107, 112):
            continue
        s = ((s >> 1) | ((s & 1) << 31)) + byte & 0xFFFFFFFF
    sec = base + 11 * bps
    img[sec:sec + bps] = struct.pack('<I', s) * (bps // 4)


def cluster_at(g, c):
    return g['base'] + g['heap'] * g['bps'] + (c - 2) * g['cb']


def fat_at(g, c):
    return g['base'] + g['fat_off'] * g['bps'] + 4 * c


def set_checksum(entries):
    s = 0
    for i, byte in enumerate(b''.join(entries)):
        if i in (2, 3):
            continue
        s = ((s >> 1) | ((s & 1) << 15)) + byte & 0xFFFF
    return s


def file_set(name, first, dl, no_fat_chain):
    name16 = name.encode('utf-16-le')
    f = bytearray(32); f[0] = 0x85; f[1] = 2; struct.pack_into('<H', f, 4, 0x20)
    s = bytearray(32); s[0] = 0xC0; s[1] = 0x01 | (0x02 if no_fat_chain else 0)
    s[3] = len(name) ; struct.pack_into('<Q', s, 8, dl)
    struct.pack_into('<I', s, 20, first); struct.pack_into('<Q', s, 24, dl)
    n = bytearray(32); n[0] = 0xC1; n[2:2 + len(name16)] = name16
    entries = [bytes(f), bytes(s), bytes(n)]
    struct.pack_into('<H', f, 2, set_checksum(entries))
    return [bytes(f), bytes(s), bytes(n)]


def root_free_slot(img, g):
    at = cluster_at(g, g['root'])
    for i in range(g['cb'] // 32):
        if img[at + 32 * i] == 0:
            return at + 32 * i, g['cb'] // 32 - i
    raise SystemExit('root cluster has no end marker')


def build_36(img):
    """TexFAT: a valid boot region declaring two FATs (and room for them)."""
    g = exfat(img)
    img[110] = 2
    # The heap must still start after both FATs (ClusterHeapBeforeFatEnd is
    # checked before TexFat). A TexFAT volume is refused before any walk,
    # so the FAT is moved to the earliest offset section 3.1.6 allows (24)
    # and shrunk to the least length its entries need.
    spc = g['cb'] // g['bps']
    count = g['count']
    while True:
        fat_len = -(-(count + 2) * 4 // g['bps'])
        heap = max(g['heap'], 24 + 2 * fat_len)
        # Moving the heap later takes clusters from the volume's end, so
        # the heap still ends inside the volume.
        new_count = g['count'] - -(-(heap - g['heap']) // spc)
        if new_count == count:
            break
        count = new_count
    struct.pack_into('<I', img, 80, 24)
    struct.pack_into('<I', img, 84, fat_len)
    struct.pack_into('<I', img, 88, heap)
    struct.pack_into('<I', img, 92, count)
    assert g['root'] < count + 2
    reseal_boot(img, g)
    return img


def build_58(img):
    """Five live files, each claiming every cluster of the heap."""
    g = exfat(img)
    at, free = root_free_slot(img, g)
    assert free >= 16, free
    dl = g['count'] * g['cb']
    for k in range(5):
        for e in file_set(f'all{k}', 2, dl, True):
            img[at:at + 32] = e
            at += 32
    return img


def build_92(img):
    """The root's first cluster is full (no end marker) and its FAT link is
    the bad-cluster mark."""
    g = exfat(img)
    at, free = root_free_slot(img, g)
    for i in range(free):
        img[at + 32 * i] = 0x01  # an unused entry: InUse clear, not an end marker
    struct.pack_into('<I', img, fat_at(g, g['root']), 0xFFFFFFF7)
    return img


# ---- FAT32 -----------------------------------------------------------------

def fat32(img):
    if img[510:512] == b'\x55\xaa' and img[446 + 4] in (0x0B, 0x0C):
        base = struct.unpack_from('<I', img, 446 + 8)[0] * SECTOR
    else:
        base = 0
    b = img[base:base + SECTOR]
    bps = struct.unpack_from('<H', b, 11)[0]
    spc = b[13]
    res = struct.unpack_from('<H', b, 14)[0]
    nfat = b[16]
    fsz = struct.unpack_from('<I', b, 36)[0]
    total = struct.unpack_from('<I', b, 32)[0]
    root = struct.unpack_from('<I', b, 44)[0]
    data = res + nfat * fsz
    return dict(base=base, bps=bps, cb=bps * spc, res=res, nfat=nfat, fsz=fsz,
                root=root, data=data, count=(total - data) // spc)


def f32_cluster(g, c):
    return g['base'] + (g['data'] + (c - 2) * (g['cb'] // g['bps'])) * g['bps']


def f32_link(img, g, c, value):
    for k in range(g['nfat']):
        struct.pack_into('<I', img, g['base'] + (g['res'] + k * g['fsz']) * g['bps'] + 4 * c, value)


def root_chain(img, g, n, start):
    """The root becomes root -> start -> start+1 ... (n clusters in all)."""
    chain = [g['root']] + list(range(start, start + n - 1))
    assert chain[-1] < g['count'] + 2, 'chain past the last cluster'
    for a, b in zip(chain, chain[1:]):
        f32_link(img, g, a, b)
    f32_link(img, g, chain[-1], 0x0FFFFFFF)
    return chain


def build_17(img):
    """Root chain one cluster longer than the entry bound allows, with the
    end marker in its first cluster (so entries stay few)."""
    g = fat32(img)
    max_clusters = -(-65536 // (g['cb'] // 32))
    root_chain(img, g, max_clusters + 1, g['count'] // 2)
    return img


def build_18(img):
    """Root chain of exactly the bound's clusters, every slot a deleted
    entry, so the 65,536th entry is reached."""
    g = fat32(img)
    slots = g['cb'] // 32
    max_clusters = -(-65536 // slots)
    chain = root_chain(img, g, max_clusters, g['count'] // 2)
    deleted = b'\xe5' + b'DELETEDTXT' + b'\x20' + bytes(20)
    for c in chain:
        at = f32_cluster(g, c)
        for i in range(slots):
            e = at + 32 * i
            if img[e] == 0 or c != g['root']:
                img[e:e + 32] = deleted
    return img


def main():
    seeds, out = sys.argv[1], sys.argv[2]
    os.makedirs(out, exist_ok=True)
    plan = {
        36: ('relan-tz-512.img.seed', build_36),
        58: ('relan-tz-4096.img.seed', build_58),
        92: ('relan-tz-512.img.seed', build_92),
        17: ('fat32-root-entries.img.seed', build_17),
        18: ('fat32-root-entries.img.seed', build_18),
    }
    for cid, (seed, fn) in plan.items():
        img = fn(decode(os.path.join(seeds, seed)))
        open(os.path.join(out, f'{cid}.img'), 'wb').write(img)
        print(cid, seed, len(img))


if __name__ == '__main__':
    main()
