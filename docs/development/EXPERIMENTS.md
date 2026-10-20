# Taphonomy Experiments

This document records recovery experiments performed during development,
each stating its question, hypothesis, input, method, expected result,
actual result, conclusion, and next action, so that recovery claims remain
traceable to the controlled evidence that produced them.

---

## EXP-0001: Determinism of synthetic partition fixture generation

**Date:** 2026-08-29
**Milestone:** M2 (ADR-0002 §8)
**Related:** ADR-0002 Appendix A §A.3; `PROJECT.md` §6.4;
`DEVELOPMENT_ENVIRONMENT.md` §11

---

### Question

Does `scripts/generate-fixtures.sh` produce byte-identical output on repeated
runs?

### Why it matters

`PROJECT.md` §6.4 requires deterministic behaviour where technically
possible. `DEVELOPMENT_ENVIRONMENT.md` §11 requires fixtures that are
deterministic and reproducible.

If fixture generation is not deterministic, a fixture-based test cannot
distinguish a regression in the parser from a change in the fixture, and
`DEVELOPMENT_ENVIRONMENT.md` §12's golden-result model does not hold.

ADR-0002 Appendix A §A.3 recorded FAT32 fixture determinism as an
**unverified** claim and required that it be measured before being cited.
This experiment discharges that obligation.

### Hypothesis

`mkfs.vfat --invariant` fixes values that would otherwise derive from the
system clock or a random source, and `sfdisk` with an explicit `label-id`
writes no time-derived fields. Given explicit GUIDs, `sgdisk` should
likewise be reproducible.

Predicted outcome: all seven fixtures byte-identical.

Confidence before running: moderate. `sgdisk` was considered the most likely
to fail, since GPT headers carry a disk GUID and the tool may ignore an
explicitly supplied one.

### Input

`scripts/generate-fixtures.sh`, producing seven images:

```text
mbr-single-fat32.img
mbr-four-partitions.img
mbr-empty.img
no-signature.img
bad-signature.img
partition-beyond-end.img
gpt-protective.img
```

Each 64 MiB (131,072 sectors of 512 bytes).

### Environment

```text
Host              Ubuntu 24.04 (noble) under WSL2 on Windows
mkfs.fat          4.2 (2021-01-31), dosfstools 4.2-1.1build1
sfdisk            util-linux 2.39.3
sgdisk            GPT fdisk 1.0.10
```

### Method

`scripts/verify-fixtures.sh` generates the full set twice into separate
temporary directories and compares each image with `cmp`.

No root privileges, no loop devices, no mounted filesystems. All targets are
regular files.

### Expected result

Seven of seven byte-identical.

### Actual result

**Seven of seven byte-identical.**

```text
IDENTICAL   bad-signature.img
IDENTICAL   gpt-protective.img
IDENTICAL   mbr-empty.img
IDENTICAL   mbr-four-partitions.img
IDENTICAL   mbr-single-fat32.img
IDENTICAL   no-signature.img
IDENTICAL   partition-beyond-end.img

Deterministic: 7/7 fixtures byte-identical across runs.
```

Recorded digests are in `fixtures/partition/MANIFEST.sha256`.

### Structural verification

Determinism alone does not establish that a fixture contains what it claims.
Each was inspected directly.

`mbr-single-fat32.img`, entry 1 at offset `0x1BE`:

```text
80 20 21 00 0c 28 20 08 00 08 00 00 00 f8 01 00
```

`0x80` bootable, type `0x0c` (W95 FAT32 LBA), start LBA 2048
(`00 08 00 00` little-endian), 129,024 sectors. Signature `55aa` at `0x1FE`.

FAT32 volume boot record at byte offset 1,048,576 (LBA 2048):

```text
eb 58 90 6d 6b 66 73 2e 66 61 74   jump, OEM "mkfs.fat"
... 54 41 50 48 46 49 58           label "TAPHFIX"
... 46 41 54 33 32                 type "FAT32"
```

Confirms `mkfs.vfat --offset=2048` wrote the filesystem inside the partition
rather than at sector 0.

`gpt-protective.img`, entry 1:

```text
00 00 02 00 ee 28 20 08 01 00 00 00 ff ff 01 00
```

Type `0xEE`, start LBA 1, 131,071 sectors. This is the protective MBR
ADR-0005 §4 requires the parser to detect.

`partition-beyond-end.img`, entry 1:

```text
00 20 21 00 0c 28 20 08 00 08 00 00 ff ff ff ff
```

Sector count `0xFFFFFFFF` against a 131,072-sector image.

`bad-signature.img`: signature bytes at `0x1FE` are `0000`.

### Conclusion

Fixture generation is deterministic on this environment and toolchain.
Fixture-based tests may be treated as reproducible, and
`DEVELOPMENT_ENVIRONMENT.md` §12's golden-result model applies.

The determinism claim in ADR-0002 Appendix A §A.3 is now **measured** for
FAT32.

### Limitations

1. Measured on one host with one toolchain version. Determinism across
   `dosfstools`, `util-linux` or `gdisk` versions is **not** established. A
   version change may alter output; `MANIFEST.sha256` is the detection
   mechanism.
2. Determinism across architectures or non-Linux hosts is not established.
3. `mkntfs` determinism, the comparison ADR-0002 §3.2 originally asserted,
   remains **unmeasured**. `ntfs-3g` is not installed and NTFS is not the
   current target. This does not affect the decision, whose primary reason is
   specification availability (ADR-0002 §3.1).

### Next action

1. Commit `MANIFEST.sha256` as a golden result.
2. Add a regression check comparing regenerated fixtures against the manifest
   when the toolchain changes.
3. Proceed to the MBR parser, using these fixtures as its test inputs.

### Re-measurement 2026-08-30

The fixture set grew to eight with the addition of `mbr-type-mismatch.img`.
`verify-fixtures.sh` was re-run and reported 8 of 8 byte-identical.

```text
Deterministic: 8/8 fixtures byte-identical across runs.
```

The conclusion is unchanged.

### Re-measurement 2026-08-30 (second)

The fixture set grew to twelve with the addition of four FAT32 boot
sector corruption fixtures: `fat32-oversized-volume.img`,
`fat32-hidden-mismatch.img`, `fat32-bad-root-cluster.img` and
`fat32-undersized-fat.img`. Each is generated by the same procedure as
`mbr-single-fat32.img`, then corrupted in a single BPB field by a new
`poke_le32` helper.

`verify-fixtures.sh` was re-run and reported 12 of 12 byte-identical.

```text
Deterministic: 12/12 fixtures byte-identical across runs.
```

The conclusion is unchanged. Determinism now covers byte-level
corruption applied after `mkfs.vfat`, which the original measurement did
not exercise.

### Re-measurement 2026-08-30 (third)

`fat32-hidden-mismatch.img` was regenerated. The original poked zero into
`BPB_HiddSec`, which measurement showed was already the value `mkfs.vfat`
writes for a volume formatted with `--offset`:

```text
xxd -s 1048576 -l 96 fixtures/partition/mbr-single-fat32.img
  00100010: 0200 0000 00f8 0000 2000 0800 0000 0000
```

Bytes `0x1C` through `0x1F` are zero. The fixture therefore differed from
`mbr-single-fat32.img` only in its label-id and tested nothing. It now
pokes 9999, a start the volume could not have had.

`verify-fixtures.sh` was re-run and reported 12 of 12 byte-identical.

```text
Deterministic: 12/12 fixtures byte-identical across runs.
```

One manifest digest changed, `fat32-hidden-mismatch.img` from
`1dd69836...` to `c14c1ddd...`. The other eleven are unchanged, which is
the property the manifest exists to establish.

The conclusion is unchanged.

### Re-measurement 2026-09-03

The fixture set grew to fourteen with the addition of
`fat32-root-multicluster.img`, a FAT32 volume whose root directory occupies
more than one cluster. ADR-0008 §8.1 records why it was needed: every
earlier fixture has a single-cluster root directory, so no fixture exercised
cluster chain walking.

`verify-fixtures.sh` was re-run and reported 14 of 14 byte-identical.

```text
Deterministic: 14/14 fixtures byte-identical across runs.
```

The manifest gained one line and the other thirteen digests are unchanged,
which is the property the manifest exists to establish.

**This series moves from twelve to fourteen.** The thirteenth fixture,
`fat32-root-entries.img`, was confirmed byte-identical when it was added on
2026-08-31 — EXP-0002 Next action item 3 records that as done, and EXP-0002
Limitations item 3 relies on it — but the figure was never entered here. The
gap is in this record, not in the measurement.

#### Structural verification of the new fixture

Determinism establishes that the fixture is reproducible. It does not
establish that the fixture exercises what it was built to exercise. The
file allocation table was read directly:

```text
xxd -s 1064960 -l 16 fixtures/partition/fat32-root-multicluster.img
  00104000: f8ff ff0f ffff ff0f 1300 0000 ffff ff0f

FAT[2] = 0x00000013, the root directory continues at cluster 19
```

The root directory chain is 2 → 19 → end of chain. The first cluster holds
the volume label and `FILE01` through `FILE15`: sixteen entries, completely
full, with no terminator anywhere in it. The second holds `FILE16` through
`FILE20` followed by a `0x00` terminator.

Cluster 19 sits between file data on both sides. Cluster 18 holds
`FILE16`'s content and cluster 20 holds `FILE17`'s, because the root
directory grew only when its seventeenth entry would not fit. A chain walk
that read the adjacent cluster instead of following the FAT would therefore
read file text rather than directory entries, and would fail visibly rather
than producing plausible output.

The fixture consequently proves three properties a single-cluster root
directory cannot: that the walk follows the FAT, that it does not treat a
full cluster as terminated, and that a terminator in a later cluster ends
the listing correctly.

One gap remains. `FAT[2]` is `0x00000013` with its high nibble zero, so no
fixture sets the reserved bits of a FAT32 entry. The 28-bit mask required
by ADR-0008 §8.2 is not exercised by any image and must be covered by a
unit test against an in-memory reader.

The conclusion is unchanged.

---

## EXP-0002: Determinism of mtools directory entry timestamps

**Date:** 2026-08-30
**Milestone:** M5 (ADR-0002 §8), prerequisite
**Related:** ADR-0006 §4, §6; `DEVELOPMENT_ENVIRONMENT.md` §11, §12;
`PROJECT.md` §6.4

---

### Question

Does `mcopy` produce byte-identical output across runs, and if not, can its
output be pinned?

### Why it matters

Every fixture before this one is an empty filesystem. M5 requires files
inside a volume, and `mkfs.vfat` cannot create them.

EXP-0001 established that fixture generation is deterministic and that
`MANIFEST.sha256` may therefore be treated as a golden result under
`DEVELOPMENT_ENVIRONMENT.md` §12. That finding was measured against tooling
that writes no timestamps. FAT directory entries carry creation, write and
access times, so a tool that populates them may break the property EXP-0001
established.

ADR-0002 Appendix A records a determinism claim written from assumption and
later found unsupported. This experiment was run before ADR-0006 was drafted
so that the same failure was not repeated.

### Hypothesis

`mcopy` writes the current time into `DIR_CrtTime` and `DIR_WrtTime`, making
its output non-deterministic across runs separated by more than the field's
two-second resolution.

Predicted outcome: two runs in quick succession identical, two runs seconds
apart differing in the directory entry.

Confidence before running: high on the mechanism, low on whether a
mitigation existed.

### Input

Four 64 MiB images built by identical procedure:

```text
dd if=/dev/zero of=IMAGE bs=512 count=131072
sfdisk --quiet --no-tell-kernel IMAGE   (label-id 0x1a2b3c4d, one type=c
                                         partition at LBA 2048)
mkfs.vfat --invariant --mbr=n -F 32 -n TAPHFIX --offset=2048 IMAGE 64512
MTOOLS_SKIP_CHECK=1 mcopy -i IMAGE@@1M payload.txt ::/payload.txt
```

`payload.txt` is 26 bytes, its mtime pinned with
`touch -d '2020-01-01 00:00:00 UTC'`.

All work was done in `/tmp`. No fixture in the repository was touched.

### Environment

```text
Host       Ubuntu 24.04 (noble) under WSL2 on Windows
mtools     4.0.43-1build1
TZ         Asia/Dubai (UTC+4) unless stated otherwise
```

### Method

Four comparisons:

1. `a.img` and `b.img`, built consecutively with no delay.
2. `a.img` and `c.img`, built five seconds apart.
3. `d.img` and `e.img`, three seconds apart, with
   `SOURCE_DATE_EPOCH=1577836800` exported.
4. `d.img` and `f.img`, both with `SOURCE_DATE_EPOCH` set, `f.img`
   additionally with `TZ=UTC`.

Each comparison used `cmp`, and the resulting directory entry was decoded
with `xxd` rather than relying on `cmp` alone. Two runs inside one clock
tick compare equal without being deterministic, and `cmp` cannot distinguish
that case.

### Expected result

Comparison 1 identical, comparison 2 differing.

Comparisons 3 and 4 had no prediction. Whether `mtools` honours
`SOURCE_DATE_EPOCH` was unknown before running.

### Actual result

**1. Same second: identical.**

**2. Five seconds apart: differing.**

```text
cmp a.img c.img
  a.img c.img differ: byte 2081839, line 5
   2081839 263 320
   2081847 263 320
```

Both bytes lie in the payload's root directory entry, at entry offsets
`0x0E` (`DIR_CrtTime`) and `0x16` (`DIR_WrtTime`). Values moved from
`0x98B3` to `0x98D0`, decoding as 19:05:38 to 19:06:32.

Only the time fields moved because both runs fell on the same day. Across
midnight the date fields would move as well.

The payload's pinned mtime did not propagate: the recorded date was the
current date, not 2020-01-01. `mcopy` does not preserve source modification
time by default.

**3. With `SOURCE_DATE_EPOCH=1577836800`: identical, and pinned.**

```text
001fc420: 5041 594c 4f41 4420 5458 5420 1800 0020  PAYLOAD TXT
001fc430: 2150 2150 0000 0020 2150 0300 1a00 0000
```

`DIR_CrtDate` and `DIR_WrtDate` read `0x5021`: year field 40, so 1980+40 =
2020, month 1, day 1. The identity is genuine rather than a clock-tick
coincidence.

**4. Adding `TZ=UTC`: different bytes for the same instant.**

```text
001fc420: 5041 594c 4f41 4420 5458 5420 1800 0000  PAYLOAD TXT
001fc430: 2150 2150 0000 0000 2150 0300 1a00 0000

cmp d.img f.img
  d.img f.img differ: byte 2081840, line 5
```

`DIR_CrtTime` is `0x0000` (00:00:00) under `TZ=UTC` and `0x2000` (04:00:00)
under UTC+4. `SOURCE_DATE_EPOCH=1577836800` is 2020-01-01 00:00:00 UTC in
both cases.

### Conclusion

`mtools` is **not** deterministic as invoked. It becomes deterministic when
both `SOURCE_DATE_EPOCH` and `TZ` are set. Either alone is insufficient:
without the first, output depends on when the script runs; without the
second, on where.

FAT directory entries store local time with no timezone field, so the
timezone dependence is a property of the filesystem format rather than of
`mtools`, and no tool choice avoids it.

`scripts/generate-fixtures.sh` exports both. EXP-0001's conclusion continues
to hold for the fixture set, now including file-bearing fixtures, on the
condition that those exports remain.

### Limitations

1. **Fixture bytes depend on the host timezone.** This is a new limitation.
   EXP-0001 records that determinism across architectures and non-Linux
   hosts is unmeasured; a timezone difference between two developers is far
   more likely than either. The exports in the script are the mitigation,
   and `verify-fixtures.sh` is the detection mechanism if they are removed.
2. Measured on one host with `mtools` 4.0.43-1build1. Whether other versions
   honour `SOURCE_DATE_EPOCH` identically is not established.
3. Only `mcopy` was measured. `mmd`, used for the subdirectory in
   `fat32-root-entries.img`, was not measured separately. It is covered
   transitively: `verify-fixtures.sh` reports that fixture byte-identical
   across runs, and it contains an `mmd`-created directory.
4. Deleting a file with `mdel`, which M6 will require, was not measured.

### Next action

