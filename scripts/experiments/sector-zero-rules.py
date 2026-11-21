#!/usr/bin/env python3
"""How four rules classify sector 0 of each image: partition table, or the
boot record of a volume written at sector 0 with no partition table.

Usage: sector-zero-rules.py IMAGE...   one TSV row per image, then a
summary line per rule. Written for ADR-0024, independently of Taphonomy.

Facts read from sector 0: the 0x55AA signature; the four MBR entries'
boot indicators, types and sector counts (an entry is used when its type
and its sector count are both non-zero); whether the sector is a FAT boot
sector (a jump instruction, 512 to 4096 bytes per sector, a power-of-two
cluster size, reserved sectors, one or two FATs and a valid media byte),
an exFAT one ("EXFAT   " at offset 3) or an NTFS one ("NTFS    ").
Rules, as their sources state them (read 2026-11-21):
  kernel   Linux block/partitions/msdos.c: a boot indicator other than 0
           or 0x80 means no table, and slot 1 is then a FAT volume if
           reserved sectors, FAT count and media are valid.
  blkid    util-linux libblkid partitions/dos.c: a boot indicator other
           than 0 or 0x80 means no table ("nothing"); else a vfat, exFAT
           or NTFS boot sector means no table ("VOLUME").
  tsk      The Sleuth Kit tsk/vs/dos.c (test mode): an OEM name MSDOS,
           MSWIN, NTFS or FAT means no table.
  decided  ADR-0024: blkid's order, with a boot record found after a bad
           boot indicator taken as a volume, and a boot record beside a
           used entry taken as ambiguous.
"""
import struct
import sys

FAT_MEDIA = {0xF0} | set(range(0xF8, 0x100))


def facts(path):
    with open(path, 'rb') as f:
        s = f.read(512)
    sig = len(s) == 512 and s[510:512] == b'\x55\xaa'
    entries = [s[446 + 16 * i:462 + 16 * i] for i in range(4)] if sig else []
    used = sum(1 for e in entries if e[4] != 0 and struct.unpack_from('<I', e, 12)[0] != 0)
    bps = struct.unpack_from('<H', s, 11)[0] if len(s) >= 13 else 0
    spc = s[13] if len(s) > 13 else 0
    reserved = struct.unpack_from('<H', s, 14)[0] if len(s) >= 16 else 0
    fats = s[16] if len(s) > 16 else 0
    media = s[21] if len(s) > 21 else 0
    jump = len(s) > 2 and (s[0] == 0xE9 or (s[0] == 0xEB and s[2] == 0x90))
    fat = (jump and bps in (512, 1024, 2048, 4096) and spc in (1, 2, 4, 8, 16, 32, 64, 128)
           and reserved > 0 and fats in (1, 2) and media in FAT_MEDIA)
    exfat = s[3:11] == b'EXFAT   '
    ntfs = s[3:11] == b'NTFS    '
    return dict(sig=sig, boot=[e[0] for e in entries], types=[e[4] for e in entries],
                used=used, fat=fat, exfat=exfat, ntfs=ntfs, oem=s[3:11],
                reserved=reserved, fats=fats, media=media)


def rules(x):
    if not x['sig']:
        return dict(kernel='no-sig', blkid='no-sig', tsk='no-sig', decided='no-sig')
    bad = any(b not in (0, 0x80) for b in x['boot'])
    record = x['fat'] or x['exfat'] or x['ntfs']
    if bad:
        kernel = 'VOLUME' if (x['reserved'] and x['fats'] and x['media'] in FAT_MEDIA) else 'nothing'
        blkid = 'nothing'
        decided = 'VOLUME' if record else 'neither'
    else:
        kernel = 'TABLE'
        blkid = 'VOLUME' if record else 'TABLE'
        decided = ('ambiguous' if x['used'] else 'VOLUME') if record else 'TABLE'
    o = x['oem']
    tsk = 'VOLUME' if (o[:5] in (b'MSDOS', b'MSWIN') or o[:4] == b'NTFS' or o[:3] == b'FAT') else 'TABLE'
    return dict(kernel=kernel, blkid=blkid, tsk=tsk, decided=decided)


def main(paths):
    cols = ['kernel', 'blkid', 'tsk', 'decided']
    print('image\tsig\tboot_ind\ttypes\tused\tfat\texfat\tntfs\toem\t' + '\t'.join(cols))
    tally = {c: {} for c in cols}
    for p in paths:
        x = facts(p)
        r = rules(x)
        for c in cols:
            tally[c][r[c]] = tally[c].get(r[c], 0) + 1
        print('\t'.join([p.rsplit('/', 1)[-1], str(x['sig']),
                         ' '.join(f'{v:02x}' for v in x['boot']),
                         ' '.join(f'{v:02x}' for v in x['types']), str(x['used']),
                         str(x['fat']), str(x['exfat']), str(x['ntfs']),
                         ''.join(chr(c) if 0x21 <= c < 0x7f else '.' for c in x['oem'])]
                        + [r[c] for c in cols]))
    for c in cols:
        print(f'# {c}: ' + ', '.join(f'{k} {v}' for k, v in sorted(tally[c].items())))


if len(sys.argv) < 2:
    sys.exit(__doc__)
main(sys.argv[1:])
