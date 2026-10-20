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