1. Record the decision and its licence reasoning in ADR-0006. **Done.**
2. Export both variables in `scripts/generate-fixtures.sh`. **Done.**
3. Add `fat32-root-entries.img` and confirm 13 of 13 byte-identical.
   **Done.**
4. Measure `mdel` determinism before M6 introduces deleted-entry fixtures.
   **Done.** EXP-0003.

---

## EXP-0003: What mtools deletion destroys, and whether it is deterministic

**Date:** 2026-09-04
**Milestone:** M6 (ADR-0002 §8), prerequisite
**Related:** ADR-0003 §3; ADR-0006 §4, §5.1; ADR-0007 §5.3; ADR-0008 §12.2;
EXP-0002 Limitations 4 and Next action 4; `KNOWN_ISSUES.md`

---

### Question

Two questions, folded into one experiment because they share a volume.

1. Is `mdel` deterministic under `SOURCE_DATE_EPOCH` and `TZ=UTC`?
2. What does `mdel` actually destroy, and what survives?

A third was added before the experiment ran. `mdel` deletes files. It does
not delete directories, and M6 requires a fixture containing a deleted
subdirectory. `mrd` and `mdeltree` are therefore measured alongside it, and
their determinism and destruction behaviour were as unmeasured as `mdel`'s.

### Why it matters

EXP-0002 measured `mcopy` and, transitively, `mmd`. Deletion was not
measured, and `KNOWN_ISSUES.md` records it as required before M6 introduces
deleted-entry fixtures.

Question 2 is the one that decides what an M6 fixture can prove. Deletion
behaviour is implementation-specific rather than specified: the FAT32
specification states that a first byte of `0xE5` marks a free entry, and
says nothing about what else an implementation may do at the same time.
Microsoft implementations are widely reported to zero the high word of the
first cluster, which the specification does not require. If `mtools` does
not, a fixture built with it will not exercise that case, and the tool will
appear more capable against fixtures than against evidence from a Windows
host.

That is the "what a fixture proves versus what it illustrates" problem
recorded in ADR-0008 §8.1, in its second instance.

### Hypothesis

Recorded before the experiment was run.

1. `mdel` is deterministic, and would be even without the exports, because
   deletion writes no timestamp. The exports remain necessary for the
   `mcopy` steps that build the volume. Confidence: high on the mechanism,
   moderate on whether `mtools` touches anything else.
2. `mdel` writes `0xE5` into byte 0 of the short entry and of every
   preceding long-name entry. High.
3. `mdel` zeroes the file's cluster chain in the FAT. High.
4. `mdel` does **not** zero `DIR_FstClusHI`, and does not zero
   `DIR_FstClusLO`. Moderate.
5. Name bytes 1 to 10, `DIR_Attr`, `DIR_NTRes`, all timestamps,
   `DIR_FileSize` and the long-name checksum byte at `LDIR_Chksum` survive
   untouched. High.

No prediction was made about `mrd` or `mdeltree`, or about the FSInfo
sector.

### Input

One 64 MiB image, built by the procedure below and then copied before each
deletion, so that every deletion is measured against the same baseline.

```text
dd if=/dev/zero of=a.img bs=512 count=131072 status=none
mkfs.vfat --invariant --mbr=n -F 32 -n TAPHFIX a.img
mcopy -i a.img low.txt ::/LOWFILE.TXT
mcopy -i a.img long.txt "::/a long deleted name.txt"
mmd   -i a.img ::/SUBDIR
mcopy -i a.img low.txt ::/SUBDIR/INNER.TXT
mcopy -i a.img bigfill.bin ::/BIGFILL.BIN
mcopy -i a.img high.txt ::/HIGHFILE.TXT
```

No partition table. The image is a bare FAT32 volume, because the question
concerns entry and FAT bytes and a partition table would only displace every
offset by a constant.

Five deletion targets, each chosen for a property:

```text
LOWFILE.TXT               pure 8.3 name, one entry, low cluster
a long deleted name.txt   two long-name entries preceding a short entry
SUBDIR                    a directory, removed by mrd and by mdeltree
SUBDIR/INNER.TXT          a file inside a directory that is then removed
HIGHFILE.TXT              first cluster above 65,535
```

`BIGFILL.BIN` is 33 MiB of zeroes and is never deleted. It exists only to
consume clusters so that `HIGHFILE.TXT`, allocated after it, begins above
cluster 65,535 and can test the high word of the first cluster. Measured, it
does: `HIGHFILE.TXT` begins at cluster 67,591, with `DIR_FstClusHI` = 1 and
`DIR_FstClusLO` = 2055.

All work was done in `/tmp/exp0003`. No fixture in the repository was
touched, and no repository file was read or written by the experiment.

### Environment

```text
Host       Ubuntu 24.04 (noble) under WSL2 on Windows
mtools     4.0.43-1build1
mkfs.fat   4.2 (2021-01-31)
Exports    SOURCE_DATE_EPOCH=1577836800  TZ=UTC  MTOOLS_SKIP_CHECK=1
```

Volume geometry, read from the BIOS parameter block rather than from the
command line that produced it:

```text
BPS=512 SPC=1 RSV=32 FATs=2 FATSz32=1009 RootClus=2
FAT 0 at 16384   FAT 1 at 532992   data and root at 1049600
129,022 clusters
```

### Method

Deletions were performed on copies of one baseline image, and each result
compared against the baseline byte by byte. Every differing byte was located
to a named structure and decoded, rather than counted.

`cmp` alone is insufficient for the determinism question, for the reason
EXP-0002 records: two runs inside one clock tick compare equal without being
deterministic. Determinism was therefore tested by deleting from the same
baseline twice, four seconds apart, and the decoded result inspected
separately.

```text
1. dump the baseline root directory, decoding every entry
2. cp a.img del.img; mdel three files
3. sleep 4; cp a.img del2.img; mdel the same three files
4. cmp del.img del2.img
5. diff a.img against del.img, locating every changed byte
6. cp a.img rd.img; mdel INNER.TXT; mrd SUBDIR; diff against a.img
7. cp a.img dt.img; mdeltree SUBDIR;          diff against a.img
```

### Expected result

Comparison at step 4 identical. Step 5 to show `0xE5` in the first byte of
five root entries, zeroed FAT entries for the three deleted files, and
nothing else.

### Actual result

**1. `mdel` is deterministic.** Two deletions from the same baseline four
seconds apart produced byte-identical images.

```text
cmp del.img del2.img
  (no output)
```

**2. `mdel` changed exactly 30 bytes in a 64 MiB image.**

The root directory, before and after:

```text
before                                                    after
1  4c4f5746494c4520545854  clus 3      size 17            byte 0 -> 0xE5
2  LFN ord 0x42 LAST  cksum 0x32                          byte 0 -> 0xE5
3  LFN ord 0x01       cksum 0x32                          byte 0 -> 0xE5
4  414c4f4e47447e31545854  clus 4      size 24            byte 0 -> 0xE5
7  4849474846494c45545854  clus 67591  size 20            byte 0 -> 0xE5
                           hi 1  lo 2055
```

Of the 30 bytes: five are the first byte of a directory entry, twenty-four
are three FAT entries in **each of the two FATs**, and one is in the FSInfo
sector.

**Destroyed.** The first byte of every entry in the set, short and long
alike. For a long-name entry that byte is `LDIR_Ord`, so the ordinal and the
last-entry flag are destroyed for every component. The cluster chain of each
deleted file, zeroed in FAT 0 and in FAT 1 identically.

**Not destroyed.** `DIR_FstClusHI` and `DIR_FstClusLO`. `HIGHFILE.TXT`
retained `hi = 1, lo = 2055` after deletion, so its first cluster of 67,591
is still readable in full. Name bytes 1 to 10, `DIR_Attr`, `DIR_NTRes`, all
timestamps, `DIR_FileSize`, and the long-name checksum byte, which read
`0x32` before and after on both long-name entries.

**No data cluster was touched.** Not one of the 30 differing bytes lies in
the data region outside the root directory. File content survives deletion
in full.

**3. The first byte of a short name is recoverable, uniquely.**

The long-name checksum is computed over the eleven-byte short name. Ten
bytes survive. The map from the first byte to the checksum is a composition
of bijections — each round is a rotation followed by an addition modulo 256,
both bijective, and the first byte enters as the initial value — so exactly
one of the 256 candidates reproduces any given checksum.

Measured against the deleted entry:

```text
surviving name bytes: b'LONGD~1TXT'
target checksum:      0x32
candidates:           ['0x41']  ->  'A'
```

One candidate, and it is the correct one. The original name was
`ALONGD~1.TXT`.

Two candidate values can never be correct, which makes the recovery
self-checking: `0x00` is the terminator and `0xE5` is never stored
literally, being escaped to `0x05`. A recovery producing either proves the
long-name set does not belong to that short entry. A recovery producing
`0x05` means the real first character is `0xE5`.

**4. A deleted long-name entry reads as a valid last entry.**

`0xE5` is `1110 0101`, and bit 6 is the last-entry flag. After deletion,
`LDIR_Ord & 0x40` is therefore non-zero on **every** component of the set,
and `LDIR_Ord & ~0x40` is `0xA5`, or 165. Both values are artefacts of
deletion and neither is evidence of anything. The order of a multi-entry
long name must be reconstructed from position, never read from the ordinal.

**5. The first character of the long name survives in the entry adjacent to
the short entry, not in the one flagged last.**

Long-name entries are stored in reverse. The entry carrying
`LAST_LONG_ENTRY` holds the final fragment and sits at the lowest offset;
the ordinal-1 entry holds characters 1 to 13 and sits immediately before the
short entry. Measured on `a long deleted name.txt`:

```text
slot 2  ord 0x42 LAST  name1 = 640020006e0061006d00   "d nam"
slot 3  ord 0x01       name1 = 610020006c006f006e00   "a lon"
slot 4  ALONGD~1TXT
```

Deletion overwrites byte 0 only, so `LDIR_Name1` at bytes 1 to 10 is intact
in both. The first character of the name is in slot 3, the entry the
specification numbers 1 and does not flag as last.

**6. `mrd` and `mdeltree` are byte-identical in effect.**

Both changed the same 19 bytes: the first byte of the directory's entry in
the root, the cluster chains of the directory and of the file it contained
in both FATs, one byte in the FSInfo sector, and the first byte of the
contained file's entry inside the directory's own cluster.

The directory's cluster is otherwise untouched. Bytes 0 to 63 of cluster 5,
which hold the `.` and `..` entries, are unchanged. A deleted subdirectory
is therefore identifiable from two independent places: its entry in the
parent, and its own cluster's dot entries.

**7. Deletion updates the FSInfo sector.**

Not predicted. `BPB_FSInfo` names sector 1, and `FSI_Free_Count` at offset
488 within it moved by exactly the number of clusters freed:

```text
                       FSI_Free_Count   FSI_Nxt_Free
baseline                       61,432         67,591
after mdel of three files      61,435         67,591
after removing SUBDIR          61,434         67,591
```

`FSI_Nxt_Free` did not move in either case.

This is a volume-level record that deletion occurred, independent of the
directory entries, and it is not mentioned in any of the project's prior
research. It is a count of free clusters and not a count of deletions, so it
establishes that clusters were freed rather than how or when.

### Conclusion

`mdel`, `mrd` and `mdeltree` are deterministic under the exports
`scripts/generate-fixtures.sh` already sets. Deleted-entry fixtures may
therefore be added to the fixture set without weakening the property
EXP-0001 established, and `MANIFEST.sha256` remains a golden result.

Deletion by `mtools` destroys the first byte of each entry and the cluster
chain, and nothing else. Every field M6 needs in order to identify a deleted
entry survives: the attribute that distinguishes a long-name component from
a short entry, the checksum that re-associates a set with its short entry,
the ten name bytes that make the destroyed byte recoverable, and both words
of the first cluster.

The high word of the first cluster is **not** zeroed. This is the finding
that constrains the fixture set. A fixture built with `mtools` cannot
exercise the case where a deleted file above cluster 65,535 has lost its
start location, because `mtools` does not produce that case. Reaching it
requires poking an image under ADR-0006 §5.1, and until that fixture exists
the case is untested.

Any first character recovered by the checksum method is `RECONSTRUCTED`
under ADR-0003 §3 and never `VERIFIED`. The arithmetic is exact, but it
rests on two assumptions the evidence does not establish: that the
long-name set belongs to the short entry that follows it, and that the ten
surviving name bytes are the original ones. The uniqueness of the result
must not be reported as certainty about the name.

### Limitations

1. Measured on one host with `mtools` 4.0.43-1build1 and `mkfs.fat` 4.2.
   Whether other versions delete identically is not established. EXP-0002
   Limitation 2 applies unchanged.
2. **What `mtools` destroys is not what every implementation destroys.**
   This experiment measures the tool that builds the fixtures. It does not
   measure Windows, and it cannot: the high-word question is a question
   about Microsoft implementations, and no `mtools` measurement can answer
   it. What is established is what a fixture built here can and cannot
   prove.
3. One cluster size only, 512 bytes at one sector per cluster. A larger
   cluster changes how many entries a directory cluster holds and therefore
   where residue can appear, but not what deletion writes.
4. Deletion from the root directory and from one subdirectory. Deletion
   from a deeply nested directory was not measured.
5. `SUBDIR` was emptied before `mrd`. Whether `mrd` refuses a non-empty
   directory was not tested, because `mdeltree` covers that case and
   produced an identical result.

### Next action

1. Close the `mdel` determinism entry in `KNOWN_ISSUES.md` and mark EXP-0002
   Next action 4 done.
2. Record as a known issue that no fixture can exercise a deleted file whose
   first-cluster high word has been zeroed, and that the case is therefore
   untested against evidence a formatting tool produced.
3. Decide the M6 design against these measurements and record it.
4. Build the M6 fixtures, and confirm the fixture count byte-identical
   across runs under `DEVELOPMENT_ENVIRONMENT.md` §12.

---

## EXP-0004: Producing a fragmented deleted file with mtools alone

**Date:** 2026-09-14
**Milestone:** M9 (ADR-0002 §8), prerequisite
**Related:** ADR-0002 §8; ADR-0003 §3.2, §4.7; ADR-0006 §5.1, §5.2, §6;
ADR-0010 Decisions C and E; ADR-0013 Appendix A, Appendix C.8; EXP-0003;
`CLAUDE.md` §26; `PROJECT.md` §6.5; `KNOWN_ISSUES.md`

---

### Question

`KNOWN_ISSUES.md` states that no fixture exercises a fragmented deleted
file, and gives as the reason that `mcopy` writes a small file into
contiguous clusters and offers no way to ask otherwise. It concludes that
producing one requires poking an image under ADR-0006 §5.1.

Two questions follow.

1. Can a fragmented deleted file be produced using only `mkfs.vfat`,
   `mcopy` and `mdel`, with no byte of the image written by this project?
2. If it can, does the resulting volume produce the tool's characteristic
   false positive — an implied run that reads as entirely free, and a
   digest that is plausible and wrong?

### Why it matters

`CLAUDE.md` §26 requires recovery testing to measure incorrect recovery as
well as successful recovery, and states that a system producing many
plausible but incorrect files must not be considered accurate. Nothing in
the tree measures it. ADR-0013 Appendix C.8 records that M8's reference
validation does not close this, because it detects a wrong recovery only
where the operator already holds the right answer.

M9 assigns a confidence level. A level assigned over a fixture set in which
every extraction is correct is a name with no measured failure behind it.
The fragmented case is the one case where FAT32 retains no evidence of the
failure: EXP-0003 measured that deletion zeroes the cluster chain, so the
run is inferred from the first cluster and the size alone, and a run whose
intervening clusters have since been freed passes the allocation check.

Whether the fixture can be built without poking decides whether this is
cheap work or a licence-and-provenance problem. ADR-0006 §5.1 rejects
hand-built structure on the grounds that a fixture encoding this project's
own reading of the specification fails in the same direction as the parser,
and the test passes while the code is wrong. A fragmented fixture produced
by `mtools` is not subject to that objection; one produced by poking a FAT
chain is.

### Hypothesis

Stated before measuring, and scored below.

