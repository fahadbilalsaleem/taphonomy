#!/usr/bin/env python3
"""Build a healthy GPT card and damaged copies of it, then check each copy.

Usage:
  python3 -I gpt-damage-cases.py OUTDIR
      build base.img and base-rebuild.img, cases c00 to c25 under
      OUTDIR/cases, then check every case (refuses an existing OUTDIR)
  python3 -I gpt-damage-cases.py --check OUTDIR
      check the cases already under OUTDIR/cases and rewrite cases.tsv

The base is built twice in one run with sgdisk, mkfs.vfat and mtools; the
run stops if the two digests differ. Each case is a sparse copy of the base
with one change applied in place. CRCs are recomputed only where a case says
so. Every case is then read back and tested with the UEFI validity rule
(signature, header CRC, MyLBA, partition array CRC), independently of how it
was made, and compared with the expected table. Any mismatch exits 1.
Python standard library only. Nothing here writes to a device.
"""
import hashlib
import os
import struct
import subprocess
import sys
import uuid
import zlib

MIB = 1 << 20
SECTOR = 512
EPOCH = "1577836800"
DISK_GUID = "5ca1e000-0000-4000-8000-000000000001"
PART_GUID = "5ca1e000-0000-4000-8000-000000000002"
FIRST_LBA = 2048
LAST_LBA = 131038
PRIMARY_LBA = 1
BACKUP_LBA = 131071
FAT_BLOCKS = "64495"  # mkfs.vfat counts 1 KiB blocks: 128990 sectors
FILE_BLOCKS = 8
FILES = ("alpha.txt", "beta.txt")
MBR_SLOT_OFFSET = 446
BASIC_DATA_TYPE = uuid.UUID("EBD0A0A2-B9E5-4433-87C0-68B6B72699C7").bytes_le
NEW_UNIQUE = uuid.UUID("6f1c2a84-3b9e-4d57-9a10-2c8e5f7d1b43").bytes_le
FAT32_RECORD = (
    bytes([0x00, 0, 0, 0, 0x0C, 0, 0, 0])  # status, CHS first, type, CHS last
    + struct.pack("<II", 2048, 128991)     # start, sector count
)
ZERO_MBR_SLOT = bytes(16)
CASE_IDS = [f"c{n:02d}" for n in range(26)]
ENV = {
    "PATH": "/usr/sbin:/usr/bin:/sbin:/bin",
    "LC_ALL": "C",
    "SOURCE_DATE_EPOCH": EPOCH,
    "TZ": "UTC",
    "MTOOLS_SKIP_CHECK": "1",
}

V, I, M = "valid", "invalid", "missing"
EXPECT = {
    "c00": (V, V), "c01": (I, V), "c02": (I, V), "c03": (I, V), "c04": (I, V),
    "c05": (I, I), "c06": (V, I), "c07": (V, V), "c08": (V, V),
    "c09": (V, V), "c10": (V, V), "c11": (V, V), "c12": (V, V),
    "c13": (V, V), "c14": (V, V), "c15": (V, V), "c16": (V, V), "c17": (V, V),
    "c18": (I, I), "c19": (V, V), "c20": (V, V), "c21": (V, V), "c22": (V, V),
    "c23": (V, V), "c24": (V, M), "c25": (V, V),
}


# --- byte-level helpers -----------------------------------------------------

def crc32(data):
    return zlib.crc32(data) & 0xFFFFFFFF


def u32(b, off):
    return struct.unpack_from("<I", b, off)[0]


def u64(b, off):
    return struct.unpack_from("<Q", b, off)[0]


def read_at(f, offset, length, file_size):
    """Bytes at offset, or None if they run past the end of the file."""
    if offset < 0 or length < 0 or offset + length > file_size:
        return None
    f.seek(offset)
    data = f.read(length)
    return data if len(data) == length else None


