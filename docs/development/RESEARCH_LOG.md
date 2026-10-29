# Taphonomy Research Log

This document records technical research that materially affects Taphonomy's architecture, implementation, safety model, or recovery strategy.

---

## 2026-08-28, Initial Recovery Ecosystem Research

### Objective

Determine how established open-source recovery and forensic systems approach data recovery, identify reusable architectural ideas, identify limitations, and determine where Taphonomy should build its own functionality.

---

## 1. TestDisk

**Project:** TestDisk
**Primary role:** Partition and filesystem recovery

TestDisk provides recovery capabilities for lost partitions, damaged filesystems, boot sectors, and deleted files. It supports storage devices including hard drives, USB drives, and memory cards, with support for filesystems including FAT, exFAT, NTFS, ext2, ext3, and ext4.

**Source:**

https://www.cgsecurity.org/testdisk_doc/

### Architectural observation

TestDisk demonstrates that partition recovery, filesystem recovery, and file recovery are related but distinct operations.

### Taphonomy implication

Taphonomy should maintain separate boundaries for:

```text
device
partition / volume
filesystem
file
```

These should not be collapsed into one recovery operation.

### Important limitation

TestDisk's broad recovery capabilities do not imply that every storage technology has the same recovery model.

SSD behavior, encryption, TRIM, filesystem corruption, and mobile-device security require separate investigation.

---

## 2. PhotoRec

**Project:** PhotoRec
**Primary role:** File carving

PhotoRec is designed to recover files from storage even when filesystem structures are severely damaged or unavailable. It can operate against physical media or disk images and intentionally avoids relying entirely on filesystem metadata.

PhotoRec documentation explicitly describes it as ignoring the filesystem and recovering underlying file data.

**Source:**

https://www.cgsecurity.org/testdisk_doc/

### Architectural observation

Filesystem-based recovery and file carving are fundamentally different recovery strategies.

A simplified model is:

```text
Filesystem Recovery
        │
        ├── metadata
        ├── directory structures
        ├── allocation information
        └── file mappings


File Carving
        │
        ├── signatures
        ├── structure
        ├── content patterns
        └── reconstruction heuristics
```

### Taphonomy implication

Taphonomy must support both strategies without forcing carving logic into filesystem parsers.

---

## 3. The Sleuth Kit

**Project:** The Sleuth Kit
**Primary role:** Disk image, volume, filesystem, and forensic analysis

The Sleuth Kit provides a C library and command-line tools for analysis of disk images, volume systems, and filesystems.

Its documented architecture separates concepts including:

```text
Disk Image
Volume System
File System
Data Units
Metadata
File Name
Application
```

**Sources:**

https://sleuthkit.org/sleuthkit/

https://www.sleuthkit.org/sleuthkit/docs/api-docs/4.15.0-develop/

### Architectural observation

The Sleuth Kit provides strong evidence for separating storage representation from filesystem analysis.

Its APIs are designed so filesystem analysis can operate against disk-image abstractions rather than requiring direct physical-device handling.

### Taphonomy implication

Taphonomy should establish a controlled evidence-reader abstraction early.

Conceptually:

```text
Physical Device
       │
       ▼
Acquisition
       │
       ▼
Evidence Image
       │
       ▼
Evidence Reader
       │
       ▼
Filesystem / Recovery Analysis
```

This is consistent with our existing architecture.

---

## 4. Autopsy

**Project:** Autopsy
**Primary role:** Graphical digital-forensics platform built around The Sleuth Kit and other tools

Autopsy uses a case-oriented workflow.

A case can contain multiple data sources, including disk images, disk devices, and logical files.

Its workflow separates:

```text
Case
 ↓
Data Source
 ↓
Ingest Modules
 ↓
Analysis
 ↓
Results
 ↓
Reporting
```

**Sources:**

https://www.autopsy.com/

https://www.sleuthkit.org/autopsy/docs/user-docs/4.23.0/

### Architectural observation

Autopsy demonstrates the usefulness of separating the underlying analysis engine from the user interface and case/reporting layer.

### Taphonomy implication

The CLI should remain an interface, not the recovery engine.

A future desktop interface should be able to use the same application and domain layers.

---

## 5. Foremost

**Project:** Foremost
**Primary role:** File carving

Foremost performs file carving using file headers, footers, and internal data structures.