1. `mcopy` has no flag requesting fragmentation. Expected true.
2. Freeing a gap and then writing a file larger than it will fragment that
   file across the gap and the tail. Expected true, moderate confidence.
3. If the only free space remaining is scattered single clusters, a
   multi-cluster file must fragment, because FAT allocation is per cluster
   and cannot refuse. Expected true, moderate confidence.
4. A volume built that way, with the fragmented file and the clusters
   between its fragments all deleted, yields an implied run that reads as
   entirely free and a digest that differs from the file's own.

### Input

A 64 MiB image matching the geometry of every other fixture in the set:
131,072 sectors of 512 bytes, one partition at LBA 2048, FAT32 formatted
over the remainder, one sector per cluster.

Seven single-cluster files `S0.BIN` to `S6.BIN` take clusters 3 to 9. A
filler file consumes every remaining cluster. `S1.BIN`, `S3.BIN` and
`S5.BIN` are deleted, freeing clusters 4, 6 and 8. A 1,536-byte file
`FRAG.BIN` is then written, needing three clusters where only three
scattered single clusters remain. Finally `S2.BIN` and `S4.BIN` — the
clusters between the fragments — and `FRAG.BIN` itself are deleted.

Every file's content names the file it belongs to on each line, so a
recovered cluster can be traced to its source by reading it.

### Environment

Measured off the development machine, in an Ubuntu 24.04 container. Tool
versions:

```text
mtools            4.0.43-1build1
dosfstools        4.2
sfdisk            util-linux 2.39.3
```

`mtools` and `dosfstools` are the versions ADR-0006 and EXPERIMENTS.md
record. `sfdisk` is the same upstream version at a different distribution
patch level. No Rust toolchain was present, which bounds what this record
establishes: see Limitations 1.

### Method

`SOURCE_DATE_EPOCH=1577836800`, `TZ=UTC`, `MTOOLS_SKIP_CHECK=1`, as
ADR-0006 §6 Condition 1 requires.

The number of clusters the filler must consume is read from `minfo`, which
prints `free clusters=` followed by a plain integer. `mdir` was used first
and rejected: it reports free space in locale-formatted digit groups, which
would make fixture generation depend on the generating machine's locale.

The generator was run twice into separate directories and the images
compared byte for byte, which is the standard `verify-fixtures.sh` applies.

`mshowfat`, which is part of `mtools` and therefore adds no dependency, was
used to read the cluster chain of `FRAG.BIN` before it was deleted.

### Expected result

Hypothesis 2 predicted fragmentation across the freed gap. Hypothesis 3
predicted fragmentation under a wrapped allocation search.

### Actual result

**Hypothesis 1 holds.** `mcopy` has no fragmentation option.

**Hypothesis 2 is false.** Writing three files, deleting the middle one to
free eight clusters, then writing a sixteen-cluster file allocated clusters
13 to 28 — contiguous, immediately after the surviving file, leaving
clusters 4 to 11 free and unused.

The mechanism was then measured rather than inferred. `minfo` on that image
reports:

```text
free clusters=126987
last allocated cluster=28
```

`mtools` maintains the FAT32 FSINFO next-free hint in the image and starts
each allocation search from it, so a cluster freed behind the hint is not
reissued until the search wraps. The claim in `KNOWN_ISSUES.md` is
therefore correct for the case it describes, and correct for the wrong
reason: the obstacle is not that `mcopy` writes small files contiguously,
it is that the free-cluster search does not go backwards.

**Hypothesis 3 holds.** With the tail exhausted and only clusters 4, 6 and
8 free, `mcopy` wrote a three-cluster file across all three. `mshowfat`
reports:

```text
::/FRAG.BIN <4> <6> <8>
```

**Hypothesis 4 holds.** After deleting `S2.BIN`, `S4.BIN` and `FRAG.BIN`,
the FAT entries for clusters 4 to 8 are all zero, so nothing records that
the file was ever fragmented. `FRAG.BIN`'s directory entry survives with a
first cluster of 4 and a size of 1,536, which implies the contiguous run 4
to 6. Every cluster of that run reads as free.

The three clusters of that implied run hold, in order, the first third of
`FRAG.BIN`, the whole of the deleted `S2.BIN`, and the second third of
`FRAG.BIN`. Read as text, cluster 5 begins:

```text
taphonomy spacer2 block 000000
```

The remaining third of `FRAG.BIN` sits in cluster 8, which the deleted
`S5.BIN` entry names as its own first cluster.

**Determinism.** Two independent runs produced byte-identical images.

```text
52d92a825c375c2f296b6ee7ab5b6787ea801accfd3634f87d750139006dac20
```

**Digests.**

| Object | SHA-256 |
| --- | --- |
| `fat32-fragmented-deleted.img` | `52d92a825c375c2f296b6ee7ab5b6787ea801accfd3634f87d750139006dac20` |
| `FRAG.BIN` content, as written | `a27a7e9556749147a521e0f133a9a1a84375861885f883b404c125f4719f3bfd` |
| Clusters 4, 5 and 6, first 1,536 bytes | `7eef746ae1c9b211bf0384deec32abf8f91eb024f49c59f8478bc617db342914` |

The second and third differ. That difference is the measurement `CLAUDE.md`
§26 asks for and the tree has never made.

**Build cost.** 0.70 seconds against 0.22 for a volume with no filler. The
image is not sparse, so it occupies 64 MiB on disk where the existing
fixtures occupy far less.

### Conclusion

A fragmented deleted file can be produced with `mkfs.vfat`, `mcopy` and
`mdel` alone. No byte of the image is written by this project, so ADR-0006
§5.1's objection does not apply and the narrow deliberate-corruption
exception is not needed.

The route is not the obvious one. Freeing a gap achieves nothing because
the FSINFO next-free hint does not search backwards. The volume must be
filled so that the search wraps, at which point scattered single clusters
are the only space available and a multi-cluster file has no contiguous
option.

`KNOWN_ISSUES.md`'s statement that producing such a fixture requires poking
an image under ADR-0006 §5.1 is superseded by this measurement. A comment
in `scripts/generate-fixtures.sh` above `fixture_fat32_recover_collision`
states that `mtools` allocates forward and never reissues a freed cluster.
That is true only while the search has not wrapped, and the general form of
the claim is false. Whether the collision fixture could now be built
without its poke is a separate question and is not answered here.

### Limitations

1. **The recovered digest is predicted, not measured.** No Rust toolchain
   was available in the measuring environment, so the tool was not run
   against this image. `7eef746a…` is the digest of the first 1,536 bytes
   of clusters 4, 5 and 6, computed independently. It is what `extract` is
   expected to produce given `assess` classifies the run as recoverable,
   read from `src/fat_recovery.rs` at `4adfda3`. Confirming it against the
   binary is the first item under Next action, and until that is done this
   record predicts the tool's behaviour rather than reporting it.

2. **Cross-machine determinism is unverified.** Two runs on one machine
   produced identical images. Whether the same procedure produces the same
   digest on the development machine has not been measured, and the
   `sfdisk` patch level differs. `verify-fixtures.sh` measures determinism
   within a machine and not across machines, so the image digest above may
   not reproduce and must be re-measured before anything asserts it.

3. **One arrangement, not a class.** This is a single fragmented file with
   one deleted single-cluster file between two of its fragments. It is
   Case 3 of the canonical list in the external work cited below. Cases
   where an active file sits between fragments, where a file wraps from
   the end of the volume to the beginning, and where fragments are out of
   logical order are not built here.

4. **The filler is a single repeated byte.** Its content is never
   recovered because it stays allocated, but a fixture whose bulk is
   uniform will not reveal a fault that depends on distinguishing filler
   clusters from one another.

5. **The fixture depends on exhausting the volume.** Anything that changes
   the geometry, the cluster size or `mkfs.vfat`'s layout changes how many
   clusters the filler must consume, and the arrangement must be
   re-verified with `mshowfat` rather than assumed.

### External practice

Researched after the measurements, and consulted in the published sources
rather than through a summary.

NIST's CFTT *Active File Identification & Deleted File Recovery Tool
Specification*, Draft 1 of Version 1.1, defines the requirement this case
breaches. It is not a core feature. `DFR-CR-04` requires a Recovered Object
to consist only of blocks from the Deleted Block Pool, and the
specification defines that pool as blocks that were part of an FS-Object,
were deleted, and have not been reallocated or reused. Cluster 5 satisfies
all three, so it is in the pool and `DFR-CR-04` is not breached.

The requirements that are breached sit in the optional section, which
applies because the specification's definition of Estimated Content covers
what this tool does — recovering beyond what residual metadata explicitly
identifies, as ADR-0013 Appendix A already records:

* `DFR-RO-05` requires that estimated content consist only of blocks
  allocated to the original object. Cluster 5 never was.
* `DFR-RO-04` requires that each recovered block be assigned to no more
  than one Recovered Object. In this fixture cluster 5 falls in the implied
  run of the fragmented entry and is also the whole of another deleted
  entry's run, and cluster 6 likewise.
* `DFR-RO-08` requires that estimated content replace blocks allocated
  since deletion with benign data of the same length. ADR-0010 Decision C
  refuses instead. That is a third deliberate non-conformance, distinct
  from the `DFR-CR-02` one recorded in ADR-0013 Appendix A.

The specification's scope also bears on the method. It restricts testing to
files created and deleted as an end user would, and excludes file system
metadata specifically corrupted, modified or otherwise manipulated. The
allocation-pressure method satisfies that. `mtools` ships `mdoctorfat`,
which changes the clusters allocated to a file and performs no consistency
check, and which would have produced this fixture in one command; it is
excluded by the specification's scope for the same reason ADR-0006 §5.1
excludes it.

Meyer and Roy, *Do Metadata-based Deleted-File-Recovery (DFR) Tools Meet
NIST Guidelines?*, EAI Endorsed Transactions on Security and Safety, 2020,
build a canonical list of test images in which this arrangement is Case 3.
They report that where the space between fragments is unallocated, every
tool they tested recovered the file as though it were contiguous and pulled
in data that was not the file's: Autopsy, Recuva, FTK Imager, TestDisk and
Magnet AXIOM. Two points follow. This failure is the field's, not this
project's alone, and M9 must not claim more about a recovery than tools
that all fail here. And their attribution of the failure to core feature 4
does not survive the specification's own definition of the Deleted Block
Pool, which is why `DFR-RO-05` is cited above instead.

Their Case 2, an active file between the fragments, corroborates ADR-0010
Decision C from the other side: Recuva and Magnet AXIOM fail because they
do not refuse on an allocated cluster, and this tool does refuse.

Their images were built on a mounted filesystem, where Linux rescans the
FAT rather than trusting FSINFO, which is why the simple gap method works
there. ADR-0006 §5.2 rejects mounting, so that route is not open here and
the wrap is required.

### Next action

1. Run the tool against the fixture on the development machine and compare
   the reported digest against `7eef746a…`. Score Limitation 1.
2. Re-measure the image digest on the development machine and record
   whether it matches `52d92a82…`. Score Limitation 2.
3. Add the fixture to `scripts/generate-fixtures.sh`, with disk signature
   `0xfa73000b`, and to `fixtures/partition/MANIFEST.sha256` as a separate
   commit.
4. Add an integration test asserting that the extraction succeeds, that its
   digest is not `FRAG.BIN`'s, and that the entry reports every cluster
   free. A test that the tool recovers correctly here would be asserting
   the wrong thing.
5. Correct the `KNOWN_ISSUES.md` entry and the
   `fixture_fat32_recover_collision` comment, each in its own commit.
6. Decide, in M9, how a confidence level is reported for an entry of this
   shape, given `DFR-RO-04`, `DFR-RO-05` and `DFR-RO-08`.

### Appendix A: Limitations discharged, and three corrections (2026-09-15)

This record was committed with two open limitations and one attribution it
did not know it owed. All three are settled here. The findings above are
not rewritten.

#### A.1 Both limitations are discharged

**Limitation 1, the predicted digest.** The tool was run against the
fixture on the development machine. The entry at `c2 s2` reports
`run 4-6, 1536 bytes, 0 slack, every cluster free` and
`recovered sha256 7eef746a…`, which is the digest this record predicted
from reading `src/fat_recovery.rs`. The four single-cluster entries report
`43a3bb6a…`, `5f91be13…`, `7a37c8fd…` and `06e933a3…`, also as predicted.
The record no longer predicts the tool's behaviour; it reports it.

**Limitation 2, cross-machine determinism.** `scripts/generate-fixtures.sh`
was run on the development machine and produced
`52d92a825c375c2f296b6ee7ab5b6787ea801accfd3634f87d750139006dac20`, equal
to the digest recorded above and measured elsewhere. The eighteen existing
fixtures were unchanged, and `scripts/verify-fixtures.sh` reported
`Deterministic: 19/19 fixtures byte-identical across runs`. The image
digest is measured rather than provisional.

#### A.2 The result this record did not anticipate

The body predicted that the fragmented entry would recover incorrectly. It
did not work out what happens to the other four deleted entries, and that
is the measurement worth having.

Slot order fixes which file each entry was. `FRAG.BIN` took the directory
slot `S1.BIN` freed, so the five deleted entries are `FRAG.BIN`, `S2.BIN`,
`S3.BIN`, `S4.BIN` and `S5.BIN` in slots 2 to 6.

| Slot | Was | Implied run | Recovers | Correct |
| --- | --- | --- | --- | --- |
| 2 | `FRAG.BIN` | 4 to 6 | frag, `S2.BIN`, frag | no |
| 3 | `S2.BIN` | 5 | its own cluster | yes |
| 4 | `S3.BIN` | 6 | `FRAG.BIN`'s second third | no |
| 5 | `S4.BIN` | 7 | its own cluster | yes |
| 6 | `S5.BIN` | 8 | `FRAG.BIN`'s final third | no |

Three of five recoveries are incorrect. Every one of the five reports
identically — every cluster free, zero slack, a digest, no refusal — and
nothing in the output separates the two that are right from the three that
are wrong. This is the first time the tree has carried a measured
incorrect-recovery rate, which `CLAUDE.md` §26 requires and no fixture
before this one could produce.

Two consequences follow that the body stated only as expectations.

`DFR-RO-04` is breached visibly rather than by argument. Cluster 5 lies in
slot 2's implied run and is the whole of slot 3's; cluster 6 lies in slot
2's run and is the whole of slot 4's. Two clusters are each recovered into
two different objects in one run of the tool.

`ADR-0010` §10.2 is falsified by an artefact in the tree rather than in
principle. It states that a fixture built with `mtools` alone cannot
contain a deleted entry whose clusters have been reused. Slots 4 and 6 are
exactly that. `ADR-0010` Appendix D records this.

#### A.3 The method is NIST's, and this record should have said so

The body presents the fill-and-wrap route as something this session
worked out. It is a rediscovery. NIST's CFTT published the technique,
under the name Forced Overwrite, as: create a desired block layout;
allocate all remaining free blocks to one large file; delete one or more
files; create one or more files, which must then overwrite the deleted
ones because no other free blocks remain. CFTT ships a tool named
`fill-fs` for the second step.

The same source states the consequence this fixture depends on: some of
the overwritten blocks end up referenced by the metadata of both a deleted
and an active file, and deleting the active file leaves a block referenced
by two deleted files. That is clusters 5 and 6 above. The property this
fixture was built to exhibit is the property the published construction is
designed to produce, which is a stronger footing than the body claims for
it.

Source: Jim Lyle, *Creating Deleted File Recovery Tool Testing Images*,
NIST/CFTT, presented at AAFS, February 2012.

#### A.4 A limitation the same source surfaces

CFTT's first requirement for a recovery test image is that every sector be
initialised uniquely, so that after formatting, anything not carrying the
initialisation pattern is metadata, and any block appearing in a recovered
object can be traced to its origin.

This project's fixtures are built over zeroed images, and this fixture's
filler is a single repeated byte. It does not weaken the measurements
above: the cluster that matters, cluster 5, carries text naming the file it
belonged to, which is why the composition assertions in
`tests/fat32_recovery_fixtures.rs` can name it. But it is a standing
limitation of the fixture laboratory rather than of this image, and a
future fixture whose failure depends on distinguishing two filler clusters
would not be diagnosable.