def rd(f, off, n):
    f.seek(off)
    data = f.read(n)
    if len(data) != n:
        raise RuntimeError(f"short read at {off}: wanted {n}, got {len(data)}")
    return bytearray(data)


def wr(f, off, data):
    f.seek(off)
    f.write(bytes(data))


def hdr_read(f, lba):
    return rd(f, lba * SECTOR, SECTOR)


def hdr_write(f, lba, h):
    wr(f, lba * SECTOR, h)


def header_crc(sector):
    """CRC-32 over HeaderSize bytes with the HeaderCRC32 field zeroed.

    None when HeaderSize is outside 1..512: the header sector cannot cover it.
    """
    size = u32(sector, 12)
    if size < 1 or size > SECTOR:
        return None
    data = bytearray(sector[:size])
    data[16:20] = b"\0\0\0\0"
    return crc32(bytes(data))


def hdr_recrc(h):
    h[16:20] = b"\0\0\0\0"
    struct.pack_into("<I", h, 16, header_crc(h))


def arr_region(h):
    """Byte offset and length of the partition entry array a header names."""
    return u64(h, 72) * SECTOR, u32(h, 80) * u32(h, 84)


def entry_fields(entry):
    name = bytes(entry[56:128]).decode("utf-16-le", errors="replace")
    return {
        "TypeGUID": str(uuid.UUID(bytes_le=bytes(entry[0:16]))),
        "UniqueGUID": str(uuid.UUID(bytes_le=bytes(entry[16:32]))),
        "StartingLBA": u64(entry, 32),
        "EndingLBA": u64(entry, 40),
        "Attributes": u64(entry, 48),
        "Name": name.rstrip("\0"),
    }


def mbr_slots(sector0):
    slots = []
    for i in range(4):
        off = MBR_SLOT_OFFSET + 16 * i
        slots.append({
            "slot": i + 1,
            "status": sector0[off],
            "type": sector0[off + 4],
            "start": u32(sector0, off + 8),
            "sectors": u32(sector0, off + 12),
        })
    return slots


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def run(argv, data=None, env=None):
    """Run a tool with no terminal input and a timeout; stop on any failure."""
    stdin = {"input": data} if data is not None else {"stdin": subprocess.DEVNULL}
    proc = subprocess.run(argv, capture_output=True, timeout=60, env=env,
                          check=False, **stdin)
    if proc.returncode != 0:
        err = proc.stderr.decode("utf-8", "replace").strip()
        raise SystemExit(f"STOP: {argv[0]} exited {proc.returncode}: {err}")


# --- base card --------------------------------------------------------------

def filler(name):
    """512-byte blocks, each starting 'TAG <name> <block> '."""
    return b"".join(
        f"TAG {name} {i} ".encode().ljust(SECTOR, b".") for i in range(FILE_BLOCKS)
    )


def build_base(path):
    with open(path, "xb") as f:
        f.truncate(64 * MIB)
    run(["sgdisk", "-U", DISK_GUID, "-n", f"1:{FIRST_LBA}:{LAST_LBA}",
         "-t", "1:0700", "-u", f"1:{PART_GUID}", path])
    run(["mkfs.vfat", "--invariant", "-F", "32", "-n", "CARD", "--mbr=n",
         f"--offset={FIRST_LBA}", path, FAT_BLOCKS], env=ENV)
    part = f"{path}@@{FIRST_LBA * SECTOR}"
    for name in FILES:
        run(["mcopy", "-i", part, "-", f"::{name}"], data=filler(name), env=ENV)
    run(["mdel", "-i", part, "::beta.txt"], env=ENV)


def build_bases(outdir):
    first = os.path.join(outdir, "base.img")
    second = os.path.join(outdir, "base-rebuild.img")
    build_base(first)
    build_base(second)
    digest = sha256(first)
    print(f"base sha256: {digest}")
    print(f"second build sha256: {sha256(second)}")
    if sha256(second) != digest:
        raise SystemExit("STOP: the two base builds differ")
    return first


# --- damage -----------------------------------------------------------------