It can operate on disk images and storage devices.

**Source:**

https://github.com/korczis/foremost

### Architectural observation

Signature-based carving can be implemented independently of filesystem metadata.

### Taphonomy implication

Taphonomy should eventually treat file-carving signatures and reconstruction algorithms as independent components.

### Limitation

Simple header/footer carving is insufficient for all file types, particularly files that are fragmented or whose structure cannot be determined from simple signatures.

---

## 6. Scalpel

**Project:** Scalpel
**Primary role:** File carving

Scalpel is a file-carving and indexing tool derived from Foremost.

The public Scalpel repository states that it is not actively maintained. It also notes that newer approaches such as PhotoRec have moved beyond simplistic header/footer-based carving.

**Source:**

https://github.com/sleuthkit/scalpel

### Architectural observation

Header/footer carving is useful but represents only one class of recovery technique.

### Taphonomy implication

Taphonomy should avoid defining "file carving" as a single algorithm.

Potential future carving levels:

```text
Signature Detection
       ↓
Structural Validation
       ↓
Extent Reconstruction
       ↓
Fragment Analysis
       ↓
Artifact Validation
```

---

# 7. Initial Architectural Conclusions

The first research pass supports several existing Taphonomy decisions.

### Conclusion 1, evidence must be abstracted

Analysis should operate against controlled evidence rather than directly against physical devices.

### Conclusion 2, filesystem recovery and carving must remain separate

A damaged filesystem may still contain recoverable files, and a filesystem may be unavailable while file contents remain recoverable.

### Conclusion 3, acquisition and analysis must remain separate

Creating an evidence image and interpreting its contents are different operations.

### Conclusion 4, validation must be independent

Extracting bytes is not equivalent to proving that a valid file has been recovered.

### Conclusion 5, recovery domains must remain independent

HDD, SSD, removable storage, mobile devices, and application artifacts have different technical constraints.

### Conclusion 6, the CLI must remain thin

The recovery engine should not depend on the CLI.

### Conclusion 7, existing projects are valuable research references

Taphonomy should learn from established systems rather than reimplementing known concepts without understanding them.

However, direct code reuse requires independent license and dependency review.

---

# 8. Important Unknowns

Fourteen questions were recorded. Four have since been closed by documented decisions.

Closed questions are retained below with the decision that closed them, so
this section reads as a record rather than a live list.

1. Which language should implement the core engine? **CLOSED** — closed by
   ADR-0001.
2. Which existing Rust crates are mature enough for evidence handling?
3. Should Taphonomy use or integrate The Sleuth Kit? **CLOSED** — closed by
   `DEVELOPMENT_ENVIRONMENT.md` §16, which excludes CPL/IPL/GPL licensed
   dependencies without a documented decision. The Sleuth Kit is CPL/GPL.
4. Which filesystem should be the first implementation? **CLOSED** — closed
   by ADR-0002.
5. Should the first capability be filesystem recovery or file carving?
   **CLOSED** — closed by ADR-0001 §11.
6. How should evidence images be represented?
7. Which image formats should be supported initially?
8. How should interrupted acquisition be represented?
9. How should recovery confidence be calculated?
10. How should fragmented files be reconstructed?
11. What can realistically be recovered from SSDs after TRIM?
12. How should Android acquisition be approached?
13. How should iOS acquisition be approached?
14. Which application artifacts are worth supporting first?

These questions must be answered through research and experiments rather than assumptions.

---

# 9. Current Research Direction

The next research phase should focus on:

```text
Rust
 ↓
Evidence Reader
 ↓
Disk Image Formats
 ↓
Filesystem Parsing
 ↓
Synthetic Recovery Laboratory
 ↓
First Recovery Capability
```

Mobile recovery and advanced application recovery should remain later research tracks.

---

# 10. Current Decision

No recovery implementation should begin yet.

The immediate objective is to establish the development language, evidence abstraction, testing laboratory, and first recovery target using evidence from existing implementations and technical specifications.

---

## 2026-10-20, exFAT: a cluster chain longer than its DataLength

### Objective

Decide what an exFAT reader should do with a chain that goes on past
what an entry's DataLength needs, before recording M14's decisions in
`ADR-0021`.

### Sources