#### A.5 The refusal case is also producible without a poke

Measured 2026-09-15, in the same environment as the body. The construction
above, stopped one step earlier — deleting only `FRAG.BIN` and leaving
`S2.BIN` and `S4.BIN` live — produces a deleted entry whose implied run
reaches a cluster allocated to a live file:

```text
slot 2  deleted  first=4  size=1536  implied=[4, 5, 6]  allocated=[5]
```

Two builds were byte-identical at
`5f30fc3474f2ae0963ca2ad98c9791a687211997606b493c74ac21ef5078ddda`. This
is the shape `fixtures/partition/fat32-recover-collision.img` uses a poke
to create. It does not follow that the collision fixture should be rebuilt;
`ADR-0010` Appendix D gives the reason it should not.

The image was not added to the fixture set. It is NIST's Case 2, an active
file between the fragments, and building it is a candidate for future work
rather than part of this one.

#### A.6 What this appendix does not change

The construction, the digests, the FSINFO mechanism and the falsification
of `KNOWN_ISSUES.md`'s claim all stand as written. Limitations 3, 4 and 5
of the body remain open: this is one arrangement, not the class.

### Appendix B: the refusal case becomes a fixture (2026-09-20)

Appendix A.5 measured a construction and did not add it to the fixture set.
It is now `fixtures/partition/fat32-fragmented-live-gap.img`. This appendix
records what that took, what the tool reports on it, and which of the
body's limitations it discharges. The body and Appendix A are not
rewritten.

#### B.1 The construction is a fixture, and its digest reproduced

`scripts/generate-fixtures.sh` gained the fixture at `3f6e7ef`, with disk
signature `0xfa73000c`, and `fixtures/partition/MANIFEST.sha256` recorded
it at `0b998f7`.

The two fragmentation fixtures now share `build_fragmented_volume`, which
ends with `FRAG.BIN` written and its chain asserted. Each fixture then
performs its own deletions, which is the division `build_recovery_volume`
already uses and is where the two volumes diverge. The refactor is
behaviour-preserving: all nineteen existing fixtures reproduce the digests
`MANIFEST.sha256` already recorded, which covers the output of `sfdisk`,
`sgdisk`, `mkfs.vfat` and `mtools` at once.

The fixture's digest is
`5f30fc3474f2ae0963ca2ad98c9791a687211997606b493c74ac21ef5078ddda`, equal
to the digest Appendix A.5 recorded. It was measured on the development
machine and, before delivery, in a second Ubuntu 24.04 environment running
`mtools` 4.0.43-1build1, `dosfstools` 4.2-1.1build1, `sfdisk` from
util-linux 2.39.3 and `sgdisk` 1.0.10. `scripts/verify-fixtures.sh` reports
`Deterministic: 20/20 fixtures byte-identical across runs`.

**Limitation 2 is therefore discharged for this construction as well.** A.1
discharged it for `fat32-fragmented-deleted.img` only. The digest recorded
in A.5, taken off the development machine, has now been reproduced on the
development machine and in a third environment.

#### B.2 What the tool reports on it

Measured at `421272d`, with no arguments:

```text
coverage     complete
  volumes analysed           1
unrecovered  1 refused, 2 not read
```

The volume carries three deleted entries rather than the five of
`fat32-fragmented-deleted.img`, because `S2.BIN` and `S4.BIN` are never
deleted:

| Slot | Was | Implied run | Cluster state | Reported |
| --- | --- | --- | --- | --- |
| 2 | `FRAG.BIN` | 4 to 6, 1536 bytes | 5 held by live `S2.BIN` | refused |
| 4 | `S3.BIN` | 6, 512 bytes | free | recovers `5f91be13…` |
| 6 | `S5.BIN` | 8, 512 bytes | free | recovers `06e933a3…` |

Those two digests are two of the four A.1 recorded on the volume above, and
both clusters hold `FRAG.BIN`'s content rather than the entry's own: cluster
6 is its second third and cluster 8 its last. So this volume measures the
refusal and the overwritten-entry case together.

Its root holds no directory, so it carries no gap of `ADR-0014` Appendix
B.2's kind and its coverage is `complete`. That is a statement about reach
alone. Two of its three deleted entries would still recover another file's
bytes, which is the separation `ADR-0014` Appendix B.6 named the status for.

`tests/fat32_recovery_fixtures.rs` asserts the layout and the refusal at
`421272d`. It does not assert slots 4 and 6, because
`three_of_the_five_recoveries_are_not_the_entrys_own_content` already
asserts that behaviour on the sibling volume, and a second assertion of the
same fact against a different image illustrates rather than proves.

#### B.3 Limitation 3, and what remains of it

The body's Limitation 3 named three arrangements it did not build. One is
now built:

* **An active file between the fragments.** Built. It is this fixture.
* **A file wrapping from the end of the volume to the beginning.** Not
  built, and nothing here establishes whether it can be.
* **Fragments out of logical order.** Blocked rather than unbuilt. B.4.

Two further arrangements were listed as unmeasured in
`docs/development/KNOWN_ISSUES.md` and were already measured by this
record. Both entries were corrected at `b4a53bc`:

* **More than two fragments.** `FRAG.BIN` occupies clusters 4, 6 and 8,
  which the body records as `::/FRAG.BIN <4> <6> <8>` and which the
  generator asserts on every run. That is three fragments.
* **Overwritten files.** Appendix A.2's slots 4 and 6 are deleted entries
  whose cluster was reallocated to `FRAG.BIN` and never freed again.

Limitations 4 and 5 of the body remain open, unchanged.

#### B.4 Fragments out of logical order have no route here

The body measured that `mtools` starts each free-cluster search from the
FAT32 FSINFO next-free hint, and that `FRAG.BIN` took clusters 4, 6 and 8
in ascending order once the search had wrapped. A forward search that does
not go backwards cannot issue a later cluster before an earlier one, so a
file whose logical fragment order runs backwards has no allocation path
under this method. That is one measurement and a mechanism consistent with
it; no route to a descending order has been identified.

Both alternatives are closed by existing decisions rather than by
difficulty. `ADR-0006` §5.2 rejects mounting, which is how Meyer and Roy
built their images and why the simple gap method works for them. A poked
FAT chain, or `mdoctorfat`, falls under `ADR-0006` §5.1's objection to
hand-built structure, and the body's external practice section records that
the CFTT specification's own scope excludes file system metadata that has
been corrupted, modified or otherwise manipulated — so an image built that
way would not measure the test case it was built for.

This is recorded as a limit of the fixture laboratory, not as pending work.

#### B.5 What the two images differ in, measured

The images differ in twenty bytes: the MBR disk signature, the first byte
of two directory entries, two cluster chains in each of the two FATs, and
the FSINFO free-cluster count. Semantically the only difference is whether
two `mdel` calls ran.

That does not give the single-field control `ADR-0010` Appendix D.3
requires of `fat32-recover-collision.img`, and D.3's conclusion stands
unchanged. What it does give is a controlled pair for the allocation state
of the clusters between the fragments: the same volume, the same file in
the same three clusters, refused in one image and extracted in the other.

#### B.6 An attribution in Appendix A.5

A.5 describes the arrangement as "NIST's Case 2". The numbering is Meyer
and Roy's, from the canonical list the body's external practice section
cites: their Case 2 is an active file between the fragments and their
Case 3 is the unallocated-gap case the body built. Nothing read for this
record numbers CFTT's own test cases that way. The fixture's tests and
`KNOWN_ISSUES.md` attribute the numbering to Meyer and Roy.

#### B.7 What this appendix does not change

The construction, the digests, the FSINFO mechanism, Appendix A.2's table
and Appendix A.3's attribution of the method to NIST's Forced Overwrite all
stand as written. `ADR-0010` Appendix D stands in full. No finding here
concerns what the tool recovers, refuses or validates, and none of it
changes a decision.

---

## EXP-0005: What survives inside a deleted FAT32 directory

**Date:** 2026-09-22
**Milestone:** prerequisite to subdirectory traversal, whose ADR is not yet
written
**Related:** ADR-0006 §5.1, §6; ADR-0014 Appendix B.2; ADR-0015 §12;
EXP-0003; EXP-0004; `KNOWN_ISSUES.md`

---

### Question

1. When `mdeltree` deletes a directory, what survives inside it: its own
   `.` and `..` entries, the entries of the files it held, and a directory
   nested within it?
2. Where a deleted directory occupied more than one cluster, can its later
   clusters be located from the evidence?
3. What distinguishes a deleted directory's first cluster from other bytes?
4. What does the tool report on such a volume today?

### Why it matters

`ADR-0014` Appendix B.2 records the tool's non-conformance with DFR-CR-01:
it lists a directory and does not read it, so deleted entries inside a
directory are not identified, and on a DCF camera card no image file sits in
the root. Every directory in the current fixture set is empty: `/logs` is
made with `mmd`, `/gone` is removed with `mrd`, and the generator copies no
file into any subdirectory. Reading subdirectories against today's fixtures
would close the reported gap and find nothing.

`ADR-0015` §12 states that the measurements for reading subdirectories are
recorded in this document. At the time only EXP-0003 finding 6 was: `.` and
`..` survive `mrd` and `mdeltree` in one directory holding one file. The
rest existed in a session handover, which is not tracked. This record makes
them tracked.

### Hypothesis

Stated before the volumes below were built, from measurements on a bare
volume taken earlier the same day and not recorded:

1. `mdeltree` marks each contained entry deleted by its first byte and
   leaves the rest of every directory cluster intact, `.` and `..` included.
2. `..` in a directory whose parent is the root holds cluster 0.
3. A directory that grows after other files were allocated takes a second
   cluster that is not adjacent to its first, and once the chain is zeroed
   nothing in the volume locates that second cluster.
4. Allocation: `GONE` at 3 and `DEEP` at 6; `BIG` at 3 then 20, `PAYLOAD.BIN`
   at 4 to 18, `TAIL` at 19.
5. The tool reports each volume as incompletely covered, with one directory
   not read, no unrecovered entry, and exit status 0.

The session handover recorded the split directory at clusters 3 and 19, from
a construction it did not record. It is not reproduced here and this record
supersedes it.

### Input

Two volumes with the geometry of every fixture in the set: 131,072 sectors
of 512 bytes, one partition at LBA 2048, FAT32 over the remainder, one
sector per cluster. Every file's content is its own name.

`subtree.img` holds a directory `GONE` containing `ALPHA.TXT`, `BETA.TXT`
and a directory `DEEP` containing `GAMMA.TXT`. `GONE` is then removed with
`mdeltree`.

`split.img` holds a directory `BIG`, then a 7,680-byte `PAYLOAD.BIN` in the
root, fifteen clusters, then fourteen empty files in `BIG`, which with `.`
and `..` fill all sixteen slots of its first cluster, then a directory
`BIG/TAIL` containing `OMEGA.TXT`. `BIG` is then removed with `mdeltree`.

### Environment

Two environments, with the same package versions measured by
`dpkg-query`:

```text
mtools       4.0.43-1build1
dosfstools   4.2-1.1build1
util-linux   2.39.3-9ubuntu6.6
```

An Ubuntu 24.04.4 container off the development machine, and the
development machine itself. The tool was run on the development machine
only, at `e9d0037`.

### Method

`SOURCE_DATE_EPOCH=1577836800`, `TZ=UTC` and `MTOOLS_SKIP_CHECK=1`, as
`ADR-0006` §6 Condition 1 requires. The partition table is written by
`sfdisk` and the volume by
`mkfs.vfat --invariant --mbr=n -F 32 -n TAPHFIX --offset=2048`, the
generator's own invocation, with disk signatures `0xfa7300e1` and
`0xfa7300e2`. With `V` the image at byte offset 1,048,576:

```text
subtree.img   mmd ::/GONE
              mcopy alpha ::/GONE/ALPHA.TXT
              mcopy beta  ::/GONE/BETA.TXT
              mmd ::/GONE/DEEP
              mcopy gamma ::/GONE/DEEP/GAMMA.TXT
              mdeltree ::/GONE

split.img     mmd ::/BIG
              mcopy payload ::/PAYLOAD.BIN
              mcopy empty ::/BIG/E01.TXT  ... ::/BIG/E14.TXT
              mmd ::/BIG/TAIL
              mcopy omega ::/BIG/TAIL/OMEGA.TXT
              mdeltree ::/BIG
```

Each volume was built twice per run and compared byte for byte, and the
whole run was repeated in each environment. `mshowfat` read every chain
before deletion. After deletion `mtools` no longer lists a deleted
directory, so its clusters were read from the image by a short reader
independent of the tool, which decodes each 32-byte slot. The tool was then
run on each image with no arguments.

### Expected result

As hypothesised: every chain as in Hypothesis 4, every contained entry
surviving with only its first byte changed, and the tool reporting one
unread directory and nothing about the four files at depth.

### Actual result

**Determinism.** Every build of each image was identical, in both
environments:

```text
325a622096263e879e887766bb6a09aa1a60d144c45ebd0926e186786d8d4e2a  subtree.img
ed53e14917494a2790f64cd0c857201fcdbb039a080adb323c14c4ff80617340  split.img
```

The tool's own digest of each image agreed.

**Chains before deletion,** exactly as predicted:

```text
::/GONE <3>                  ::/BIG <3> <20>
::/GONE/ALPHA.TXT <4>        ::/PAYLOAD.BIN <4-18>
::/GONE/BETA.TXT <5>         ::/BIG/TAIL <19>
::/GONE/DEEP <6>             ::/BIG/TAIL/OMEGA.TXT <21>
::/GONE/DEEP/GAMMA.TXT <7>
```

**After deletion,** every directory cluster's FAT entry reads 0. In every
contained entry the first byte reads `0xE5`, and the rest of the name, the
first cluster and the size survive. Other fields were not compared:

| Cluster | Held | Slots after deletion |
| --- | --- | --- |
| 3, `subtree.img` | `GONE` | `.`→3, `..`→0, deleted `ALPHA` →4 size 10, deleted `BETA` →5 size 9, deleted directory `DEEP` →6, terminator at slot 5 |
| 6, `subtree.img` | `DEEP` | `.`→6, `..`→3, deleted `GAMMA` →7 size 10, terminator at slot 3 |
| 3, `split.img` | `BIG`, first | `.`→3, `..`→0, fourteen deleted empty files at cluster 0 size 0, **no terminator** |
| 20, `split.img` | `BIG`, second | deleted directory `TAIL` →19 at slot 0, terminator at slot 1, **no `.` or `..`** |
| 19, `split.img` | `TAIL` | `.`→19, `..`→3, deleted `OMEGA` →21 size 10, terminator at slot 3 |

`.` and `..` stay unmarked in a deleted directory, as EXP-0003 finding 6
measured for one directory. `TAIL`'s `..` names cluster 3, `BIG`'s first,
not 20.

Cluster 4, the one adjacent to `BIG`'s first, read as if it continued
`BIG`: sixteen entries, none marked deleted, none a terminator, each made of
`PAYLOAD.BIN`'s text, with first clusters between 1,095,699,022 and
1,229,081,689 on a volume the tool reports as holding 127,006.

**What the tool reports.** On both images, abridged:

```text
      c2 s1     deleted       directory, cluster 3, first byte destroyed, no long name survives
      deleted content
        c2 s1     no content: a deleted directory, not a file

coverage     incomplete
  volumes analysed           1
  directories not read       1
```

Exit status 0 on both, and no `unrecovered` line.

### Conclusion

Every hypothesis held. What the measurements establish:

1. **A deleted subtree survives whole.** Four deleted files sit at depths
   one and two, each with its first cluster and size intact: `ALPHA`, `BETA`
   and `GAMMA` in `subtree.img` and `OMEGA` in `split.img`. The tool
   reports none of them, and its only indication that they exist is one
   gap line per volume.
2. **A deleted directory's first cluster is self-identifying.** Slot 0 is
   `.` naming that same cluster, and slot 1 is `..`.
3. **`..` holding 0 means the root.** The FAT specification requires it
   where the parent is the root directory. Read as a cluster number it is
   out of range, and it is not an error.
