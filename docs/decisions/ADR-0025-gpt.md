# ADR-0025: GPT Disks Are Read, the Backup Header Is Used When the Primary Is Invalid, and Nothing Is Repaired

* **Status:** Proposed
* **Date:** 2026-11-26
* **Decision owners:** Taphonomy project; decided by the planning session
  under the decision-making the author delegated on 2026-11-13 and
  restated on 2026-11-20
* **Scope:** How the run recognises a GPT disk, which of its two headers
  it believes, which entries it analyses, and what it reports about the
  rest
* **Related:** `ADR-0005` (which deferred GPT); `ADR-0024`; `ADR-0018`
  section 5; `ADR-0022` section 15

---

## 1. Context

`ADR-0005` section 2: "GPT is deferred, not rejected. It is required
before Taphonomy can process SDUC media or modern internal drives." Since
M2 a protective MBR is detected and reported as "GPT detected: not
supported", coverage none, exit status 3. Every public GPT image the
project holds is therefore unanalysed: NIST's nine exFAT images on GPT
(dfr-01, 02, 03, 04, 05, 07, 11, 12, 16), eleven of Nordvik and
Axelsson's, and the three GPT cards of the video chat's third set of
scenarios, on which The Sleuth Kit recovered all eight deleted photos.

All of those are healthy: both headers valid, one basic-data entry. How
a reader should behave on a damaged GPT is shown by no public image, so
this decision rests on the specification, on three open readers' code,
and on damaged copies of one healthy card made here (section 3).

## 2. The specification