* Microsoft, *exFAT file system specification*, the Markdown source in
  `MicrosoftDocs/win32`, `desktop-src/FileIO/exfat-specification.md`,
  SHA-256
  `09454c9663cbd7b33c6dfe3ad37baa1351779ad5ab73034c53fcf864ccdee825`,
  downloaded twice with the same digest. Sections 4.1.3, 6.2.3,
  6.3.4.2, 7.6.7, 8.2.
* exfatprogs, `fsck/fsck.c`, commit `8b42748`: `check_clus_chain`,
  called through `check_inode` from `read_file` for every File entry
  set, directories included. GPL-2.0-or-later; read, not copied.
* Linux, `fs/exfat/inode.c` and `cache.c`, commit `f47d223`:
  `exfat_map_cluster` and `exfat_get_cluster`. GPL-2.0-or-later; read,
  not copied.
* The Sleuth Kit, tag `sleuthkit-4.12.1` (`8049377`):
  `tsk/fs/fatfs_meta.c`, `fatfs_make_data_runs`, which
  `tsk/fs/exfatfs.c` line 478 installs as exFAT's attribute loader.
  Common Public License 1.0; read, not copied.

### Findings

1. For a directory, the specification settles it. §7.6.7: DataLength is
   "the entire size of the associated allocation". §4.1.3: `FFFFFFFFh`
   "is the only valid value for the last FatEntry of any given cluster
   chain". A longer chain, or a first cluster with a DataLength of 0,
   breaks both.
2. For a file, it does not. §6.2.3 says DataLength is the size of the
   data the allocation contains. §6.3.4.2 sizes a contiguous allocation
   from DataLength. No clause says whether a chain may be longer.
3. exfatprogs' checker treats a longer chain as an error,
   `ER_FILE_SMALLER_SIZE` ("more clusters are allocated"), for files and
   directories, and repairs it by truncating. It also treats a first
   cluster with a DataLength of 0 as an error, `ER_FILE_FIRST_CLUS`.
4. The Linux driver and The Sleuth Kit never look: each maps only the
   clusters the size needs and stops.
5. At `8ea143c` Taphonomy refused a longer chain for file content and,
   for a directory, read to DataLength and stopped without looking at
   the last link. A read-only scan of EXP-0009's images A and B found
   neither shape on either; each has one live subdirectory.

### Implications

A checker reports the condition as damage; a reader that only needs the
data never sees it. Taphonomy reports on evidence and takes the
checker's view. `780d6f0` made a directory with either shape a
directory not read, counted as a gap; `ADR-0021` Decisions I and L
record both rules.

### Confidence

High for findings 1 to 4: the specification and three code bases, each
read at a pinned revision. Finding 5 rests on a scan written separately
from Taphonomy and on its two-image scope.

### Follow-up

* A file with a first cluster and a DataLength of 0 is still accepted;
  recorded in `KNOWN_ISSUES.md`.
* No fixture holds either directory shape. One would need a volume
  edited after it was made, because no tool measured here writes one.

---

## 2026-10-21, exFAT: deleted entry sets, the allocation bitmap and timestamps

### Objective

Establish, before M15, how established implementations treat deleted
exFAT entry sets and the allocation bitmap, what the specification
says about timestamps and their UTC offsets, and what is known of
attacks on the bitmap, so that ADR-0022 rests on more than one source.

### Sources

* Microsoft, *exFAT file system specification*, the Markdown source in
  `MicrosoftDocs/win32`, SHA-256
  `09454c9663cbd7b33c6dfe3ad37baa1351779ad5ab73034c53fcf864ccdee825`,
  a third download with the same digest. Sections 6.2.1.4, 7.1 to
  7.1.5.1, 7.4 to 7.4.10.2.
* The Sleuth Kit, commit `51d8672`: `tsk/fs/exfatfs.c`,
  `exfatfs_meta.c`, `exfatfs_dent.c`, `fatfs_meta.c`. Common Public
  License 1.0; read, not copied.
* TestDisk, commit `0aa4bc2`: `src/exfat_dir.c`. GPL-2.0-or-later;
  read, not run, not copied.
* exfatprogs, commit `8b42748`: `fsck/fsck.c`, `read_bitmap`.
  GPL-2.0-or-later; read, not copied.
* Linux, commit `f47d223`: `fs/exfat/balloc.c`, `dir.c`, `fatent.c`.
  GPL-2.0-or-later; read, not copied.