4. **A later cluster of a deleted directory is unreachable.** It carries no
   `.` or `..`, the FAT chain that named it is zeroed, and no entry anywhere
   points to it: `TAIL`'s `..` names the first cluster. Here that cluster
   holds the only entry leading to `TAIL` and `OMEGA.TXT`.
5. **The first cluster says whether the listing may continue.** `GONE`'s
   first cluster holds a terminator; `BIG`'s is full and holds none. A
   terminator ends a directory, so its presence proves the listing
   complete. Its absence proves only that the directory may have continued,
   not that it did: a directory exactly one cluster long also has none.
6. **Adjacency is not continuation.** Reading the next cluster as the
   directory's continuation returned sixteen entries that look live and
   point billions of clusters beyond the volume.

The fourteen deleted empty files have first cluster 0, which the FAT
specification sets for a zero-length file. The tool already refuses such
an entry as empty.

### Limitations

1. One cluster size. A larger cluster holds more entries, so a directory
   needs more of them before it takes a second cluster.
2. Deletion by `mdeltree` only. Deletion by another implementation is not
   measured.
3. 8.3 names only. Long-name entries inside a deleted directory are not
   measured.
4. No case where a deleted directory's first cluster was reallocated and
   overwritten, which would make item 2's check the only defence.
5. The directory clusters were decoded by a reader written for this record,
   not by the tool. What the tool's own enumeration makes of them is
   unmeasured until it reads them.

### External practice

The Microsoft FAT specification requires a `..` entry to hold its parent's
first cluster and to hold 0 where the parent is the root, sets a
zero-length file's first cluster to 0, and records that the root directory
itself holds neither `.` nor `..`.

The Sleuth Kit added recursion into deleted directories to `fls` in 2004,
behind its `-r` flag, as the Sleuth Kit Informer's fourteenth issue
records. fatcat extracts a deleted directory given its cluster number. Both
read inside a deleted directory, and this tool does not.

### Next action

1. Record the subdirectory traversal decisions in an ADR citing this
   record.
2. Add a fixture built by this construction to
   `scripts/generate-fixtures.sh`, so that the gap closes against a volume
   where reading subdirectories finds something.

---

## EXP-0006: Building the volumes ADR-0016's bounds need

**Date:** 2026-09-23
**Milestone:** M11, closing `ADR-0016` section 11 conditions 6 and 7
**Related:** `ADR-0016` Decisions E and F, section 11; `ADR-0006` §5.2, §6;
EXP-0004; EXP-0005

---

### Question

`ADR-0016` Decision E reads at most 128 levels and reads no cluster twice,
and Decision F names an artifact by its entry's own cluster and slot. No
fixture exercised any of the three.

1. Can `mtools` alone nest directories deeper than 128 levels?
2. Can `mtools` alone produce a directory entry naming an ancestor?
3. Can `mtools` alone produce two deleted entries at the same slot of
   different directories naming the same first cluster?
4. What does the tool report on each volume once built?

### Why it matters

A bound that no fixture reaches is a bound nobody has seen work. `ADR-0006`
§5.2 prefers a fixture built by ordinary tools, and permits a poke where no
tool produces the arrangement, so which of these need poking decides how
the fixtures are built and what the poke has to be checked against.

### Hypothesis

1. `mmd` nests without a depth limit of its own.
2. No `mtools` command writes an entry naming an ancestor: every one of them
   maintains the tree.
3. `mtools` reuses a freed cluster, so deleting a file and writing another
   would put two entries on the same cluster without a poke.

### Environment

An Ubuntu 24.04.4 container off the development machine: `mtools`
4.0.43-1build1, `dosfstools` 4.2-1.1build1, util-linux 2.39.3-9ubuntu6.6.
The fixture digests and the tool's output were measured on the development
machine at `909fb6a`.

### Method

Volumes of the fixture set's geometry, built as `ADR-0006` §6 Condition 1
requires. For the depth question, `mmd` was called in a loop with one more
`/D` each time until it failed or 130 levels existed. For the reuse
question, `/A/X.TXT` was written, its chain read with `mshowfat`, deleted,
and `/B/Y.TXT` then written and its chain read. Directory clusters were
read back from the image with a reader independent of the tool.

### Expected result

As hypothesised: deep nesting builds, the loop needs a poke, and the
collision does not.

### Actual result

**1. Depth: 130 levels built with `mmd` alone.** The deepest path is 262
characters, each level takes one cluster, and the deepest is cluster 132.

**2. The loop needs a poke,** as expected. `mmd` maintains the tree.

**3. `mtools` does not reuse a freed cluster.** `/A/X.TXT` took cluster 5.
After `mdel`, `/B/Y.TXT` took cluster **6**, not 5. EXP-0004 recorded reuse
only after the allocator wraps the volume, which at 127,006 clusters no
fixture will reach. The hypothesis was wrong, and the collision fixture
therefore needs a poke as well.

**4. Both pokes are one byte** in a directory entry's first-cluster low
word, written with `poke_expecting` against the value the entry already
holds, at an offset computed from the volume's own BIOS parameter block:

```text
fat32-directory-loop.img     cluster 3, slot 2: 4 -> 3
fat32-slot-collision.img     cluster 4, slot 2: 6 -> 5
```

**5. The three fixtures are deterministic.** `verify-fixtures.sh` reports
25 of 25 byte-identical across runs, and the 22 fixtures that existed
before are unchanged.

**6. What the tool reports,** measured at `909fb6a`:

| Fixture | Reported |
| --- | --- |
| `fat32-directory-loop.img` | `/A` listed once; `/A/B` not read, because cluster 3 was already read; one gap; incomplete |
| `fat32-nested-directories.img` | 128 levels read, the deepest at cluster 130; the 129th not read, deeper than the bound; **one** gap; incomplete |
| `fat32-slot-collision.img` | both entries recovered from cluster 5 with the same digest, written as `c3-s2-first-5.bin` and `c4-s2-first-5.bin`; coverage complete |

### Conclusion

1. Nesting past the bound needs no poke; the loop and the collision each
   need one byte, which `ADR-0006` §5.2 permits where no tool produces the
   arrangement.
2. **The depth bound leaves one gap, not one for every level beneath it.**
   The 130th level is named only inside the 129th's cluster, and that
   cluster was not read, so the 130th is never reached. A prediction of two
   gaps was made before the run and was wrong.
3. Decision F's name holds where Decision D's did not. Two entries sharing
   a slot and a first cluster are two files, and under the old name the
   second would have been reported as a file that already existed.
4. Cluster 5 is recovered twice, once for each entry, with the same digest.
   The tool does not claim the two entries are the same file, and nothing in
   the evidence says which of them the content belonged to.

### Limitations

1. One `mtools` version. Another allocator might reuse a freed cluster and
   make the collision buildable without a poke.
2. The loop built here is the shortest: an entry naming its own parent. A
   longer cycle through several directories is not measured, though the
   record of clusters read is the same for both.
3. 130 levels only, two past the bound. A tree thousands deep is not
   measured.
4. The collision was built with both entries at slot 2. Entries at
   different slots of the same cluster were already distinct under
   `ADR-0015` Decision D and are not measured here.

### Next action

1. Bring the descriptive documents up to what the tool now does.
2. Record M11 in `CHANGELOG.md`.

### Appendix A: the permission to poke is §5.1, not §5.2 (2026-09-24)

This record cites `ADR-0006` §5.2 three times, in its Related line, in Why
it matters, and in Conclusion 1, for the rule that a fixture is built by
ordinary tools and poked only where no tool produces the arrangement. That
rule is the closing paragraph of §5.1, *Construct directory entries with
the existing poke helpers*. §5.2 is *Loop device and mount*, and says
nothing about poking. Every citation should read §5.1. The body is not
rewritten; what the poke built and measured is unchanged.

---

## EXP-0007: What a quick format leaves of a DCF directory tree

**Date:** 2026-09-23
**Milestone:** prerequisite to reading directories no surviving entry
names, whose ADR is not yet written
**Related:** ADR-0001 §10; ADR-0002 §8; ADR-0006 §5.1, §6; ADR-0014
Decision A, Appendix B.2; ADR-0016 Decisions A, C and D, §10; EXP-0005;
`src/fat_recovery.rs` `eligibility`

---

### Question

1. When a FAT32 volume holding a DCF-shaped tree is formatted again with
   the same geometry, what survives of its directories and the files they
   list?
2. Would a directory that survives pass `ADR-0016` Decision C's three
   checks as they stand?
3. What does the volume look like once it has been written to after the
   format, as a camera does on its next shot?
4. What does the tool report on each volume at `8504cd8`?

### Why it matters

`ADR-0016` §10 records that the tool does not find a directory no
surviving entry names. EXP-0005 measured one such case, the later cluster
of a deleted directory. A format is the other: it rewrites the boot
sector, the FATs and the root, so every directory below the root is named
by nothing. `ADR-0014` Appendix B.2 records that on a DCF camera card no
image file sits in the root, so a formatted card is one on which the tool
has nothing to walk.

Whether such a volume is within this project's scope depends on what
survives. `ADR-0001` §10 separates filesystem-aware recovery, which uses
directory records and allocation metadata, from carving; `ADR-0002` §8
leaves carving to a later decision. If the directory records survive,
reading them is the former. If they do not, only carving is left.

### Hypothesis

Stated before the volumes below were built, from measurements on a bare
volume taken earlier the same day and not recorded:

1. A quick format leaves every subdirectory's first cluster intact, its
   FAT entry 0, `.` naming itself at slot 0 and `..` at slot 1.
2. The entries of the files those directories list are not marked
   deleted, and keep their first cluster and size.
3. Every file's implied run is free and holds its original bytes.
4. Written to once more, the volume puts its new `DCIM` and `100TAPH` on
   the clusters the old ones held, and the old directories are gone.

On the bare volume the root was predicted to be empty after the format. It
held the new volume label, so that prediction was wrong; the tool finds
nothing to recover there either way.

### Input

Two volumes with the geometry of every fixture in the set: 131,072
sectors of 512 bytes, one partition at LBA 2048, FAT32 over the remainder,
one sector per cluster.

`formatted.img` holds `DCIM/100TAPH` containing `IMG_0001.JPG` and
`IMG_0002.JPG`, each holding its own name, and `IMG_0003.JPG`, holding its
own name repeated to 1,300 bytes so that its run spans three clusters. The
volume is then formatted again.

`reused.img` is built the same way and formatted, and `DCIM`,
`DCIM/100TAPH` and a new `IMG_0001.JPG` holding `NEW_0001.JPG` are then
written.

### Environment

Two environments, with the same package versions measured by
`dpkg-query`:

```text
mtools       4.0.43-1build1
dosfstools   4.2-1.1build1
util-linux   2.39.3-9ubuntu6.6
```

An Ubuntu 24.04.4 container off the development machine, and the
development machine itself. The tool was run on the development machine
only, at `8504cd8`.

### Method

`SOURCE_DATE_EPOCH=1577836800`, `TZ=UTC` and `MTOOLS_SKIP_CHECK=1`, as
`ADR-0006` §6 Condition 1 requires. The partition table is written by
`sfdisk` with disk signatures `0xfa7300e3` and `0xfa7300e4`, and the
volume by the generator's own invocation,
`mkfs.vfat --invariant --mbr=n -F 32 -n TAPHFIX --offset=2048`. The format
is that same invocation run a second time over the populated volume. With
`V` the image at byte offset 1,048,576:

```text
formatted.img  mmd ::/DCIM
               mmd ::/DCIM/100TAPH
               mcopy 1 ::/DCIM/100TAPH/IMG_0001.JPG
               mcopy 2 ::/DCIM/100TAPH/IMG_0002.JPG
               mcopy 3 ::/DCIM/100TAPH/IMG_0003.JPG
               mkfs.vfat (as above)

reused.img     as formatted.img, then
               mmd ::/DCIM
               mmd ::/DCIM/100TAPH
               mcopy new ::/DCIM/100TAPH/IMG_0001.JPG
```

No byte is poked. Each volume was built twice per run and compared, and
the whole run was repeated in each environment. `mshowfat` read every
chain before the format. After it, clusters 2 to 4 were read with `od`,
their first five slots decoded field by field, and each file's run hashed
with `dd` and `sha256sum`, independently of the tool. The tool was then
run on each image with no arguments.

The first version of the method decoded only slots 0 and 1, which cannot
show hypothesis 2. It was corrected before this record, and both versions
produced the same image digests.

### Expected result

As hypothesised, and the tool reporting nothing recoverable on either
volume, with coverage complete and exit status 0.

### Actual result

**Determinism.** Every build of each image was identical, in both
environments:

```text
94d34d3cf21237538049f2b729f98e63cba8c61f63df9c42531d8403cbfbd9ce  formatted.img
1280a9057f2c78fc5228e37ec6df3738163c57aaa67a6a0e8330aa806abae94f  reused.img
```

The tool's own digest of each image agreed.

**Chains before the format,** exactly as predicted:

```text
::/DCIM <3>                      ::/DCIM/100TAPH/IMG_0002.JPG <6>
::/DCIM/100TAPH <4>              ::/DCIM/100TAPH/IMG_0003.JPG <7-9>
::/DCIM/100TAPH/IMG_0001.JPG <5>
```

**After the format,** in `formatted.img`:

| Cluster | FAT | Slots |
| --- | --- | --- |
| 2, the root | `0xffffff8` | volume label `TAPHFIX`, then zero |
| 3, `DCIM` | 0 | `.`→3, `..`→0, directory `100TAPH` →4, then zero |
| 4, `100TAPH` | 0 | `.`→4, `..`→3, `IMG_0001JPG` →5 size 13, `IMG_0002JPG` →6 size 13, `IMG_0003JPG` →7 size 1,300 |

Each file entry's first byte is `0x49`, not `0xE5`, and its attribute is
`0x20`. The runs 5, 6 and 7 to 9 hash to the content written:

```text
86a773f90bcf220bea99ad333fdc1476de79daddba2ea7e017de7f302e394417  IMG_0001.JPG
6aaed5ab92714c55bf157801cb012d61cfcdec8b5abcff7059722582da8c951f  IMG_0002.JPG
01910a698db4916cbeb1ae6311f0d4e9e38e9e26b753db0747e0e41c002530ee  IMG_0003.JPG
```

**After the reuse,** in `reused.img`: the root lists `DCIM` →3. Clusters 3
and 4 read `0xfffffff` in the FAT and hold the new tree, `.` and `..` as
before. Cluster 4 lists only the new `IMG_0001JPG` →5 size 13, and slots 3
and 4 are zero. Cluster 5 hashes to `f10d526e…`, the new content. The runs
of `IMG_0002.JPG` and `IMG_0003.JPG` still hash to the digests above.

**What the tool reports,** at `8504cd8`:

| Image | Reported |
| --- | --- |
| `formatted.img` | root: the volume label only; nothing recovered; coverage complete; exit 0 |
| `reused.img` | `/DCIM/100TAPH` walked; one live `IMG_0001.JPG`, 13 bytes, cluster 5; nothing recovered; coverage complete; exit 0 |

### Conclusion

1. **A quick format leaves the tree below the root whole.** Both
   directories' first clusters survive and pass `ADR-0016` Decision C's
   three checks unchanged: FAT entry 0, `.` naming the cluster it sits in,
   `..` present. `..` still links `100TAPH` to `DCIM`, and `DCIM` to the
   root.
2. **The files they list are not deleted entries.** Their first byte is
   intact and their first cluster and size survive. `eligibility` in
   `src/fat_recovery.rs` offers only deleted entries for recovery, so the
   tool would not offer these even if it read the directory.
3. **Every file survives, and is reachable only through an orphaned
   directory.** All three runs are free and hold their original bytes.
   Recovering them from these entries infers each run from a first cluster
   and a size, the method `ADR-0014` Decision A §5.1 classifies for every
   artifact the tool produces.
4. **The tool reports this volume as completely covered.** Three
   recoverable files sit on it, and nothing in the output says they might.
   Under `ADR-0014` Appendix A.6 coverage states the tool's reach, and the
   report is correct by that rule; it is the same situation Appendix B.2
   recorded for unread subdirectories.
5. **One write after the format destroys the directories, not the data.**
   The recreated tree takes clusters 3 and 4 and clears them, so no record
   of `IMG_0002.JPG` or `IMG_0003.JPG` remains in any directory, while
   their bytes survive intact. On that volume only carving can reach them.

