#!/usr/bin/env python3
"""Read-only scan of the exFAT volume in an image's first MBR or GPT partition.

Written independently of the EXP-0009 scripts and of Taphonomy. Reports every set,
live or deleted, with its timestamps, UTC offsets and allocation, the
allocation bitmap's chain, and the bitmap state of each set's clusters.
Usage: exp-0011-scan.py IMAGE [--deleted-dirs] > table.tsv
  --deleted-dirs also reads a deleted directory whose clusters are all free
  in the bitmap and were not read before, and marks the sets found in it.
"""
import struct, sys

img = open(sys.argv[1], 'rb').read()
if img[450] == 0xEE and img[512:520] == b'EFI PART':
    # Protective MBR: the first GPT entry, at LBA 2.
    part_lba = struct.unpack_from('<Q', img, 1024 + 32)[0]
else:
    part_lba = struct.unpack_from('<I', img, 446 + 8)[0]
base = part_lba * 512
bs = img[base:base + 512]
assert bs[3:11] == b'EXFAT   ', bs[3:11]
fat_off, fat_len, heap_off, ccount, root = struct.unpack_from('<IIIII', bs, 80)
bps = 1 << bs[108]
spc = 1 << bs[109]
cb = bps * spc
nfats = bs[110]

def fat(c):
    return struct.unpack_from('<I', img, base + fat_off * bps + c * 4)[0]

def coff(c):
    return base + (heap_off + (c - 2) * spc) * bps

def chain(first, limit=1 << 20):
    out, c = [], first
    while 2 <= c <= ccount + 1 and len(out) < limit:
        out.append(c)
        c = fat(c)
    return out, c

def ts(v, ms, off):
    sec = (v & 0x1F) * 2; mi = (v >> 5) & 0x3F; h = (v >> 11) & 0x1F
    d = (v >> 16) & 0x1F; mo = (v >> 21) & 0xF; y = 1980 + (v >> 25)
    o = 'valid' if off & 0x80 else 'invalid'
    q = off & 0x7F
    q = q - 128 if q & 0x40 else q
    return f'{y:04d}-{mo:02d}-{d:02d}T{h:02d}:{mi:02d}:{sec:02d}+{ms*10}ms off={o}:{q*15}min'

def cksum(b):
    s = 0
    for i, x in enumerate(b):
        if i in (2, 3):
            continue
        s = (((s & 1) << 15) | (s >> 1)) + x & 0xFFFF
    return s

bitmap = None
sets = []
orphans = []

DESCEND = '--deleted-dirs' in sys.argv
seen = set()
unread_deleted_dirs = []