* CVE-2025-40307, the cve.org record, and its fix in `balloc.c`
  (`exfat_test_bitmap_range`).
* R. Nordvik and S. Axelsson, *It is about time: Do exFAT
  implementations handle timestamps correctly?*, Forensic Science
  International: Digital Investigation 42-43 (2022) 301476, CC BY,
  DOI 10.1016/j.fsidi.2022.301476, confirmed through Crossref. Its
  corrigendum (DOI 10.1016/j.fsidi.2023.301542) was not read.

### Findings

1. **Deleted sets.** On Windows, deletion clears the InUse bit of each
   entry of the set and changes nothing else in it (EXP-0009). Linux
   does the same per entry (`exfat_remove_entries`, which can also clear
   only a set's trailing entries) and writes no FAT entry when it frees
   clusters.
2. **The Sleuth Kit never checks a SetChecksum.** `exfatfs_dent.c`
   stores the field in `expected_check_sum` and nothing reads it.
   Checked twice, by different searches. A deleted file is recovered
   from its first cluster onward, skipping allocated clusters, with the
   chain and NoFatChain ignored (`fatfs_meta.c`). This confirms in the
   source what EXP-0009 inferred from its output.
3. **TestDisk follows the FAT for every deleted file, NoFatChain
   ignored**, although §6.3.4.2 says those entries are then not to be
   interpreted. Where the first link is 0 it takes the next clusters
   whose FAT entry is 0. It reads to DataLength, not ValidDataLength,
   checks no SetChecksum and no bitmap, and continues after a read
   error.
4. **The bitmap is a chain.** §7.1.3: its FirstCluster is "the first
   cluster of the cluster chain, as the FAT describes". The Sleuth Kit,
   exfatprogs and Linux all read it as contiguous from that cluster.
5. **A crafted bitmap is a known attack.** CVE-2025-40307: a volume
   whose bitmap leaves the bitmap's own clusters free made Linux
   reallocate them. Linux now refuses to mount unless they are marked
   in use.
6. **Timestamps are local.** §7.4.5 to §7.4.10: each timestamp is a
   local time, with a 10 ms field of 0 to 199 for creation and
   modification, and an offset in 15-minute steps with an OffsetValid
   bit. With OffsetValid 0 an implementation "shall treat" the time as
   having the reader's current offset, which for evidence means the
   offset is unknown.
7. **Implementations differ.** Nordvik and Axelsson measured that
   Windows 10 follows the specification, macOS writes the offset with
   its sign switched, exfat-fuse 1.3 writes `0x00` (invalid), and the
   native Linux driver writes `0x80` (UTC+0). EXP-0011 saw the macOS
   sign on NIST's images and `0x80` on a Linux-made image.

### Implications

* A deleted set is used only with its checksum valid, which no prior
  implementation read here does (ADR-0022 Decision B).
* The bitmap is read through its chain and checked against the
  structure the walk knows to be in use (ADR-0022 Decision E).
* Times are ordered only under the same valid offset byte (ADR-0019
  Appendix C).

### Confidence

High for findings 1 to 6, each read from source or the specification;
finding 2 checked twice. Finding 7 rests on the paper, and on EXP-0011
for the macOS sign and the Linux byte. TestDisk's behaviour is read,
not measured.

### Follow-up

* Read the paper's corrigendum.
* Measure Linux's deletion of a fragmented file, which EXP-0011 did not
  exercise.

---

## 2026-10-21, exFAT: the up-case table, bad clusters and live files in the bitmap check

### Objective

Establish, before ADR-0022 Decision E's check is implemented, how
established implementations locate the up-case table, how they treat a
bad cluster and a second up-case entry, and whether they check live
files against the bitmap, so that ADR-0022 Appendix A rests on more
than the specification.

### Sources

* Microsoft, *exFAT file system specification*, the Markdown source
  with SHA-256
  `09454c9663cbd7b33c6dfe3ad37baa1351779ad5ab73034c53fcf864ccdee825`
  (the same file as the entry above). Sections 7.1.5.1 and 7.2 to
  7.2.5.1.
* Linux, commit `f47d223`: `fs/exfat/balloc.c`
  (`exfat_test_bitmap_range`, `exfat_find_free_bitmap`) and
  `fs/exfat/nls.c` (`exfat_create_upcase_table`,
  `exfat_load_upcase_table`). GPL-2.0-or-later; read, not copied.
* exfatprogs, commit `8b42748`: `fsck/fsck.c` (`read_upcase_table`,
  `check_clus_chain`, `write_bitmap`) and `mkfs/`. GPL-2.0-or-later;
  read, built and run, not copied.
* The Sleuth Kit, commit `51d8672`: `tsk/fs/exfatfs_meta.c`
  (`exfatfs_copy_upcase_table_inode`). Common Public License 1.0; read,
  not copied. Its 4.12.1 release, as installed, was run.
* relan's exfat, commit `1264da3`: `mkfs/`. GPL-2.0-or-later (header of
  `mkfs/uctc.c`); built and run, not copied.