### Limitations

1. One formatter. `mkfs.vfat` stands in for a camera's format, which is
   not measured. Vendor sources report that some cameras erase the card
   instead, which would leave nothing.
2. The same geometry before and after. A format with a different cluster
   size or FAT size moves the data area, so an old directory's `.` would
   no longer name the cluster it is read as, and Decision C's check would
   refuse it. Not measured.
3. One cluster size. The unrecorded bare-volume measurement used 4,096-byte
   clusters and random content and agreed, but its images cannot be
   reproduced and it is not counted.
4. One allocator for the reuse. `mtools` placed the recreated tree on the
   lowest free clusters; a camera's firmware may not.
5. Every file is contiguous by construction. A file fragmented before the
   format has the same false positive `KNOWN_ISSUES.md` records for a
   deleted one.
6. 8.3 names only, as in EXP-0005.

### External practice

The Sleuth Kit's FAT implementation notes describe treating every sector
of the data area as though it could hold directory entries, and scanning
for those that do (SleuthKitWiki, *FAT Implementation Notes*, accessed
2026-09-23). That accepts any plausible entry. Issue 2900 in the
`sleuthkit` repository, opened 2024-03-17, asks for orphaned FAT
directories to be found by their `.` and `..` entries, and states that
IsoBuster already does so. Decision C's checks are narrower than either:
`.` must name the cluster it sits in.

Several recovery vendors state that cameras and operating systems format
quickly by default, leaving the data area in place. These are commercial
sources and are recorded as context, not as evidence.

### Next action

1. Record, in an ADR citing this record, how directories that no
   surviving entry names are found, read and reported, and how the files
   they list are offered for recovery.
2. Add both volumes to `scripts/generate-fixtures.sh`: `formatted.img` as
   the case where such directories are found, `reused.img` as the case
   where none is.
3. Measure the same search against the existing split fixture, where
   EXP-0005 recorded that `TAIL` at cluster 19 carries `.`→19 and `..`→3.

---

## EXP-0008: Three NIST deleted-file-recovery images, against The Sleuth Kit

**Date:** 2026-09-24
**Milestone:** after M12; external validation, no decision yet
**Related:** ADR-0010 Decision C; ADR-0014 Decision A, Appendix A.11,
Appendix B.4; ADR-0016; ADR-0017; EXP-0004; EXP-0005

---

### Question

1. On images the project did not make, does Taphonomy identify the
   partitions and filesystems correctly?
2. Are the files it recovers the files that were deleted, byte for byte?
3. Where a deleted file is fragmented, what does it do, and what does an
   established tool do?
4. How much of what was deleted does it reach?

### Why it matters

Every earlier measurement used volumes this project built with
`mkfs.vfat` and `mtools`. A tool that agrees with its own fixtures has
shown consistency, not correctness. NIST's Computer Forensic Reference
Data Sets publish images built for exactly this class of tool: the
CFReDS deleted-file-recovery page describes them as images for testing
metadata-based recovery, not carving. Each is documented with the
sectors every file occupied, so a recovery can be checked against the
image's own layout rather than against either tool.

### Hypothesis

Stated before each image was run:

1. `dfr-01-fat.dd`: the first two partitions are identified as FAT16 and
   reported not analysed; the FAT32 partition lists one deleted file,
   and Taphonomy and The Sleuth Kit recover identical bytes whose tags
   name `Bellatrix.txt`.
2. `dfr-02-fat.dd`: Taphonomy refuses `Bellatrix.txt`, because its
   implied run crosses the live `Canopus.txt`. What The Sleuth Kit
   recovers was stated as uncertain: either the correct file, if it
   skips allocated clusters, or a mixture, if it does not.
3. `dfr-11-fat.dd`: Taphonomy reads the deleted directory `Sagittarius`
   from its first cluster and recovers its three files, at clusters 55,
   71 and 87, each equal to the sectors NIST lists.

### Input

Three images from the CFReDS deleted-file-recovery page, retrieved
2026-09-24, and the image layout document linked from the same page.
Each image is 1,073,742,336 bytes with three primary partitions, of which
the third is FAT32. Digests of what was served, then of each image
decompressed with `bunzip2 -k`:

```text
4a9df428d55e5c6836f998882e36f77b595bd5bcbed077cb4cbbd25c593bce46  dfr-01-fat.dd.bz2
460832716f0dc1d2817f7d2cd46a46f5f4418da6556287daa53e4d9a932dcc0a  dfr-02-fat.dd.bz2
c88b9a683e920a3f0c5c70614a2074e4a2042c52ba550cf83a250e5d11006c79  dfr-11-fat.dd.bz2
5d5bf8fb15a1df463fb0b9c0a958c05ece30941af40545f895131ff0daf32fab  dfr-01-fat.dd
ec86439d835444113cb744add3baee101394494883d5e79ff87bd64eb0189d54  dfr-02-fat.dd
08340c17a76ac4e7173388e0249665ab740dd0e1b4646579fef3440ba4d60505  dfr-11-fat.dd
```

The layout document was read in a Markdown conversion of the published
PDF, `b9c89a455b39e20386d4595e3aa2bd28843cf0f0703bd2f4395d0668df1abaa0`;
the sections used are Test Images fat-01, fat-02 and fat-11, pages 92,
95 and 145 of the original.

### Environment

The development machine: Ubuntu under WSL2, Rust 1.98.0. Taphonomy built
with `cargo build --release` at `4bd85d8`. The Sleuth Kit 4.12.1 from
the Ubuntu package `sleuthkit` 4.12.1+dfsg-1.1ubuntu2.

### Method

For each image: `mmls` for the partition table; `fls -r -o 82048` for
The Sleuth Kit's listing of the FAT32 partition; Taphonomy with
`--recover --output`, timed; `icat -r -o 82048` for The Sleuth Kit's
recovery of each deleted file on that partition. The ground truth for
each file was cut from the image with `dd` at the sectors the layout
document lists for it, and every recovery compared by SHA-256. Content
was read with `strings`: NIST's `mk-file` tags each file's blocks with
its name, so a recovered file's origin can be read from its bytes.

### Expected result

As hypothesised.

### Actual result

**Identification.** Taphonomy's classification of the first partition
equals the true type the CFReDS page lists for each image: FAT16 on
`dfr-01` and `dfr-02`, FAT12 on `dfr-11`. On the two whose table entry
declares type `0x01`, FAT12, over a FAT16 filesystem, it reported
`MISMATCH: declared type 0x01 disagrees with observed filesystem FAT16`,
the discrepancy the CFReDS page itself warns of. It reported no mismatch
on `dfr-11`, where the declaration is right. Every image reported two
filesystems not analysed and coverage incomplete.

**`dfr-01`.** One deleted file on the FAT32 partition, `Bellatrix.txt`,
712 bytes, first cluster 5. The layout document places it at sectors
90243 and 90244; cluster 5 is sector 82048 + 8192 + 3 = 90243. Both
tools recovered the same bytes, and the content names itself:

```text
6523600836b7e0fdce6713bb28c3d6d70c8f390fd885bac57d4894c3764ca527  Taphonomy
6523600836b7e0fdce6713bb28c3d6d70c8f390fd885bac57d4894c3764ca527  The Sleuth Kit
```

**`dfr-02`.** `Bellatrix.txt`, 4,096 bytes, first cluster 6, two
sectors to the cluster. The layout document places it at 90248–90251
and 90256–90259, with the live `Canopus.txt` at 90252–90255 between the
two. Taphonomy reported `run 6-9, 4096 bytes, REFUSED: cluster 8 is in
use`; cluster 8 is sector 90252, `Canopus.txt`'s first. It wrote
nothing. The Sleuth Kit recovered 4,096 bytes. Cut from the image:

```text
ae9351608a8a34c5ac24700001914cec44687b62488d6e869491da020105156b  sectors 90248-90251 and 90256-90259
ae9351608a8a34c5ac24700001914cec44687b62488d6e869491da020105156b  The Sleuth Kit
1e28b774a00742d99d1590e1b72d77937f6e2a9cbc0277a3ed2e140ab8168aa3  sectors 90248-90255, the implied run
```

The implied run's tags name `Bellatrix.txt` and `Canopus.txt`; The
Sleuth Kit's name only `Bellatrix.txt`.

**`dfr-11`.** The root lists `SAGITT~1` as a deleted directory at
cluster 5. Taphonomy read it from that cluster alone and found three
deleted files of 8,192 bytes at clusters 55, 71 and 87, each run free.
All three sources agree:

| File | Sectors | NIST, cut by `dd` | Taphonomy | The Sleuth Kit |
| --- | --- | --- | --- | --- |
| `Rukbat.txt` | 90293–90308 | `145c1752…` | `145c1752…` | `145c1752…` |
| `Elnasl.txt` | 90309–90324 | `56c8302c…` | `56c8302c…` | `56c8302c…` |
| `Nunki.txt` | 90325–90340 | `71dbaab9…` | `71dbaab9…` | `71dbaab9…` |

No orphaned directory was reported.

**Reach.** The layout document lists 15 deleted files across the three
images:

| Image | Deleted | On FAT32 | Taphonomy, FAT32 | The Sleuth Kit, FAT32 |
| --- | --- | --- | --- | --- |
| `dfr-01` | 3 | 1 | 1 | 1 |
| `dfr-02` | 3 | 1 | 0, refused | 1 |
| `dfr-11` | 9 | 3 | 3 | 3 |

The Sleuth Kit was not run on the FAT12 and FAT16 partitions, so what it
reaches there is not measured.

**Time.** On the 1 GiB images the release build took 10.63 s on
`dfr-02` and 9.93 s on `dfr-11`, reading and hashing every byte.

**Two defects observed.** The heading of the deleted directory reads
`deleted directory /?AGITT~1`, while its own entry in the root reads
`name SAGITT~1 recovered`: the path is built without the long-name
association the listing uses. And every name is shown in its 8.3 form
(`URSA_M~1`, `BETELG~1.TXT`) where The Sleuth Kit shows the long name.

### Conclusion

1. **Every file Taphonomy recovered is the file that was deleted.** Four
   recoveries across three images each equal the sectors NIST documents,
   and equal The Sleuth Kit's.
2. **Its identification is right on evidence it did not make,** including
   the declared-type discrepancy NIST documents.
3. **On a file fragmented around a live file, it refuses, and The Sleuth
   Kit recovers it correctly.** The Sleuth Kit's recovery equals the
   documented sectors, so it passed over the allocated clusters to reach
   the second fragment. That rule is an inference too; here the layout
   rewards it. Taphonomy's refusal is correct by `ADR-0010` Decision C
   and produces nothing, where the implied run would have produced a
   file half of which is another, live file.
4. **It reaches four of the fifteen deleted files.** The other eleven are
   on FAT12 and FAT16 partitions, which it identifies and does not
   analyse. Every CFReDS FAT image carries two such partitions.

### Limitations

1. Three images of the seventeen FAT cases. The cases the layout
   document singles out for FAT, the interleaved fragmentation of
   DFR-05, DFR-05-BRAID and DFR-05-NEST, were not run; there the gaps in
   an implied run are free, and neither tool's rule is expected to help.
2. The Sleuth Kit was run only on the FAT32 partitions, so its reach on
   FAT12 and FAT16 is not measured here.
3. The layout document was read in a conversion of the PDF, whose own
   digest was not recorded. Every sector used here was checked against
   the image, and every one was right.
4. Times are single runs.

### External practice

NIST's CFTT project reported, from testing deleted-file-recovery tools
on these scenarios, that on FAT only a deleted file's first block is
identified, and that tools guessing the rest often recover files mixed
from several originals (Lyle, AAFS 2013). The CFReDS page notes that the
fragmented cases often give interesting results on FAT. `dfr-02` is one
of them: the mixture is the implied run, which Taphonomy refused.

### Next action

1. Correct the deleted directory's heading to use the name its entry
   recovers.
2. Decide the next milestone from this record: FAT12 and FAT16, long
   names, or the fragmented cases.

### Appendix A: ten on FAT12 and FAT16, not eleven (2026-09-25)

Conclusion 4 says the eleven deleted files Taphonomy did not reach are on
FAT12 and FAT16 partitions. The Reach table in Actual result says
otherwise: five of the fifteen are on the FAT32 partitions, one on
`dfr-01`, one on `dfr-02` and three on `dfr-11`. Taphonomy recovered four
of those five and refused the fifth, `dfr-02`'s `Bellatrix.txt`. Of the
eleven it did not reach, ten are on FAT12 and FAT16 partitions and one is
that refusal. The table is right and the conclusion is not. The body is
not rewritten; every measurement in it is unchanged.

---

## EXP-0009: What Windows leaves when it deletes from exFAT

**Date:** 2026-09-26
**Milestone:** before exFAT support; informs ADR-0019
**Related:** ADR-0002 §3.5, §5.2; EXP-0008

---

### Question

When Windows' own exFAT driver deletes a file:

1. What survives of the file's directory entry set?
2. What survives of a fragmented file's FAT chain?
3. What happens to the allocation bitmap?
4. What does The Sleuth Kit recover from the result?

### Why it matters

`ADR-0002` §5.2 names exFAT the correct second target. §3.5 records that
FAT32 deletion clears the cluster chain, so a deleted fragmented FAT32
file cannot be reassembled from metadata at all. exFAT keeps allocation
state in a bitmap rather than in the FAT (specification §5.1 and §7.1.5),
and marks an entry deleted by clearing one bit, InUse (§6.2.1). If
Windows leaves the chain in place, a deleted fragmented exFAT file can be
recovered exactly, which FAT32 never allows. The Sleuth Kit's exFAT
implementation notes say it does; nothing in this project had measured
it.

### Hypothesis

Stated in the protocol before any measurement:

1. P1: `CONTIG.bin`, written first to an empty volume, is contiguous and
   its Stream Extension entry has NoFatChain = 1.
2. P2: `FRAG.bin`, written after the volume was filled and every second
   filler deleted, occupies at least four runs and has NoFatChain = 0.
3. P3: deletion changes only the high bit of each EntryType in the set:
   `85h` to `05h`, `C0h` to `40h`, `C1h` to `41h`.
4. P4: with the InUse bits restored, each deleted set's SetChecksum is
   valid.
5. P5: deletion clears the file's bits in the allocation bitmap.
6. P6: deletion leaves `FRAG.bin`'s FAT entries unchanged.
7. P7: every file not deleted is byte-identical before and after.

### Input

Four target files and 300 fillers, written by
`scripts/experiments/exp-0009-generate.py`. Every 512-byte block begins
with a header naming its file and block index, so any cluster identifies
itself. Output is deterministic; the targets' digests:

```text
799230d4c57749efdd2c1bf80a663062bc7323a47869e52c7122b82068a95e2e  KEEP.txt  5000
afbc355c38682d3c1e46755da3a681fdc931129c6fc6fed3ba47eb790bd87f17  CONTIG.bin  262144
ac61dcd74380dc95f3ca6ffbfc34725b9c47c67378c5a51282df2311da0e9f77  SMALL.txt  1000
446a6a9ca69120765f8f541b75790a9402dd4f61da7df2e1c9d27cd487ba259f  FRAG.bin  245760
```

Two images of one 16 MiB fixed-size VHD, before deletion (A) and after
(B). Each is 16,777,728 bytes: the disk and the 512-byte VHD footer.

```text
f9ad9b94a1ce38b45abfda1c17d78f0278e5fe5314f4556ea431439605dd545e  exp9-A.vhd
a0ffa52069c0238f47201cea69df495a85c27ad589c1414299644339b9e4fc39  exp9-B.vhd
```

### Environment

Windows 10, build 19045.6466; DiskPart 10.0.19041.3636. The images were
read under WSL2 on the same machine. The analysis was repeated on a
second machine, an Ubuntu container, from a transferred copy whose
digests matched. The Sleuth Kit 4.12.1, the Ubuntu package, on both; and
4.15.0 on the second machine, built from the official release tarball,
`3a8c1e7d18a9b81f3e5e8aa78313974aceaafc6e051d636bc92cd7168286eca9`.

### Method