def dir_clusters(first, data_len, nofat):
    need = -(-data_len // cb)
    if nofat:
        return list(range(first, first + need)), 'contig'
    cl, e = chain(first, need)
    ok = len(cl) == need and fat(cl[-1]) == 0xFFFFFFFF
    return cl, ('EOC' if ok else 'broken')

def walk_dir(first, data_len, nofat, path, is_root, in_deleted=False):
    global bitmap
    if is_root:
        cl, _ = chain(first)
    elif nofat:
        cl = list(range(first, first + -(-data_len // cb)))
    else:
        cl, _ = chain(first, -(-data_len // cb))
    seen.update(cl)
    raw = b''.join(img[coff(c):coff(c) + cb] for c in cl)
    if not is_root:
        raw = raw[:data_len]
    n = len(raw) // 32
    i = 0
    subdirs = []
    while i < n:
        e = raw[i * 32:i * 32 + 32]
        t = e[0]
        if t == 0:
            break
        if t == 0x81:
            bitmap = (struct.unpack_from('<I', e, 20)[0], struct.unpack_from('<Q', e, 24)[0])
        if (t & 0x80) == 0 and (t & 0x40) != 0:
            # An unused secondary outside any set: a remnant whose primary
            # was overwritten.
            orphans.append((path, f'c{cl[i * 32 // cb]} s{(i * 32 % cb) // 32}', t))
        if t in (0x85, 0x05):
            cnt = e[1]
            ent = raw[i * 32:(i + 1 + cnt) * 32]
            restored = bytearray(ent)
            deleted = t == 0x05
            sec_ok = all((ent[k * 32] & 0x7F) in (0x40, 0x41) or (ent[k * 32] & 0x60 == 0x60)
                         for k in range(1, cnt + 1)) and len(ent) == (cnt + 1) * 32
            inuse_states = sorted({ent[k * 32] >> 7 for k in range(1, min(cnt + 1, len(ent) // 32))})
            if deleted:
                for k in range(cnt + 1):
                    if k * 32 < len(restored):
                        restored[k * 32] |= 0x80
            ok = len(ent) == (cnt + 1) * 32 and struct.unpack_from('<H', ent, 2)[0] == cksum(restored)
            st = ent[32:64]
            attrs = struct.unpack_from('<H', e, 4)[0]
            ct, mt, at = struct.unpack_from('<III', e, 8)
            c10, m10, cu, mu, au = e[20], e[21], e[22], e[23], e[24]
            flags = st[1]; nl = st[3]
            vdl = struct.unpack_from('<Q', st, 8)[0]
            fc = struct.unpack_from('<I', st, 20)[0]
            dl = struct.unpack_from('<Q', st, 24)[0]
            name = ''
            for k in range(2, cnt + 1):
                if k * 32 + 32 <= len(ent):
                    name += ent[k * 32 + 2:k * 32 + 32].decode('utf-16le', 'replace')
            name = name[:nl]
            pos = f'c{cl[i * 32 // cb]} s{(i * 32 % cb) // 32}'
            sets.append(dict(path=path, name=name, deleted=deleted, ok=ok, sec_ok=sec_ok,
                             secs_inuse=inuse_states, attrs=attrs, pos=pos,
                             ctime=ts(ct, c10, cu), mtime=ts(mt, m10, mu), atime=ts(at, 0, au),
                             nofat=bool(flags & 2), fc=fc, dl=dl, vdl=vdl,
                             offs=f'{cu:02x}/{mu:02x}/{au:02x}', in_deleted=in_deleted))
            if attrs & 0x10 and not deleted and ok:
                subdirs.append((fc, dl, bool(flags & 2), path + '/' + name, in_deleted))
            elif attrs & 0x10 and deleted and ok and DESCEND:
                # A deleted directory is read only where every cluster is
                # free and never read before: then its entries are its own.
                dcl, end = dir_clusters(fc, dl, bool(flags & 2)) if fc and dl else ([], 'none')
                if end in ('contig', 'EOC') and dcl and all(2 <= c <= ccount + 1 for c in dcl) \
                        and not any(alloc(c) for c in dcl) and seen.isdisjoint(dcl):
                    subdirs.append((fc, dl, bool(flags & 2), path + '/' + name + '[deleted]', True))
                else:
                    unread_deleted_dirs.append(path + '/' + name)
            i += 1 + cnt
            continue
        i += 1
    for s in subdirs:
        walk_dir(s[0], s[1], s[2], s[3], False, s[4])

def alloc(c):
    k = c - 2
    return (bm[k >> 3] >> (k & 7)) & 1

# Pass 1, live only, finds the bitmap; pass 2 reads deleted directories too.
_d, DESCEND = DESCEND, False
walk_dir(root, 0, False, '', True)
bm_first, bm_len = bitmap
bm_chain, bm_end = chain(bm_first)
bm = b''.join(img[coff(c):coff(c) + cb] for c in bm_chain)[:bm_len]
if _d:
    DESCEND = True
    sets.clear(); orphans.clear(); seen.clear()
    walk_dir(root, 0, False, '', True)

print(f'# partition_lba={part_lba} bps={bps} spc={spc} cluster_bytes={cb} clusters={ccount} '
      f'fats={nfats} root={root} bitmap_first={bm_first} bitmap_len={bm_len} '
      f'bitmap_chain={bm_chain} bitmap_chain_end={bm_end:#x}')
for d in unread_deleted_dirs:
    print(f'# deleted_dir_unread path={d}')
for o in orphans:
    print(f'# orphan_secondary path={o[0] or "/"} pos={o[1]} type={o[2]:#04x}')
print('\t'.join(['path', 'name', 'deleted', 'checksum_ok', 'pos', 'ctime', 'mtime', 'atime',
                 'nofat', 'first', 'dl', 'vdl', 'clusters', 'chain_end', 'allocated_clusters',
                 'attrs', 'raw_offsets', 'in_deleted_dir']))
for s in sets:
    need = -(-s['dl'] // cb)
    if s['fc'] == 0 or need == 0:
        cl, end = [], 'none'
    elif s['nofat']:
        cl, end = list(range(s['fc'], s['fc'] + need)), 'contig'
    else:
        cl, e = chain(s['fc'], need)
        end = 'EOC' if len(cl) == need and fat(cl[-1]) == 0xFFFFFFFF else f'{fat(cl[-1]) if cl else e:#x}'
    allocd = [c for c in cl if 2 <= c <= ccount + 1 and alloc(c)]
    runs = []
    for c in cl:
        if runs and runs[-1][1] == c - 1:
            runs[-1][1] = c
        else:
            runs.append([c, c])
    print('\t'.join(map(str, [s['path'] or '/', s['name'], int(s['deleted']), int(s['ok']), s['pos'],
                              s['ctime'], s['mtime'], s['atime'], int(s['nofat']), s['fc'], s['dl'],
                              s['vdl'], ','.join(f'{a}-{b}' for a, b in runs), end,
                              ','.join(map(str, allocd)) if (s['deleted'] or s['in_deleted']) else '-',
                              f"{s['attrs']:#06x}", s['offs'], int(s['in_deleted'])])))