* FatFs R0.16 (`ff16.zip`, SHA-256
  `99f7dc1f7e095356e4a9e3dbe29959090d8b948afe2bbc5441e52fdf4b85449e`),
  ChaN's licence of one condition, retaining the notice (header of
  `source/ff.c`): `f_mkfs`. Built and run, not copied.

### Findings

1. **No implementation read follows the up-case table's chain.** Linux
   reads it as contiguous sectors from its first cluster
   (`exfat_load_upcase_table`), exfatprogs reads it in one piece from
   that cluster (`read_upcase_table`), and The Sleuth Kit says so in a
   comment: "There is no FAT chain walk for the upcase table". §7.2.3
   says the first cluster begins "the cluster chain, as the FAT
   describes". The same split as for the bitmap.
2. **Linux's CVE-2025-40307 fix checks only the bitmap's own clusters**
   (`exfat_test_bitmap_range`), not the up-case table, the root or any
   file.
3. **exfatprogs checks every file and directory cluster against the
   bitmap**: `check_clus_chain` reports "cluster ... is marked as free"
   (`ER_FILE_INVALID_CLUS`). The bitmap's and the up-case table's
   clusters it marks in use itself, silently (`write_bitmap`). It treats
   an up-case length of 0, an odd length, or more than `0x10000 * 2`
   bytes as an error; Linux treats a length of 0 as one.
4. **Linux allocates without reading the FAT.**
   `exfat_find_free_bitmap` returns the first clear bit. A bad cluster
   whose bit is clear would be allocated.
5. **No open-source writer read creates a bad cluster.** exfatprogs'
   `mkfs` has no surface scan, and Linux only reads `FFFFFFF7h`
   (`fatent.c`). Windows' `format` was not examined.
6. **exfatprogs' checker judges neither case.** On a volume made by its
   own `mkfs`, `fsck.exfat -n` reported clean with a free cluster set to
   `FFFFFFF7h` in the FAT and its bit clear, with the same and its bit
   set, and with a bit set for a cluster nothing claims. With the root's
   up-case entry copied into its first unused slot, `-n` and `-y` both
   reported clean and left both entries. Linux returns at the first
   up-case entry it finds. The Sleuth Kit 4.12.1's `fls -f exfat` lists
   both, without an error.
7. **FatFs writes an up-case table of its own.** `f_mkfs` builds a
   compressed table from its own case mapping (`ff.c`, "Create a
   compressed up-case table"), 4,104 bytes, where exfatprogs and relan's
   exfat write the recommended 5,836 (EXP-0011 Appendix A). FatFs also
   writes the bitmap, the up-case table and the root as FAT chains
   ("Create chains of bitmap, up-case and root directory").

### Implications

* ADR-0022 Appendix A: the check covers live allocations, as
  exfatprogs' checker does (finding 3), because condition 1 alone keeps
  a deleted set off live data.
* A rule that the up-case table holds 5,836 bytes would be wrong
  (finding 7); only a bound on its length is needed.
* No tool tested refuses a second up-case entry (finding 6). ADR-0022
  Appendix A refuses it, which costs only assessment.

### Confidence

High for findings 1 to 4 and 7, each read from source; finding 7 also
measured on volumes FatFs made. Finding 5 holds for the sources read.
Finding 6 is measured, once before and once after a restart of the
machine, with the same results.

### Follow-up

* Whether Windows' `format` marks the bad clusters a surface scan finds
  in the bitmap.
* What Windows and macOS do with a second up-case entry. Windows needs
  a virtual disk attached with administrator rights; macOS was not
  available.

---

## 2026-10-29, exFAT: the UTC offsets writers record, and reuse after deletion

### Objective

Establish, before condition 2 is implemented (`ADR-0022` Appendix C),
which UTC offset bytes established writers record, whether any can write
two that differ on one volume, and how a writer reuses clusters and
directory slots after a deletion, so that the volumes EXP-0011 Appendix
C makes are understood rather than assumed.

### Sources

* FatFs R0.16 (`ff16.zip`, SHA-256
  `99f7dc1f7e095356e4a9e3dbe29959090d8b948afe2bbc5441e52fdf4b85449e`),
  ChaN's licence of one condition, retaining the notice (header of
  `source/ff.c`): `f_open`, `create_xdir`, `f_sync`, `f_utime`,
  `mount_volume`. Built and run, not copied.