def hdr_write_recrc_both(f, edit, recrc=True):
    for lba in (PRIMARY_LBA, BACKUP_LBA):
        h = hdr_read(f, lba)
        edit(h)
        if recrc:
            hdr_recrc(h)
        hdr_write(f, lba, h)


def entries_both_arrays(f, fn, header_edit=None):
    """Replace each array with fn(array), set its CRC, edit and re-CRC its header."""
    for lba in (PRIMARY_LBA, BACKUP_LBA):
        h = hdr_read(f, lba)
        off, length = arr_region(h)
        new = bytes(fn(rd(f, off, length)))
        if len(new) != length:
            raise RuntimeError("array length changed unexpectedly")
        wr(f, off, new)
        struct.pack_into("<I", h, 88, crc32(new))
        if header_edit:
            header_edit(h)
        hdr_recrc(h)
        hdr_write(f, lba, h)


def set_entry0(arr, offset, data):
    arr[offset:offset + len(data)] = data


def c01(f):
    h = hdr_read(f, PRIMARY_LBA)
    h[56] ^= 0xFF
    hdr_write(f, PRIMARY_LBA, h)


def c02(f):
    h = hdr_read(f, PRIMARY_LBA)
    h[0:8] = bytes(8)
    hdr_write(f, PRIMARY_LBA, h)


def c03(f):
    h = hdr_read(f, PRIMARY_LBA)
    struct.pack_into("<Q", h, 24, 2)
    hdr_recrc(h)
    hdr_write(f, PRIMARY_LBA, h)


def c04(f):
    h = hdr_read(f, PRIMARY_LBA)
    off, length = arr_region(h)
    arr = rd(f, off, length)
    arr[56] ^= 0xFF  # entry 0's first name byte
    wr(f, off, arr)


def c05(f):
    hdr_write_recrc_both(f, lambda h: h.__setitem__(56, h[56] ^ 0xFF), recrc=False)


def c06(f):
    h = hdr_read(f, BACKUP_LBA)
    h[56] ^= 0xFF
    hdr_write(f, BACKUP_LBA, h)


def c07(f):
    h = hdr_read(f, PRIMARY_LBA)
    h[56] ^= 0xFF
    hdr_recrc(h)
    hdr_write(f, PRIMARY_LBA, h)


def c08(f):
    f.truncate(os.path.getsize(f.name) + MIB)


def c09(f):
    wr(f, MBR_SLOT_OFFSET, FAT32_RECORD)  # replaces the 0xEE entry in slot 1


def c10(f):
    wr(f, MBR_SLOT_OFFSET + 16, FAT32_RECORD)  # slot 2; slot 1 keeps 0xEE


def c11(f):
    slot1 = rd(f, MBR_SLOT_OFFSET, 16)
    wr(f, MBR_SLOT_OFFSET + 16, slot1)
    wr(f, MBR_SLOT_OFFSET, ZERO_MBR_SLOT)


def c12(f):
    wr(f, MBR_SLOT_OFFSET + 8, struct.pack("<I", 2))  # 0xEE entry's StartingLBA


def c13(f):
    def fn(arr):
        set_entry0(arr, 0, bytes(16))  # type GUID zeroed
        return arr
    entries_both_arrays(f, fn)


def c14(f):
    def fn(arr):
        set_entry0(arr, 40, struct.pack("<Q", 200000))
        return arr
    entries_both_arrays(f, fn)


def c15(f):
    def fn(arr):
        set_entry0(arr, 32, struct.pack("<Q", 20))
        return arr
    entries_both_arrays(f, fn)


def c16(f):
    entry1 = (BASIC_DATA_TYPE + NEW_UNIQUE
              + struct.pack("<QQQ", 60000, 70000, 0) + bytes(72))

    def fn(arr):
        set_entry0(arr, 128, entry1)
        return arr
    entries_both_arrays(f, fn)