UEFI Specification 2.11, chapter 5, read from copies saved by the
research session (chapter 5 page sha256 `2a390747…`, chapter 4 page
`8ccde3e8…`; uefi.org refuses this project's fetches). Section 5.3.2
names four checks: "Check the Signature", "Check the Header CRC", "Check
that the MyLBA entry points to the LBA that contains the GUID Partition
Table", "Check the CRC of the GUID Partition Entry Array", and for the
primary, "Check the AlternateLBA to see if it is a valid GPT". "If the
primary GPT is invalid, the backup GPT is used instead". "If both the
primary and backup GPTs are corrupted this block device is defined as
not having a valid GUID Partition Header." The restoring duties that
follow are for software that maintains the disk; a stale GPT left behind
a legacy MBR "will misinterpret the contents of the disk" if honoured.
Table 5.5 bounds HeaderSize ("greater than or equal to 92 and ... less
than or equal to the logical block size"), fixes Revision 0x00010000
and SizeOfPartitionEntry "128 x 2 n". Section 5.3.3: a partition is "all
the logical blocks inclusive of the StartingLBA and EndingLBA"; a zero
PartitionTypeGUID "defines that this partition entry is not being used".
Section 4.2.1 names the CRC only as "a standard CCITT32 CRC algorithm
with a seed polynomial value of 0x04c11db7"; its other parameters come
from outside the specification (Decision H).

## 3. How others decide, read and measured (2026-11-26)

Read: Linux `block/partitions/efi.c`, libblkid `partitions/gpt.c`, The
Sleuth Kit `tsk/vs/gpt.c` (behaviour only; no code taken). Measured on
2026-11-26 outside the repository: 25 damaged copies of the third set's
`gpt-fat32` card, each checked to hold exactly its damage, and sgdisk
1.0.10, fdisk and partx from util-linux 2.39.3, and mmls from The Sleuth
Kit 4.12.1 run on each. The scripts that make these cases from a card
they build themselves (sgdisk, `mkfs.vfat --invariant`, mtools, at a
pinned clock), and that run the tools, are to be committed with the code
that implements this decision, and must reproduce the findings below
before this decision is accepted.

* **A bad primary.** libblkid, fdisk and sgdisk use the backup; Linux
  does only when booted with the `gpt` option; The Sleuth Kit checks no
  CRC, so it uses a corrupt primary as it stands, and with both headers
  corrupt it still lists a partition.
* **Fields the test list does not name.** Linux and libblkid require
  92 <= HeaderSize <= the block, an entry size of exactly 128 and a
  usable range inside the disk; sgdisk refuses a Revision it does not
  know; none checks the Reserved bytes.
* **Entries.** libblkid drops every entry outside the usable range;
  Linux drops one that starts after it ends or lies beyond the disk;
  sgdisk and The Sleuth Kit list both such entries, and all four tools
  list an entry that starts after it ends, three with the extent it
  states.
* **An oversized entry count.** sgdisk aborts (`std::bad_alloc`).

No tool is right on every case.

## 4. Decisions

**A. Recognising a GPT disk.** Sector 0 is classified first (`ADR-0024`),
with one change: a sector with the `0x55AA` signature and a `0xEE` record
in any slot is a table whatever its boot indicators, because UEFI 5.2.3
says a protective record's BootIndicator "Must be ignored by UEFI
implementations" (today such a sector is classified as neither table nor
volume). A sector that also identifies as a FAT32 or exFAT boot record is
ambiguous, as `ADR-0024` already treats a boot record that lists used
entries, because the record offsets lie inside boot code. A table with a
`0xEE` record in any slot is a protective MBR, and the GPT is read. A
protective record whose StartingLBA is not 1, or whose SizeInLBA is
neither the image's sectors minus one nor `0xFFFFFFFF`, is
an anomaly. Any other non-zero record is an anomaly ("hybrid MBR") and is
not analysed as a partition: the GPT is the table.

**B. A stale GPT.** A table with no `0xEE` record is read as an MBR, as
now. If LBA 1 holds a header that passes Decision C, the run reports
that a GPT header lies behind a legacy MBR and is not honoured (UEFI
5.3.2).

**C. A header is valid** where its signature is "EFI PART"; 92 <=
HeaderSize <= 512 (Decision I); its CRC over HeaderSize bytes, with the
CRC field zeroed, matches; MyLBA is the LBA it was read from;
FirstUsableLBA <= LastUsableLBA; SizeOfPartitionEntry is 128 x 2^n and
at most 512 (the upper limit is the project's); the entry array lies
inside the image, where the specification places it (the primary's
after the header and before FirstUsableLBA, the backup's after
LastUsableLBA and before its header),
and within the project's bound of 1 MiB (a bound chosen by the project,
not by the specification, and a parameter a test can make small); and
the array's CRC matches. A Revision other than 0x00010000, non-zero
Reserved bytes, and a usable range that runs past the end of the image
(a truncated image) are anomalies, not failures.

**D. Which header is believed.** A valid primary is used. The backup is
then read at the primary's AlternateLBA and, where that is not the
image's last LBA, at the last LBA too; a backup that is missing,
invalid, or disagrees with the primary (MyLBA and AlternateLBA not
naming each other, the usable range, DiskGUID, the entry count or size,
the array CRC) is an anomaly. An invalid primary is reported with the
check it failed, and the backup at the image's last LBA is used if
valid. With both invalid the disk has no valid GPT: a gap, coverage
none, exit status 3, with each header's failed check reported. Nothing
is ever written: no header is restored.

**E. Entries.** Entries are read in index order from the believed
header's array. An entry with a zero PartitionTypeGUID is unused; if any
other byte of it is non-zero, that is an anomaly. A used entry whose
StartingLBA is after its EndingLBA, or whose extent runs past the end of
the image, is left out and counted as a partition declared and not
analysed (as `ADR-0005`'s MBR entries beyond the end are). So is a used
entry whose start or length does not fit 32 bits of sectors (past
2 TiB), because the analysis counts a volume's sectors in 32 bits; this
limit is the project's, reported as such, and to be lifted when the
volume extent widens. A used entry
outside FirstUsableLBA to LastUsableLBA but inside the image, and two
entries that overlap, are anomalies; such entries are analysed.

**F. A used entry is analysed by what lies at its extent**, as an MBR
partition is: the filesystem is identified by its boot record, whatever
the type GUID says. An MBR's type byte is used for two things only: the
line reporting a mismatch with the filesystem found, and the search for
an exFAT backup boot region behind an unidentified `0x07` partition
(`ADR-0019` section 3). For GPT the search applies to a Microsoft
basic-data partition (EBD0A0A2-B9E5-4433-87C0-68B6B72699C7), the type
of every partition on the 20 public GPT images surveyed here and on the
third set's GPT cards (sgdisk's code 0700); no mismatch is reported
(section 8). The report prints the type GUID, and a name for the types in
the specification's Table 5.7 and for Microsoft basic data.

**G. Naming.** A partition is numbered by its entry index plus one, as
Linux and libblkid number it, and its artifacts are named `p<N>-`.

**H. The CRC-32** is the project's own, with polynomial 0x04C11DB7
(UEFI 4.2.1) and, from outside the specification, initial value
0xFFFFFFFF, reflected input and output, and a final XOR of 0xFFFFFFFF,
as Linux's `efi_crc32` and libblkid's `count_crc32` compute it. Its test
vectors: "123456789" gives 0xCBF43926, and the header and array CRCs of
the third set's three GPT cards, the `gpt-protective` fixture and NIST's
dfr-02 and dfr-05, each of which the specification's checks pass.

**I. Logical blocks of 512 bytes only.** Where LBA 1 holds no GPT
signature and byte 4096 holds "EFI PART", the run reports a GPT with
4096-byte logical blocks, not analysed: a gap, exit status 3.

## 5. Reasoning

The four checks and the backup are the specification's. Linux's default
of ignoring a valid backup leaves a recoverable disk unread; The Sleuth
Kit's trust of a header that fails its CRC lists partitions from damaged
data. Between them, a forensic reader states which header it used and
why. The fields the test list does not name are bounded where an
unbounded value could mislead (HeaderSize, entry size, the array's
place and size) and reported where it cannot (Revision, Reserved). An
entry that cannot describe sectors on the image is left out, as an MBR
entry is; one that can is analysed and flagged, because the filesystem
probe, not the table, says what lies there.

## 6. Acceptance (to be run when the code exists)

1. NIST's nine GPT images: per set, the binary's decision equals
   EXP-0011's scanner under `moec`, and every recovered file is judged.
   `ADR-0022` section 15 predicts two wrong recoveries there
   (`/.Trashes/._501` on dfr-02 and dfr-05); the run checks that the
   prediction holds. Whether a release may ship them is the author's
   decision, not this one's.
2. Nordvik and Axelsson's eleven GPT images: each partition is found and
   analysed; decisions equal the scanner's.
3. The third set's three GPT cards: from the filesystem line onwards the
   report equals that of the same volume in an MBR, and every deleted
   photo is judged against the set's ground truth.
4. The 25 damaged cases: each run's header choice, anomalies and
   partitions equal the outcome these decisions give, case by case.
5. `scripts/experiments/report-diff.sh` over the project's 106-image set:
   only images with a protective MBR change.

## 7. Held by tests

Each of Decisions A to I gets a unit test that fails with its rule
removed, and the damaged cases that matter most (a broken primary, both
headers broken, 4096-byte blocks, a GPT behind a legacy MBR, the exFAT
backup search behind basic data) are built by tests from the fixtures
and run through the binary.

## 8. Not decided here

A mismatch between a type GUID and the filesystem found; GPT disks with
4096-byte logical blocks; reading the part of a partition that lies
inside a truncated image.