DiskPart created the VHD, gave it an MBR and one primary partition, and
quick-formatted it exFAT with the default 4 KiB cluster. PowerShell
copied `KEEP.txt`, `CONTIG.bin` and `SMALL.txt`, then fillers of 64 KiB
until the volume was full, then deleted every even-numbered filler and
copied `FRAG.bin` into the holes. DiskPart detached the VHD, which was
copied as A. It was attached again, `Remove-Item` deleted `CONTIG.bin`,
`SMALL.txt` and `FRAG.bin`, bypassing the Recycle Bin, and the detached
VHD was copied as B.

`scripts/experiments/exp-0009-analyse.py` parses both images from the
specification: boot region with its checksum, FAT, bitmap, and every
entry set, following each directory's chain. It scores each prediction
from the bytes and names the structure holding every byte that differs
between A and B. The Sleuth Kit's `fls -r -d -o 128` listed B's deleted
files, and `icat -o 128` recovered each target; every result was compared
by SHA-256 with the generated file.

Two departures from the protocol. `SMALL.txt` was copied twice, the
second copy overwriting the first. The fill stopped at `FILL233.bin`,
leaving 232 fillers, of which 116 remained after the holes were punched.

### Expected result

As hypothesised.

### Actual result

The volume: partition at sector 128, 512-byte sectors, 4,096-byte
clusters, 3,792 clusters, root directory at cluster 5, bitmap at
cluster 2. The boot checksum is valid in both images.

| | Result |
| --- | --- |
| P1 | True. `CONTIG.bin`: clusters 10 to 73, NoFatChain 1. |
| P2 | True. `FRAG.bin`: four runs, clusters 3776 to 3793, 91 to 106, 123 to 138 and 155 to 164; NoFatChain 0. |
| P3 | True. In each of the three sets only bytes 0, 32 and 64 changed, from `85 C0 C1` to `05 40 41`. |
| P4 | True. Each deleted set's checksum is invalid as found and valid with the InUse bits restored. |
| P5 | True. Exactly 125 bits cleared: 64, 1 and 60, the targets' clusters and no others. |
| P6 | True. Every FAT entry of `FRAG.bin` is unchanged; its chain followed in B yields the file, SHA-256 equal to the original. |
| P7 | True. All 118 live files other than the targets are byte-identical in A and B. |

589 bytes differ between A and B:

| Bytes | Structure |
| ---: | --- |
| 511 | Cluster 165: first sector of `System Volume Information\IndexerVolumeGuid` |
| 48 | Directory `System Volume Information`: that file's new entry set |
| 20 | Allocation bitmap: the 125 cleared bits and one bit set, cluster 165 |
| 9 | Root directory: three EntryType bytes in each of three sets |
| 1 | Main boot sector: PercentInUse, 52 to 49 |

`IndexerVolumeGuid` did not exist in A. Windows created it between A and
B, while the volume was attached for the deletion, in cluster 165, which
was free and held data of the deleted `FILL006.bin`. Windows wrote one
512-byte sector, 76 bytes and zero padding; the cluster's other seven
sectors still hold `FILL006.bin`'s blocks 81 to 87.

The Sleuth Kit's listing of B shows the three targets deleted, and also
six deleted entry sets named `FILL233.bin`. The only operations on that
name were one failed copy and the removal of its partial file.

Recovery from B:

| File | 4.12.1 | 4.15.0 |
| --- | --- | --- |
| `CONTIG.bin` | equals original | equals original |
| `SMALL.txt` | equals original | equals original |
| `FRAG.bin` | empty; `Invalid address in run (too large): 30592` | the same |

With `-r` the result for `FRAG.bin` is the same. Both versions recover
`FRAG.bin` correctly from A, where it is live.

### Conclusion

1. **On this Windows build, deletion from exFAT destroys no metadata a
   recovery needs.** The entry set, the FAT chain and the data survive;
   only the InUse bits, the bitmap and PercentInUse change.
2. **A deleted fragmented file can be recovered exactly, and the
   recovery can be checked three ways:** the entry set's checksum with
   InUse restored, the chain's clusters all free in the bitmap, and the
   chain's length against the recorded size. FAT32 offers none of these.
3. **The Sleuth Kit, including its current release, does not recover
   that file.** It reads the chain while the file is live and not once
   it is deleted. The error names sector 30592, one past the partition's
last sector. Sixty clusters counted contiguously from cluster 3776 would
run past the volume's last cluster, 3793, whose last sector is 30591.
That is consistent with treating the deleted file as contiguous. It is
an inference from the output, not from The Sleuth Kit's source.
4. **Attaching a volume in Windows wrote to it.** Windows created a file
   in free space and overwrote deleted data. A tool must read a volume
   without mounting it.
5. **One failed copy and its removal left six deleted entry sets for
   one file name.**

### Limitations

1. One Windows build, one volume size, one cluster size, one deletion
   method. Cameras, phones, macOS and Linux delete through their own
   drivers and were not measured.
2. Every target was written in full, so each ValidDataLength equals its
   DataLength. The analysis does not read ValidDataLength, and this
   experiment says nothing about files where they differ.
3. `FRAG.bin`'s first run ended at the volume's last cluster. Where The
   Sleuth Kit's assumed run would fit inside the volume, what it returns
   was not measured.
4. `SMALL.txt` was copied twice before A was taken.

### External practice

The Sleuth Kit's exFAT implementation notes state that deletion does not
clear FAT entries on exFAT, so a fragmented file can be reconstructed if
none of its clusters were reused. This experiment confirms the statement
for Windows 10 and finds The Sleuth Kit's own recovery does not act on it.

### Next action

1. ADR-0019: exFAT support, scoped by this record.
2. Phase 2 on physical flash: whether inserting a USB stick into Windows
   writes to it before any tool reads it.
3. EXP-0010: a deleted fragmented file whose contiguous reading would
   stay inside the volume, and what The Sleuth Kit returns for it.
4. Report conclusion 3 to The Sleuth Kit's maintainers.

### Appendix A: intact metadata is not proof of data (2026-09-26)

After this record was committed, the recovery rule proposed for ADR-0019
was tested against the ground truth each block carries. Conclusion 2 says
a deleted fragmented file's recovery "can be checked three ways". Those
checks show the metadata is intact. They do not show that the clusters
hold the file's data, and on image B, for 19 files, they do not.

**Method.** `scripts/experiments/exp-0009-overlap.py` reads, for every
deleted entry set with data on B, the clusters its metadata records, and
compares the file named in each block's header with the name the set
records. Second method: The Sleuth Kit 4.15.0's `icat -o 128` recovered
every deleted entry `fls -r -d -o 128` lists on B, 125 in all, and each
result was compared by SHA-256 with the generated file and checked for
content that is only zero bytes.

**Result.** 119 deleted entry sets have data. One, `FILL006.bin`, fails
the metadata checks, because cluster 165 is allocated again. Of the other
118:

| Count | Content at the recorded clusters |
| ---: | --- |
| 99 | The file's own data, including `FRAG.bin` by its chain |
| 16 | Zero bytes only: `FILL200.bin` to `FILL230.bin`, even numbers |
| 3 | `FRAG.bin`'s data: `FILL002.bin`, `FILL004.bin`, `FILL232.bin` |

The Sleuth Kit agrees on every file both read. It returns the same 98
files correctly, the same 16 as zeros, and the same three, with
`FILL006.bin`, as another file's data. It returns nothing for `FRAG.bin`
and for the six zero-length `FILL233.bin` sets.

**The 16 zero files.** Their clusters were already zero in A. Each
Stream Extension entry records ValidDataLength equal to DataLength,
65,536, read by the parser and, for `FILL200.bin`, from the raw bytes; so
the file system recorded them as fully written. They are the last even
fillers written before the holes were punched; the other even fillers,
deleted by the same command, kept their data. One explanation fits: their
data was still in Windows' write cache when they were deleted and was
never written, so the clusters kept the zeros the new VHD held. That is
not established.

**The three overwritten files.** `FRAG.bin` was created at 18:52:14; the
files whose clusters it took, `FILL002.bin`, `FILL004.bin`, `FILL006.bin`
and `FILL232.bin`, between 18:51:48 and 18:51:53. The Sleuth Kit's
`istat` and the parser agree. `FILL002.bin`, `FILL004.bin` and
`FILL006.bin` share the same second and differ only in the 10 ms field.

**What each recovery rule does on B:**

| Rule | Right | Wrong | Zeros | Refused |
| --- | ---: | ---: | ---: | ---: |
| Metadata checks only | 99 | 3 | 16 | 0 |
| And, where deleted sets claim one cluster, only the latest created | 99 | 0 | 16 | 3 |
| And a run of zero bytes only is refused | 99 | 0 | 0 | 19 |

Conclusion 2 is to be read as: the three checks show the metadata is
intact; they do not show the data is the file's. The other conclusions
are unaffected. The last rule is correct on this image; it detects data
that is only zeros, not stale data of any other kind, and its test of
creation time can meet a tie. The body is not rewritten; every
measurement in it is unchanged.

---

## EXP-0011: The recovery rules of ADR-0019 against public exFAT images

**Date:** 2026-10-21
**Milestone:** before M15; informs ADR-0022 and ADR-0019 Appendix C
**Related:** ADR-0019 sections 4 and 6; EXP-0009 and its Appendix A;
EXP-0008

---

### Question

Before any recovery code exists:

1. Does ADR-0019 section 4 recover only files whose data is their own,
   on exFAT volumes deleted by systems other than the one EXP-0009
   measured?
2. Does each rule proposed for M15 in the audit of 2026-10-20 improve
   that, and what does each cost?
3. What do macOS and Linux leave when they delete from exFAT?

### Why it matters

ADR-0019 section 4 was derived from one Windows volume. ADR-0019
section 6 requires NIST's exFAT images before any public claim about
exFAT recovery. A rule that is right on the volume it was built from is
not shown right by that volume.

### Hypothesis

No prediction was recorded before measuring. The audit of 2026-10-20
made recommendations, not predictions. The statements below restate
those recommendations as testable claims, and were written after the
measurement. They are scored, but they are not a protocol in EXP-0009's
sense.

1. H1: on NIST's images, section 4 recovers no file whose clusters hold
   another file's data.
2. H2: comparing a rival's last modification as well as its creation
   refuses no correct file.
3. H3: refusing every set on a volume holding a claim the walk cannot
   see costs little, because such claims are rare.
4. H4: macOS and Linux leave the FAT chain of a deleted file, as
   Windows does.
5. H5: on every volume, every cluster used as structure is marked in
   use in the allocation bitmap.
6. H6, written after the first scoring: NIST's tags judge every
   recovery.

### Input

**NIST CFReDS deleted-file-recovery images**, the twenty exFAT
("xfat") images listed at
<https://cfreds-archive.nist.gov/dfr-test-images.html>, downloaded
2026-10-20, with the layout document `setup-july-10-2012.pdf`
(SHA-256
`86b5a888e60f1127ac77e57929dd64bd4346af882f904ddd45ba7d90afbc803e`).
NIST made them on macOS (`diskutil` output in the document). Nine have
a GPT and eleven an MBR partition of type `0x07`. Compressed:

```text
4ce2864c1ab4353fb26fc3bd7d5a2ec8fb90cfccaa14e26aef646305efb1ddc5  dfr-01-xfat.dd.bz2
84727f96984bcac0b61f9fa67b376a5b8884ef83e42d4a17455b4c0a1f21a951  dfr-02-xfat.dd.bz2
271edaa5ec7dad75bf9d5be235b164ef16acb6554c18c2c96ef35ec840bb5555  dfr-03-xfat.dd.bz2
b6f7ab11bce20bd02ff6c1b689ca35098cb8b8757c411bd765a9e46ad58f73c7  dfr-04-xfat.dd.bz2
c4f01410d6f0a7f759f867eb6ca7945d550e624cb421edd91d478c35c470b855  dfr-05-braid-xfat.dd.bz2
ce0472cc193d0923e787dab8cce99c5000826a06cd69efc5507097e3e25c3ec8  dfr-05-nest-xfat.dd.bz2
0ce5d5dde2d351b6fc076901d9342ecec3719b93c53c8e224cebbc4f228d1f57  dfr-05-xfat.dd.bz2
0c5b06fe0cef21f46533b214d3f0ecfe2fe1b499a7027a4389e50ca49b9ba4a9  dfr-06-xfat.dd.bz2
da47c444bb520ba1241c46cad7740dd698815bfe9b18c98b8f89521146da2f3f  dfr-07-one-xfat.dd.bz2
f239191e0441931cbf9024c01fc1dd4db5b2e291be2e525aa663252038bebdf6  dfr-07-two-xfat.dd.bz2
15720cdb99927ad2c333130fb308718165cb09c1404737e7c638d9d9b51ca65b  dfr-07-xfat.dd.bz2
8a5caac60b4d802dcdb1000d784eb604dddaad47abc736c5ed4558ba844c990a  dfr-08-xfat.dd.bz2
d506e79b4aa2e62fe29fee50d543b886db19a5b895d3f1f3acb50f08536531c5  dfr-09-xfat.dd.bz2
2f76f3b41613ff5b5ed40608998934f5c259c0dff5ec8c68fd6a3ec870ea6daf  dfr-10-xfat.dd.bz2
7fc36a9f1f51b7b867c6ee09e5366db071991e1d2bfb103717764c7f09326389  dfr-11-xfat.dd.bz2
8a84d6a8152cfe0a414a744ec47b445586ed728adbd56752f50301b8b06b6301  dfr-12-xfat.dd.bz2
d140b849e883747638b517426133176d1a47b8917802271fc8dd0ae678d13280  dfr-13-xfat.dd.bz2
4924532286809be83f269443b0d2822c0c50b493757c1cc18990de05071abfc1  dfr-15-xfat.dd.bz2
5661beda4b6160129ee058c4276b3cb6f2ed8e1a036fd607167dbf558516f8fa  dfr-16-xfat.dd.bz2
3c7d1b7d170980ae26f063ae997c5a90d0335716122ee5a8c4480b003e803d19  dfr-17-xfat.dd.bz2
```

Expanded with `bzip2 -dc`:

```text
bb3755982959e189d7cfc7a4819553406e5c64e67a11d6b875ea1d4f23fa745b  dfr-01-xfat.dd
e0647de7136edcddb0b0be842a5a344b0b0e7a987f43afb1eccdb256c2eb01c8  dfr-02-xfat.dd
65d9663bb9469b9432d1164bce7123ef3e3434a3e1dc5d04e72212244ed64789  dfr-03-xfat.dd
aa294805bef0da4630126cb3dc3c20db48b3508ab06bb4958c025ad85d0d4ae5  dfr-04-xfat.dd
5cf7e6e429af3cf3ddd08c3f5ef73ce074a4cfe1b11752587e48d9a23112d64a  dfr-05-braid-xfat.dd
f717b39e7bdbfeb5c88c9ceb7205f3bf2205ed3405a4d71fedc270362c199b20  dfr-05-nest-xfat.dd
770873d206449cc55512ea3d06eb9a64dc0c300282fc03f267a3bef974647a1a  dfr-05-xfat.dd
6ff1e2967dd5a2f183af972b52ad128c3aecf3e1b722ff286fffdbe7bef6d615  dfr-06-xfat.dd
91f1171b434f500ee66e3bc4a0ed2cf7184b021d35c71a6081bdff759e5ba61a  dfr-07-one-xfat.dd
b3585b73f02de39c597ac7729ca544a5ff205372008e103886a6d3101d418d7f  dfr-07-two-xfat.dd
9cdcdda1944b4ed2ce9c4ba4361936b77bf05caca8a54161d7480560c51f15a8  dfr-07-xfat.dd
0bcc2868cf92eb627c75d7d0505de991759a81b717e949891058fd8ef225ba5c  dfr-08-xfat.dd
e9005b6f8207e85b64acfd78bb5de88ed969686273e9efec87b4bfd31b3751a1  dfr-09-xfat.dd
c102ef096ffa38979dfeaefed563bc7fb23f900ae4d0f5aad05656e28e6d0449  dfr-10-xfat.dd
c598a1034e095ba103e3c885b09ed0aa05daebd32d4f9b174973b26378110dcc  dfr-11-xfat.dd
5f234b36b41b9f47279d78221a3e7f93ea7efc43c807f801e31cf25f159e76d1  dfr-12-xfat.dd
b44230b25744da359acc96b5fd885f240c970aa019c149beb548073ba27befcf  dfr-13-xfat.dd
8c5d468e7086852a40e2c2e0a3887cbef664fce9e723ae568c1d69e9ec049fed  dfr-15-xfat.dd
49a0b87c933486a8a79c0d1804918a18adfe5e1e2122ad0aa5b5c95cbfc30af3  dfr-16-xfat.dd
fc0c1f200e9955abbae3c5d19db5e531d5f55417524de816943ad72dce6cd147  dfr-17-xfat.dd
```