def c17(f):
    def fn(arr):
        # 64 entries of 256 bytes: each old 128-byte entry, then 128 zero bytes
        return b"".join(bytes(arr[i * 128:(i + 1) * 128]) + bytes(128)
                        for i in range(64))

    def hedit(h):
        struct.pack_into("<I", h, 80, 64)
        struct.pack_into("<I", h, 84, 256)
    entries_both_arrays(f, fn, header_edit=hedit)


def c18(f):
    hdr_write_recrc_both(f, lambda h: struct.pack_into("<I", h, 80, 0x7FFFFFFF))


def c19(f):
    hdr_write_recrc_both(f, lambda h: struct.pack_into("<I", h, 12, 512))


def c20(f):
    hdr_write_recrc_both(f, lambda h: struct.pack_into("<I", h, 12, 91))


def c21(f):
    hdr_write_recrc_both(f, lambda h: struct.pack_into("<I", h, 8, 0x00010001))


def c22(f):
    hdr_write_recrc_both(f, lambda h: h.__setitem__(slice(20, 24), b"\x01\x01\x01\x01"))


def c23(f):
    def fn(arr):
        set_entry0(arr, 32, struct.pack("<Q", 100000))
        set_entry0(arr, 40, struct.pack("<Q", 50000))
        return arr
    entries_both_arrays(f, fn)


def c24(f):
    f.truncate(63 * MIB)


def c25(f):
    wr(f, MBR_SLOT_OFFSET, b"\x01")  # protective record's BootIndicator, nothing else


CHANGES = {
    "c00": None,
    "c01": c01, "c02": c02, "c03": c03, "c04": c04, "c05": c05, "c06": c06,
    "c07": c07, "c08": c08, "c09": c09, "c10": c10, "c11": c11, "c12": c12,
    "c13": c13, "c14": c14, "c15": c15, "c16": c16, "c17": c17, "c18": c18,
    "c19": c19, "c20": c20, "c21": c21, "c22": c22, "c23": c23, "c24": c24,
    "c25": c25,
}


def make_cases(outdir, base):
    cases = os.path.join(outdir, "cases")
    os.mkdir(cases)
    for case_id in CASE_IDS:
        dest = os.path.join(cases, f"{case_id}.img")
        run(["cp", "--sparse=always", base, dest])
        if CHANGES[case_id] is not None:
            with open(dest, "r+b") as f:
                CHANGES[case_id](f)
        print(f"made {case_id}")


# --- check, independent of how the cases were made --------------------------

def uefi_status(f, size, lba):
    """(status, reason) for the header read at lba: valid, invalid or missing."""
    sector = read_at(f, lba * SECTOR, SECTOR, size)
    if sector is None:
        return M, "header sector beyond end of image"
    if bytes(sector[0:8]) != b"EFI PART":
        return I, "signature"
    if header_crc(sector) is None:
        return I, "HeaderSize outside 1..512"
    if header_crc(sector) != u32(sector, 16):
        return I, "header CRC"
    if u64(sector, 24) != lba:
        return I, "MyLBA differs from location read"
    arr = read_at(f, u64(sector, 72) * SECTOR,
                  u32(sector, 80) * u32(sector, 84), size)
    if arr is None:
        return I, "partition array beyond end of image"
    if crc32(arr) != u32(sector, 88):
        return I, "partition array CRC"
    return V, "ok"