* relan's exfat, commit `1264da3` (archive SHA-256
  `f5957b7602d457f503e6b3d5b2291bcd14489425e09fa20d3160c2adbcb96874`),
  GPL-2.0-or-later (header of `libexfat/time.c`): `exfat_unix2exfat`,
  `exfat_tzset`, and `exfat_mount` in `libexfat/mount.c`. Built and run
  through its library, not copied.
* Linux, commit `f47d223`, GPL-2.0-or-later (header of
  `fs/exfat/misc.c`): `exfat_set_entry_time` and `exfat_get_entry_time`
  in `fs/exfat/misc.c`, the `time_offset` option in `fs/exfat/super.c`,
  and `__exfat_free_cluster` in `fs/exfat/fatent.c`. Read, not run.
* exfatprogs, commit `8b42748`, GPL-2.0-or-later: `lib/exfat_dir.c`
  (`unix_time_to_exfat_time`). Read, not copied.
* The Sleuth Kit, commit `51d8672`, Common Public License 1.0:
  `tsk/fs/tsk_exfatfs.h`. Read, not copied. Its wiki page *ExFAT
  Implementation Notes*,
  <https://wiki.sleuthkit.org/ExFAT-Implementation-Notes/>, last
  modified 2014-06-03.
* relan's exfat `ChangeLog`, in the archive above.
* R. Nordvik and S. Axelsson, "It is about time - Do exFAT
  implementations handle timestamps correctly?", *Forensic Science
  International: Digital Investigation* 42-43 (2022) 301476,
  doi:10.1016/j.fsidi.2022.301476, and its corrigendum, 45 (2023)
  301542, doi:10.1016/j.fsidi.2023.301542. Their existence and
  bibliographic details are confirmed through Crossref; their text could
  not be opened here (the publisher refused the request), so what they
  say is reported below as read by another research session, not
  verified.
* Microsoft, *exFAT file system specification*, the Markdown source with
  SHA-256
  `09454c9663cbd7b33c6dfe3ad37baa1351779ad5ab73034c53fcf864ccdee825`,
  sections 7.4.10 to 7.4.10.2.

### Findings

1. **FatFs records no offset.** `f_open` and `create_xdir` clear a new
   File entry, its offset bytes included, before writing its times, and
   `f_sync` and `f_utime` write `00h` beside every time they set.
   OffsetValid is therefore clear on every set FatFs writes. Measured on
   EXP-0011 Appendix C's four FatFs volumes: `00h` on every set.
2. **relan's exfat records the local offset, valid.**
   `exfat_unix2exfat` writes the offset from UTC in 15-minute units with
   bit 7 set, from a value `exfat_tzset` computes from TZ and the clock
   when the volume is mounted. A host that changes TZ and calls
   `exfat_tzset` again writes another byte on the same volume. Measured:
   `90h` and `94h` on one volume (EXP-0011 Appendix C).
3. **Linux records UTC, valid.** `exfat_set_entry_time` writes the time
   in UTC and the offset byte `80h` (OffsetValid set, offset 0) whatever
   the mount options. `time_offset` and `sys_tz` are used only by
   `exfat_get_entry_time`, for an entry whose OffsetValid is clear. At
   this commit, then, `exfat_set_entry_time` writes no other byte. Read,
   not measured here; EXP-0011 measured `80h` on every set of
   forensics-samples' Linux image.
4. **Linux frees a cluster in the bitmap only.** `__exfat_free_cluster`
   clears bitmap bits and writes no FAT entry, so a deleted file's chain
   survives until another allocation rewrites it.