**EXP-0009's images A and B** (`f9ad9b94…`, `a0ffa520…`), deleted by
Windows 10.

**`forensics-samples`** by João Eriberto Mota Filho,
<https://github.com/eribertomota/forensics-samples>, commit `b938bbe`.
The images are under the MIT licence and the original files under
CC-BY-SA-4.0, per its README. `fs.exfat.xz`
(`6aa8868301b0288cd3f9d751c1779fd79570f7cb1bb9568d70d0a27d74c00d70`)
expands to `fs.exfat`
(`98d518601199a32054158bb3a759e12b554fd2ebcc5960541caf9e1a907198d0`).
Made on Linux by `cp -a` of the originals, then `rm -rf` of every
directory whose name ends in `2`. `fs.multiple.xz`
(`5c2be233b134a162048bae0ebecb38ec320eb404e6b74c6ada31ece31a072447`)
was read by Taphonomy only.

No image was written to, and none is committed.

### Environment

WSL2 on the author's machine, Python 3 run with `-I`, Taphonomy built at
`a89a122` into a separate target directory.

### Method

Every figure below is printed by one command,
`scripts/experiments/exp-0011-run.sh WORKDIR`, whose header names the
inputs it expects. It checks every input's digest before and after the
runs, and prints `FAIL` for any step that fails. On 2026-10-21 it printed
none, and every digest was unchanged after the runs.

* `exp-0011-scan.py` reads the exFAT volume of an image's first MBR or
  GPT partition from the specification, and lists every entry set, live
  and deleted: checksum with InUse restored, the three timestamps with
  their 10 ms fields and offset bytes, the allocation as runs, whether a
  chain ends in `FFFFFFFFh`, and which clusters the bitmap marks in use.
  With `--deleted-dirs` it also reads a deleted directory whose clusters
  are all free and were not read before, and marks the sets found in it
  as deleted. It was written apart from EXP-0009's scripts and from
  Taphonomy.
* `exp-0011-score-nist.py` decides every deleted set under each rule
  and scores it against NIST's ground truth. NIST's tool tags every
  512-byte block: "File X" for block 0, "Block n Segment s file X" after
  it, "Tail X" last, and "Empty Sector … not used" where nothing was
  written. A recovery is **right** when its blocks are blocks 0, 1, 2 …
  of one file in order and the tag names the entry. Where NIST tagged a
  non-ASCII name with an ASCII stand-in (dfr-04), only the order is
  checked. It is **wrong** when it holds another file's blocks, blocks
  out of order, or unwritten sectors. macOS's own files carry no tag,
  and are judged by their format instead: an AppleDouble file (`._*`)
  begins `00 05 16 07`, an fseventsd log (named by 16 hexadecimal
  digits) is gzip, `1f 8b`, and `fseventsd-uuid` is a UUID. A file
  holding another format is **wrong**. The format proves the format,
  not identity. A file of no known format would be **unjudged**; none
  was recovered.
* `exp-0011-score-exp9.py` does the same for images A and B against
  EXP-0009's generator, re-implemented from its documented format and
  checked against EXP-0009's four target digests.
* `exp-0011-score-linux.py` compares each deleted file on `fs.exfat`
  with the published original by SHA-256.
* `exp-0011-check-bitmap.py` checks the bitmap's chain, and whether
  every cluster of the root, each directory read, the bitmap and the
  up-case table is marked in use.
* Second method for NIST: `exp-0011-pdf-text.py` turns NIST's layout
  document into text, and `exp-0011-sector-list.py` compares the extent
  of each recovery with the document's `Sector List` for that file. The
  extraction is partial.

The rules scored:

| Variant | Rule |
| --- | --- |
| `adr` | ADR-0019 section 4 as written, creation times compared in UTC by the stored offsets |
| `m` | and a rival last modified at or after this set's creation also refuses it |
| `o` | times compared only where both offset bytes are valid and equal and every field is in range; otherwise refused |
| `e` | ValidDataLength 0 with data, and a first cluster with DataLength 0, refused |
| `moe` | `m`, `o` and `e` together: the rule ADR-0019 Appendix C and ADR-0022 adopt |
| `u` | any orphan secondary, unreadable deleted directory or unusable set refuses every set on the volume |
| `lit` | `moe`, and any deleted directory at all also refuses every set: section 4's "only if all of these hold" read literally |
| `adrc`, `moec` | `adr` and `moe` with deleted directories read for their claims, nothing inside them decided: what M15 does |

Two tables per image: deleted directories not read (`unread`), and
read (`read`). `adrc` and `moec` are scored on the `read` tables. On
them, a set inside a deleted directory counts among the refused, since
it is not recovered.

### Expected result

Not applicable: see Hypothesis.

### Actual result

| | Result |
| --- | --- |
| H1 | False. Three wrong recoveries with claims read, eight without (below). |
| H2 | False. It refuses 2 correct files, and one wrong one. |
| H3 | False. Every NIST volume holds a deleted directory. |
| H4 | True of macOS. Not exercised for Linux. |
| H5 | True on all 23 images checked. |
| H6 | False. macOS's own files carry no tag, and are judged by format. |

**NIST, all twenty, totals:**

| Table | Variant | Recovered right | Recovered wrong | Refused right | Refused wrong |
| --- | --- | ---: | ---: | ---: | ---: |
| `unread` | `adr` | 658 | 8 | 7 | 35 |
| `unread` | `m`, `mo`, `moe` | 656 | 7 | 9 | 36 |
| `unread` | `o`, `e` | as `adr` | | | |
| `unread` | `u` | 644 | 8 | 21 | 35 |
| `unread` | `lit` | 0 | 0 | 665 | 43 |
| `read` | `adrc` | 658 | 3 | 33 | 41 |
| `read` | `moec` (M15) | 656 | 2 | 35 | 42 |
| `read` | `adr` | 681 | 4 | 10 | 40 |
| `read` | `moe` (M16) | 679 | 2 | 12 | 42 |
| `read` | `lit` | 0 | 0 | 691 | 44 |

No recovery under any variant is unjudged, and none holds unwritten
sectors. Under `adrc` and `moec`, "refused right" includes the 23
correct sets inside readable deleted directories, which those variants
do not recover.

**NIST, per image, recovered right / wrong:**

```text
image               sets  unread    unread    claims    read
                          adr       moe       moec      moe
dfr-01-xfat            4     1/1       1/1       1/0       3/0
dfr-02-xfat            4     1/1       1/1       1/1       1/1
dfr-03-xfat            4     1/1       1/1       1/0       3/0
dfr-04-xfat           15    12/1      12/1      12/0      14/0
dfr-05-braid-xfat      4     3/0       3/0       3/0       3/0
dfr-05-nest-xfat       5     2/1       2/1       2/0       4/0
dfr-05-xfat            5     2/1       2/1       2/1       3/1
dfr-06-xfat            3     2/0       2/0       2/0       2/0
dfr-07-one-xfat        4     2/0       2/0       2/0       2/0
dfr-07-two-xfat        3     1/0       1/0       1/0       1/0
dfr-07-xfat            5     4/0       4/0       4/0       4/0
dfr-08-xfat           15    14/0      14/0      14/0      14/0
dfr-09-xfat          262   261/0     261/0     261/0     261/0
dfr-10-xfat          262   261/0     261/0     261/0     261/0
dfr-11-xfat            4     1/0       1/0       1/0       5/0
dfr-12-xfat            8     3/1       3/1       3/0      11/0
dfr-13-xfat          121    84/1      82/0      82/0      84/0
dfr-15-xfat            2     1/0       1/0       1/0       1/0
dfr-16-xfat            2     1/0       1/0       1/0       1/0
dfr-17-xfat            2     1/0       1/0       1/0       1/0
```

"sets" counts deleted sets outside deleted directories (the `unread`
tables). Columns are recovered right / wrong.

**The wrong recoveries.**

* dfr-13, `/Ursa/B104-beta=s-s=D=s-s=C-1.txt`: its cluster holds
  segment 3 of `B083-alpha=s-s=C=a-a=D-2.txt`, from that file's block 24.
  NIST's build steps for dfr-13 append to files after others are
  created. `m` refuses it, and also refuses
  `/Taurus/B148-beta=s-s=C=a-a=D-1.txt` and
  `/Taurus/B151-beta=s-s=C=a-a=D-4.txt`, which hold their own data. These
  three are the only decisions that differ between `adrc` and `moec`.
* `/.Trashes/._501` at cluster 10 on dfr-01, 03, 04, 05-nest and 12,
  recovered under every variant but `lit` on the `unread` tables, holds
  gzip data, not AppleDouble. On each, a set inside the deleted
  `/.fseventsd` claims cluster 10, so reading deleted directories for
  their claims refuses all five.
* `/.Trashes/._501` on dfr-02 and dfr-05 holds gzip data under every
  variant but `lit`. On dfr-05 a deleted fseventsd log inside the
  deleted `/.fseventsd` claims the same cluster, and its creation and
  modification times are both earlier than `._501`'s creation. On
  dfr-02 no entry the walk finds claims the cluster. Neither is
  prevented by any rule scored.
* Under `adr` on the `read` tables, which recover inside deleted
  directories, one fseventsd log inside the deleted `/.fseventsd` of
  dfr-02 holds another format; `m` refuses it.

**The second method.** 54 of the recoveries under `moe` on the `read`
tables could be compared with NIST's sector lists. 47 equal them. The
other 7 differ only through the text extraction: five have a page number
joined to their last sector (`391-39881` for 391 to 398, on page 81),
one has its tail sector listed apart (`2400 TAIL/Betelgeuse` in the
layout history), and one lists the same two runs in ascending order. 627
have no list in the extracted text. NIST's list of overlaps ("A x B:
overlap") names pairs of files, not which wrote last. In dfr-12, the
files written at step 5 into sectors freed at step 4 hold their own
data by their tags.

**Deleted directories.** Every NIST volume holds at least one: macOS's
`/.Trashes/501` and others. On eleven, `/.Trashes/501`'s cluster is in
use again, so it cannot be read. 27 sets lie inside those that can be.
Reading them for their claims prevents five wrong recoveries (above).
Recovering them as well adds 23 correct recoveries, among them the
targets of DFR-11 and DFR-12, and no wrong one. macOS leaves the
entries inside a deleted directory marked in use.

**macOS and the FAT chain.** The 14 fragmented deleted files of DFR-02,
03, 05, 05-braid, 05-nest and 12 are recovered by their chains, each
right. The 8 sets refused under condition 4 are all on dfr-13. Each
chain ends before its DataLength is covered, because later files
reused its clusters.

**Offsets.** On every NIST volume, all deleted sets carry one offset
byte: `0x90` (valid, UTC+4 h), or `0x94` (UTC+5 h) on dfr-15 and
dfr-17. NIST's log records the deletions in EDT, UTC-4 h, so `0x90` is
the sign-switched offset Nordvik and Axelsson (2022) report of macOS.
On dfr-15, seven live sets carry `0xEC` (UTC-5 h) beside sets carrying
`0x94`. Which systems wrote them was not established. No two deleted
sets with different bytes claim a cluster, so `o` changes nothing.

**Unused secondaries outside a set.** Three: a File Name entry in
dfr-08's root, and entries of types `66h` and `71h` inside deleted
directories. No unused Stream Extension lies outside a set anywhere.

**EXP-0009, against the generator**, identical under `adr-0019`, `c2m`
(`m`) and `moe`:

* B: 99 recovered, each equal to the file written, `FRAG.bin` by its
  chain; 3 refused under condition 2; `FILL006.bin` under condition 1;
  16 under condition 3; 6 with no data.
* A: 96 recovered, each right; 4 refused under condition 1, the clusters
  of the live `FRAG.bin`; 16 under condition 3; 6 with no data.

No deleted set on either is unusable, all 125 on B included.

**Linux.** `fs.exfat` holds 4 deleted directories in its root, which
hold 18 deleted files. Read with deleted directories, all 18 equal the
originals by SHA-256. All are contiguous, so the FAT chain was not
exercised. Every offset byte is `0x80` (valid, UTC+0), which is what
Nordvik and Axelsson report of the native Linux driver. Taphonomy at
`a89a122` lists the 18 live files with the originals' names and sizes.
In `fs.multiple`, the exFAT partition declares 103,809,024 bytes in a
partition of 41,943,040, running into the NTFS partition after it, and
Taphonomy refuses its boot region, as ADR-0021 section 3 decides.

**The bitmap.** On A, B, the twenty NIST images and `fs.exfat`, the
bitmap is one cluster, its chain ends in `FFFFFFFFh`, and every
structure cluster is marked in use.

**How many clusters deleted sets claim.** On every image, the clusters
claimed by deleted sets, deleted directories read, are at most 1.00
times the volume's cluster count: NIST's dfr-06, one large file.

**Taphonomy at `a89a122`** reads NIST's eleven MBR images without
error, counting their deleted sets as not examined. It reports the nine
GPT images as "GPT not analysed".

### Conclusion

1. **ADR-0019 section 4 recovered wrong files on a public set.** One is
   an append: creation time does not say who wrote a cluster last.
   Adding the rival's modification time removes it, at a cost of 2
   correct files in 658. ADR-0019 Appendix C adopts it.
2. **Claims inside deleted directories matter.** Left unread, they let
   five files be recovered over data a deleted directory's file wrote.
   ADR-0022 Decision K reads deleted directories for their claims.
3. **Two wrong recoveries remain.** Under the rule adopted, M15's
   configuration recovers 656 correct files and 2 wrong ones on NIST's
   images, both macOS's own `._501` holding stale data that is not
   zeros. No rule scored prevents them except refusing everything.
4. **Refusing a whole volume for any unseen claim is not usable.** Read
   literally it recovers nothing on any NIST image. ADR-0022 Decision D
   judges condition 2 over the claims seen and counts the rest as gaps.
5. **macOS leaves FAT chains on deletion, as Windows does.** A deleted
   fragmented exFAT file is recoverable by its chain from both.
6. **Deleted directories hold recoverable files on macOS and Linux.**
   Recovering them is M16.
7. **The offset rule, the two refused shapes, and reading the bitmap as
   a chain are exercised by no public image found.** Unit tests must
   hold them.

### Limitations

1. The scripts' rules are a second implementation, not Taphonomy's.
   M15 must be run against the same images and agree.
2. The second method covers 54 of the recoveries.
3. macOS's own files are judged by format, which proves the format and
   not identity: a recovered file of the right format could still be
   another file of that format.
4. Only part of NIST's layout document survived text extraction.
5. The NIST images are from 2011 and 2012, made on one macOS release.
   The Linux image was made in 2020 by one copy and one `rm -rf`.
6. The scripts do not implement ADR-0022's bitmap gate, orphan-stream
   claims, bounds or hand-back rule. None of these arises on these
   images, where no deleted set is unusable and no Stream Extension is
   orphaned. They read a deleted directory only where every cluster is
   free and none was read before, as ADR-0022 Decision K does.

### External practice

NIST's page notes that tests 02, 03, 05, 05-braid and 05-nest "often
produce interesting results, especially for the FAT images", because a
tool that has only a first cluster and a size guesses at the rest. On
exFAT the chain answers that question where the deleting system kept
it.

### Next action

1. ADR-0022, ADR-0019 Appendix C, ADR-0014 Appendix F, ADR-0003
   Appendix C.
2. M15, accepted against these images (ADR-0022 section 14).
3. M16: recovering inside deleted directories, against DFR-11, DFR-12
   and the Linux image.
4. GPT, so that NIST's nine GPT images can be run through Taphonomy.
