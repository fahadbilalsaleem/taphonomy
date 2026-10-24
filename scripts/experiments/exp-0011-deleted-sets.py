#!/usr/bin/env python3
"""For one image: the deleted entry sets phase 1 of ADR-0022 reads, by a
walk written apart from Taphonomy's, and what reading them as sets does to
the count of entries each directory reports.

Walks the root and every live directory (a File set whose SetChecksum is
valid and whose Directory attribute is set), breadth first. Each unused
File entry (05h) is read as ADR-0022 Decision B and Appendix B read it: a
count below two consumes nothing; otherwise up to the declared count of
unused secondaries (TypeCode & C0h == 40h) are consumed, stopping at the
first entry that is not one. A set is counted usable here where it is
complete, its checksum with every InUse bit set again is valid, and its
second entry is a Stream Extension.

Prints one line: the deleted sets, how many are usable, how many straddle
two or more directory clusters, how many are cut short by an entry that is
not an unused secondary, and the entry count of each directory
under two readings: each unused entry counted on its own (Taphonomy at
b7ec0cb), and a deleted set counted once (Appendix B).
The volume is found at byte 0, through a GPT, or through the first MBR
entry, with the backup boot sector where the main one is zeroed. The
script assumes a well-formed volume. Usage: exp-0011-deleted-sets.py IMAGE"""
import struct
import sys
from collections import deque

img = open(sys.argv[1], 'rb').read()
if img[3:11] == b'EXFAT   ':
    lba = 0
elif img[450] == 0xEE and img[512:520] == b'EFI PART':
    lba = struct.unpack_from('<Q', img, 1056)[0]
else:
    lba = struct.unpack_from('<I', img, 454)[0]
b = lba * 512
boot = b if img[b + 3:b + 11] == b'EXFAT   ' else b + 12 * 512
fo, fl, ho, cc, root = struct.unpack_from('<IIIII', img, boot + 80)
bps = 1 << img[boot + 108]
spc = 1 << img[boot + 109]
cb = bps * spc
per = cb // 32


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


def read(entries, as_sets, tally):
    """The directory's entry count, and its live subdirectories in order."""
    n, i, top, subs = len(entries), 0, 0, []
    while i < n:
        t = entries[i][0]
        top += 1
        i += 1
        if t == 0:
            break
        if t & 0x80 == 0:
            if t == 0x05 and as_sets:
                start, declared = i - 1, entries[i - 1][1]
                tally['sets'] += 1
                if declared < 2:
                    continue
                got = 0
                while got < declared and i < n and entries[i][0] & 0xC0 == 0x40:
                    i += 1
                    got += 1
                raw = bytearray(b''.join(entries[start:i]))
                for k in range(0, len(raw), 32):
                    raw[k] |= 0x80
                if got == declared and checksum(raw) == struct.unpack_from('<H', raw, 2)[0] \
                        and raw[32] == 0xC0:
                    tally['usable'] += 1
                if start // per != (i - 1) // per:
                    tally['straddling'] += 1
                if got < declared:
                    tally['cut_short'] += 1
            continue
        if t == 0x80 or t & 0x40:
            continue
        declared = entries[i - 1][1]
        if t == 0x85 and declared < 2:
            continue
        start, got = i - 1, 0
        while got < declared and i < n and entries[i][0] & 0xC0 == 0xC0:
            i += 1
            got += 1
        if t == 0x85 and got == declared:
            s = b''.join(entries[start:i])
            if checksum(s) == struct.unpack_from('<H', s, 2)[0] \
                    and struct.unpack_from('<H', s, 4)[0] & 0x10:
                st = s[32:64]
                fc = struct.unpack_from('<I', st, 20)[0]
                dl = struct.unpack_from('<Q', st, 24)[0]
                need = -(-dl // cb)
                subs.append((list(range(fc, fc + need)) if st[1] & 2 else chain(fc, need), dl))
    return top, subs


def walk(as_sets, tally):
    counts, q = [], deque([(chain(root), None)])
    while q:
        cl, dl = q.popleft()
        raw = b''.join(img[off(c):off(c) + cb] for c in cl)
        if dl is not None:
            raw = raw[:dl]
        top, subs = read([raw[k:k + 32] for k in range(0, len(raw) - 31, 32)], as_sets, tally)
        counts.append(top)
        q.extend(subs)
    return counts


tally = {'sets': 0, 'usable': 0, 'straddling': 0, 'cut_short': 0}
each = walk(False, {})
once = walk(True, tally)
print(f"{sys.argv[1].rsplit('/', 1)[-1]}\tdeleted_sets={tally['sets']}"
      f"\tusable={tally['usable']}\tstraddling={tally['straddling']}"
      f"\tcut_short={tally['cut_short']}"
      f"\tentries_each={','.join(map(str, each))}\tentries_once={','.join(map(str, once))}")