5. **FatFs allocates from the heap's start after a mount, and reuses a
   deleted set's slots.** `mount_volume` sets the allocation hint
   (`last_clst`) invalid, so the next allocation searches from the first
   cluster and takes freed clusters first. A new set takes the first run
   of free slots large enough; where it ends on a deleted set's File
   entry, that set's Stream Extension and names stay behind, unused and
   outside any set. Measured on EXP-0011 Appendix C's volumes; read in
   source for the hint.
6. **Linux stamps every time it writes back as valid UTC.** On every
   write of a file's entry, `__exfat_write_inode` (`fs/exfat/inode.c`)
   passes its creation, modification and access times through
   `exfat_set_entry_time`, which writes `80h` beside each. A time read
   from an entry whose OffsetValid was clear was first converted with
   `time_offset` or `sys_tz` (finding 3), 0 unless the mount says
   otherwise, so an entry another writer stored as local time with `00h`
   can come back as the same digits stamped valid UTC. Read in source;
   that this happens to a given file is inferred, not measured.
7. **exfatprogs' fsck writes `80h`.** `unix_time_to_exfat_time` sets
   `*tz = 0x80` for the entry sets it builds.
8. **The Sleuth Kit ignores the offset bytes.** At `51d8672` the three
   fields are declared in `tsk_exfatfs.h` and used nowhere else; its wiki
   says "At present, TSK does not store this time zone information."
9. **relan's exfat recorded offsets from 1.4.0.** Its `ChangeLog` lists
   "Added support for timezone offsets" under 1.4.0 (2023-03-02) and not
   under 1.3.0 (2018-09-15).
10. **The Sleuth Kit's wiki reports macOS switching the sign, and two
    writers that record no offset.** "Files created on a system set for
    UTC-5:00 end up with a timezone for UTC+5:00"; "the timezone fields
    are empty in some older versions of Windows, such as Vista SP1"; and
    "the Panasonic Lumix camera is leaving the TZ fields as zero on the
    SDXC cards". One source for each, from 2014; the versions are not
    stated.
11. **Reported, not verified here: the 2022 paper and its corrigendum.**
    Another research session read them and reports that Windows 10
    writes the offset in force at each write, daylight saving included,
    and rewrites the modification and access offsets but not the
    creation's when a file changes; that Linux's driver turned `00h`
    offsets into `80h` without changing the times; that macOS rewrote
    every file's access offset on mounting; and that the corrigendum
    corrects four other points and leaves macOS's switched sign as
    reported.

### Implications

* Condition 2 can order no two sets FatFs wrote (finding 1): on a FatFs
  volume, deleted sets sharing a cluster are all refused. `ADR-0022`
  Appendix C keeps the OffsetValid requirement and records the cost.
* Differing valid bytes on one volume need a writer that records local
  time (finding 2), as the writers of EXP-0009's and NIST's images did
  (EXP-0011), and not Linux at `f47d223` (finding 3).
* An orphan Stream Extension arises from ordinary slot reuse (finding
  5), so its claim matters on volumes no one crafted.
* An equal, valid offset byte does not prove that two times were written
  under one offset. Linux can stamp `80h` on local-time digits (finding
  6), and on a card used by Windows and by macOS one byte means opposite
  offsets (findings 10 and 11). Condition 2 may then order two sets
  wrongly, and recover the earlier. `ADR-0022` Appendix C records it as
  a limit, to be measured on the paper's published images.

### Confidence

High for findings 1 and 2, each read from source and measured; and for
finding 5, measured, and read in source for the allocation hint. Finding
3 is read from source and agrees with EXP-0011's measured byte; findings
4, 6 and 7 are read only. Finding 8 rests on source and the wiki,
finding 9 on the ChangeLog alone, finding 10 on one wiki page. Finding
11 is not verified.

### Follow-up

* Measure a volume written by Windows or macOS across a daylight-saving
  change, which the cost of the OffsetValid and same-byte rules depends
  on (`ADR-0022` section 16).
* Find which devices write exFAT with FatFs or another writer that
  records no offset; one camera is named (finding 10), not measured.
* Score condition 2 on the paper's published images (Mendeley Data,
  doi:10.17632/krjsmdc65h.2), written by Windows 10, macOS and Linux,
  before stage 6 decides whether close times need a margin.
* Read the paper and its corrigendum directly.