def describe(f, size, lba, label, lines):
    sector = read_at(f, lba * SECTOR, SECTOR, size)
    if sector is None:
        lines.append(f"{label} header at LBA {lba}: missing (beyond end of image)")
        return
    n, s = u32(sector, 80), u32(sector, 84)
    hcrc = header_crc(sector)
    arr = read_at(f, u64(sector, 72) * SECTOR, n * s, size)
    lines.append(f"{label} header at LBA {lba}:")
    lines.append(f"  Signature: {bytes(sector[0:8])!r}")
    lines.append(f"  HeaderSize: {u32(sector, 12)}")
    lines.append(f"  Revision: {u32(sector, 8):#010x}")
    lines.append(f"  MyLBA: {u64(sector, 24)}  AlternateLBA: {u64(sector, 32)}")
    lines.append(f"  HeaderCRC stored {u32(sector, 16):#010x} recomputed "
                 + ("unavailable (HeaderSize outside 1..512)" if hcrc is None
                    else f"{hcrc:#010x}"))
    lines.append(f"  Entries: {n} x {s}, array at LBA {u64(sector, 72)}")
    lines.append(f"  ArrayCRC stored {u32(sector, 88):#010x} recomputed "
                 + ("unavailable (array beyond end of image)" if arr is None
                    else f"{crc32(arr):#010x}"))
    if arr is not None:
        for idx in (0, 1):
            lines.append(f"  entry {idx}: {entry_fields(arr[idx * s: idx * s + 128])}")


def check_case(case_id, path):
    size = os.path.getsize(path)
    lines = [f"== {case_id}  image bytes {size}"]
    with open(path, "rb") as f:
        mbr = read_at(f, 0, SECTOR, size)
        lines.append("MBR slots (1-based): " + "; ".join(
            f"slot{s['slot']} type {s['type']:#04x} start {s['start']} "
            f"sectors {s['sectors']}" for s in mbr_slots(mbr)))
        backup_lba = u64(read_at(f, SECTOR, SECTOR, size), 32)
        describe(f, size, PRIMARY_LBA, "PRIMARY", lines)
        describe(f, size, backup_lba, "BACKUP", lines)
        primary = uefi_status(f, size, PRIMARY_LBA)
        backup = uefi_status(f, size, backup_lba)
    lines.append(f"primary: {primary[0]} ({primary[1]})")
    lines.append(f"backup: {backup[0]} ({backup[1]})")
    return primary[0], backup[0], lines, size


def check_cases(outdir):
    cases = os.path.join(outdir, "cases")
    if not os.path.isdir(cases):
        raise SystemExit(f"STOP: no cases folder in {outdir}")
    checkdir = os.path.join(outdir, "check")
    os.makedirs(checkdir, exist_ok=True)
    rows = ["case\tprimary valid\tbackup valid\texpected primary\t"
            "expected backup\tmatches expectation\timage bytes\timage sha256"]
    mismatches = 0
    for case_id in CASE_IDS:
        path = os.path.join(cases, f"{case_id}.img")
        actual_p, actual_b, lines, size = check_case(case_id, path)
        want_p, want_b = EXPECT[case_id]
        mism = []
        if actual_p != want_p:
            mism.append(f"primary: expected {want_p}, got {actual_p}")
        if actual_b != want_b:
            mism.append(f"backup: expected {want_b}, got {actual_b}")
        mismatches += len(mism)
        verdict = "matches brief expectation" if not mism else "MISMATCH: " + "; ".join(mism)
        with open(os.path.join(checkdir, f"{case_id}.txt"), "w", encoding="utf-8") as w:
            w.write("\n".join(lines + [verdict]) + "\n")
        rows.append("\t".join([case_id, actual_p, actual_b, want_p, want_b,
                               "yes" if not mism else "NO", str(size), sha256(path)]))
        print(f"{case_id}: primary {actual_p}, backup {actual_b}  {verdict}")
    with open(os.path.join(outdir, "cases.tsv"), "w", encoding="utf-8") as w:
        w.write("\n".join(rows) + "\n")
    print(f"mismatches: {mismatches}")
    return 1 if mismatches else 0


def main(argv):
    if len(argv) == 3 and argv[1] == "--check":
        return check_cases(argv[2])
    if len(argv) != 2:
        sys.exit(__doc__)
    outdir = argv[1]
    if os.path.exists(outdir):
        raise SystemExit(f"STOP: {outdir} exists")
    os.mkdir(outdir)
    base = build_bases(outdir)
    make_cases(outdir, base)
    return check_cases(outdir)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
