# Taphonomy Changelog

This document records notable changes to Taphonomy across releases, including
new capabilities, fixes, and breaking changes, so that the history of the
project's behavior can be reviewed without reconstructing it from commit
history.

## Unreleased

### Added

- exFAT volumes are analysed, read-only (`ADR-0019`, `ADR-0021`):
  - the boot region, from the backup region when the main one is
    refused, found by trying each sector size when needed;
  - every directory the root reaches;
  - every live file and directory, with a live file's content readable
    through the library.

  Deleted entry sets are counted and not examined. TexFAT is identified
  and not analysed.
- A partition of type `0x07` whose first sector identifies nothing is
  tried as exFAT from its backup boot region. It is reported as
  "filesystem exFAT, identified from its backup boot region".
- Three new coverage lines: "entry sets not used", "deleted sets not
  examined" and "entries after end marker". Each is a gap.
- A subdirectory whose allocation exceeds its DataLength is reported as
  a directory not read. That covers a chain that goes on, a last link
  that is not end-of-chain, and a first cluster with a DataLength of 0.
- New public modules `taphonomy::exfat_boot`,
  `taphonomy::exfat_directory` and `taphonomy::analysis::exfat`, and
  fourteen `Event` variants whose names begin `ExFat`. No stability is
  promised for these yet (`ADR-0021` section 14).

### Fixed

- A recovered file's name now includes its partition, so two FAT32
  partitions on one image no longer collide when an entry shares the
  same directory cluster and slot in each.
- A single malformed partition table entry (zero length, or claiming
  sectors past the end of the evidence) no longer causes the entire
  table to be refused. The entry is reported as an anomaly and counted
  as a gap; every other entry on the table is still analysed.
- A repeated `--output` or `--reference-digest` flag is now an argument
  error instead of silently letting the last one given win.
- A FAT32 volume that declares no backup boot sector (`BPB_BkBootSec`
  of `0` or `0xFFFF`) or no FSInfo sector (`BPB_FSInfo` of `0xFFFF`) is
  no longer refused. `mkfs.fat` writes `0` for the backup when the
  reserved area is small, so such volumes were reported as `coverage
  none` although they were entirely readable.

### Changed

- The code that hashes and writes a recovered artifact moved from
  `taphonomy::fat_recovery` to the new public module
  `taphonomy::extraction`. `Destination`, `Extraction` and `Output` are
  still re-exported from `fat_recovery`, so existing paths keep working.
- The read-back of a written artifact reads in 64 KiB chunks and not in
  cluster-sized ones. What is written, hashed and reported is unchanged.
- An exFAT partition, previously counted as a filesystem identified and
  not analysed, is now analysed, so a run's coverage on an exFAT disk
  changes accordingly.

## 0.1.0 - 2026-09-24

### Added

- Read-only evidence file access with SHA-256 hashing (ADR-0002 M1)
- CLI reporting evidence path, reported size, bytes read, and digest
- SHA-256 verification against NIST FIPS 180-4 vectors (ADR-0004 §5.5)
- Evidence immutability tests covering permissions, mtime, and contents
- MBR partition table parser with bounds validation against evidence size
  (ADR-0002 M2, ADR-0005)
- GPT protective MBR detection, reported as unsupported rather than parsed
  (ADR-0005 §4)
- Deterministic synthetic fixture generation for partition parsing
- Positional evidence reads via `EvidenceFile::read_exact_at`
- Filesystem identification from volume boot record structure (ADR-0002 M3)
- FAT variant determined by data-region cluster count per the FAT
  specification, not by the advisory type string at offset 0x52
- Detection of disagreement between declared MBR partition type and observed
  filesystem
- exFAT and NTFS recognised and reported as unsupported
- Partition anomalies and filesystem observations reported as readable
  text rather than debug-formatted structures
- FAT32 boot sector parsing validated against the partition extent
  (ADR-0002 M4)
- Detection of a volume declaring more sectors than its partition holds,
  a root directory cluster outside the data region, and a file allocation
  table too small for the clusters declared
- Reporting of a hidden sector count that records a start the volume does
  not have
- FAT32 boot sector detail reported by the CLI, including root cluster,
  backup boot sector location, FAT mirroring state, and volume label
- Rejection of an active FAT index at or beyond the declared FAT count
- FAT32 root directory enumeration by cluster chain walk (ADR-0002 M5,
  ADR-0008)
- Directory entry classification covering the volume label, 8.3 names,
  long-name components, and the attribute combination the specification
  names invalid
- Long-name entries retained in on-disk order and counted, not decoded
  (ADR-0007 §5)
- Cluster chain links read from the active FAT rather than from FAT 0
  unconditionally (ADR-0008 §4)
- Reporting of non-zero content found after a directory terminator, which
  a conformant volume does not contain (ADR-0008 §5)
- Refusal of a cyclic cluster chain, a chain link outside the data region,
  and a directory exceeding the specification's maximum size
- Root directory contents reported by the CLI, including the cluster chain
  and the position of every entry
- Deleted directory entry identification (ADR-0002 M6, ADR-0009)
- Classification of what a deleted entry was, with the fields deletion
  destroys omitted rather than computed, so that a destroyed long-name
  ordinal cannot be read as a valid one (ADR-0009 §3, EXP-0003 §4)
- Recovery of a short name's destroyed first byte from the checksum a
  surviving long-name entry carries, which determines it rather than
  narrowing it (ADR-0007 Appendix C, ADR-0009 §6.1)
- Rejection of a long-name association whose recovered byte is one a live
  entry never holds, which proves the set belongs to another entry
  (ADR-0009 §6.2)
- Entries beyond a directory terminator classified and reported separately
  from the allocated listing, rather than counted (ADR-0008 §5,
  ADR-0009 §4)
- Deleted entry count reported by the CLI alongside the short and long-name
  counts
- Fixtures carrying deleted 8.3 entries, a complete long-name set, a
  partial one left by slot reuse, a removed subdirectory, and entries past
  a terminator (ADR-0006 §5.1, ADR-0009 §8)
- Measurement of what mtools deletion destroys and leaves, and of what a
  fixture built with it can and cannot prove (EXP-0003)
- Recovery of the data of an unfragmented deleted file in the FAT32 root
  directory, extracted to memory and reported as a digest with the number
  of bytes it covers (ADR-0002 M7, ADR-0010)
- Refusal of a recovery whose implied cluster run reaches a cluster the
  active FAT reports as in use, naming the cluster that caused it
  (ADR-0010 §5)
- Eligibility reported as named refusals rather than silence, covering a
  deleted directory, a volume label, a long-name component, the invalid
  attribute pair, a zero size, a reserved first cluster, and a run
  extending past the last data cluster (ADR-0010 §6)
- Extraction streamed one cluster at a time, so memory does not scale with
  a size read from evidence; file slack is read, excluded from the digest,
  and its size reported (ADR-0010 §7)
- A project-owned incremental SHA-256 hasher, with `hash_reader` written
  over it so one implementation does the hashing (ADR-0010 §8)
- `--recover`, placing the reading of a deleted file's content behind an
  explicit request; without it the implied run and its allocation status
  are reported and no content is read (ADR-0010 §4)
- Fixtures carrying a four-cluster deleted file with its run intact, a
  single-cluster deleted file, a deleted directory, and a poked size whose
  implied run reaches a cluster a live file holds (ADR-0006 §5.1,
  ADR-0010 §10)
- Validation of recovered content against a digest the operator supplies,
  reported as a match or a difference alongside the number of bytes
  compared (ADR-0002 M8, ADR-0013)
- `--reference-digest`, accepting 64 hexadecimal characters in either case
  and rejecting everything else, including a pasted `sha256sum` line whose
  trailing filename would otherwise be discarded in silence (ADR-0013 §6.1)
- A reference that cannot be read stopping the run before any evidence is
  opened, which is the only condition M8 fails closed on (ADR-0013 §3.4)
- A reference supplied without `--recover` reported as an argument error
  rather than implying a flag that reads content (ADR-0013 Decision H)
- A differing digest reported as a finding about the evidence rather than a
  failed operation, so the run continues and the exit status is zero
  (ADR-0013 §3)
- The four conditions a differing digest is consistent with, enumerated
  without one of them being chosen (ADR-0013 Decision E)
- The limit of a match stated alongside it: byte equality with what was
  supplied, which establishes less where the content is not distinctive
  (ADR-0013 §8.2)
- `Sha256Digest::from_hex`, which reads a recorded digest and computes none
  (ADR-0013 §6.2)
- A `validation` module that compares digests, reads no evidence and names
  no filesystem structure, keeping validation separate from extraction
  (`docs/ARCHITECTURE.md` §5.6, ADR-0013 §11)
- Command-line argument handling covered by tests that run the binary,
  including that two of its checks precede opening the evidence
  (ADR-0013 §16)
- A fixture holding a deleted file whose three clusters were not adjacent,
  with the clusters between its fragments freed as well, built with
  `mtools` alone and containing no poked field (EXP-0004)
- Measurement of incorrect recovery on that fixture: three of its five
  deleted entries recover content that is not their own file's, and the
  tool reports all five identically (`CLAUDE.md` §26)
- A confidence level carried by a reconstructed artifact and reported once
  per run, the only level `ADR-0003`'s model leaves reachable for this tool
  (ADR-0002 M9, ADR-0014 Decision A)
- A coverage statement reporting what a run did not analyse, as a count per
  kind of gap and a status of complete, incomplete or none (ADR-0014
  Appendix B.5)
- A directory that was listed and not read counted as a gap, live or
  deleted, which four fixtures hold and no run previously reported
  (ADR-0014 Appendix B.2)
- Counts of the deleted entries that produced no artifact, by what stopped
  each one, so that an operator knows which questions went unanswered
  (ADR-0013 Appendix C.4)
- Artifact and comparison counts reported for the run rather than per
  entry, which `ADR-0003` §4.7 requires of a session (ADR-0014 Appendix B.8)
- `--output <directory>`, writing each recovered artifact to a file named
  from the entry's slot and first cluster, so no byte of evidence reaches
  the path (ADR-0015 Decisions A and D)
- Every directory the root reaches read: a live one along its cluster chain,
  a deleted one from its first cluster alone (ADR-0016 Decisions A and B)
- A deleted directory's first cluster read only where the FAT marks it free
  and it carries `.` naming that cluster and `..`, and reported with the
  check it failed otherwise (ADR-0016 Decision C)
- A listing that may continue beyond a deleted directory's first cluster
  counted as a coverage gap of its own kind, the thirteenth (ADR-0016
  Decision D)
- Bounds on the walk: no cluster read twice, and nothing deeper than 128
  levels below the root, each reported as a gap (ADR-0016 Decision E)
- Fixtures for a deleted subtree, a deleted directory whose clusters are not
  adjacent, 130 nested directories, a directory loop, and two deleted
  entries sharing a slot and a first cluster (EXP-0005, EXP-0006)
- Read-back verification of every written artifact: the file is re-opened,
  hashed, and reported as matching or differing from what was read, which
  states what landed rather than what was sent (ADR-0015 Decision H)
- Refusal of a destination holding the evidence, before the evidence is
  opened (ADR-0015 Decision B, corrected by its Appendix A)
- A twelfth coverage gap, for an artifact that was read and could not be
  handed to the operator (ADR-0015 section 9)
- A second fragmentation fixture, the same construction with the clusters
  between the fragments still allocated, on which the run the deleted entry
  implies reaches a live cluster and is refused rather than extracted. The
  refusal is reached with no poked field, where the existing collision
  fixture needs one (EXP-0004 Appendix A.5)
- A search, after the walk, of every data cluster the walk did not name,
  for a directory whose first entry names that cluster and whose FAT entry
  is free; each found is read under ADR-0016 Decision C and reported on its
  own with the cluster its `..` names (ADR-0017 Decisions A and B)
- The files an orphaned directory lists offered for recovery whether or not
  marked deleted, under a heading that does not call them deleted
  (ADR-0017 Decision C)
- A fourteenth coverage gap, for an orphan search an evidence error
  stopped, and a directory an orphaned listing names that the run never
  read counted as unread (ADR-0017 Decision D)
- Three fixtures: a quick-formatted DCF tree, the same volume written to
  once more, and a deleted tree deeper than the walk's bound with one dot
  entry broken (EXP-0007; ADR-0017 section 10)
- Continuous integration: on every push the fixtures are rebuilt on a clean
  runner and checked against the committed digests, then formatting, lint
  and the full test suite are run
- Validation against three NIST CFReDS deleted-file-recovery images and The
  Sleuth Kit (EXP-0008)

### Changed

- FAT BIOS parameter block parsing and variant determination moved from
  `filesystem.rs` into a dedicated `fat` module; `filesystem.rs` retains
  generic identification only
- `EntryKind::Deleted` carries what the surviving bytes establish the entry
  to have been; it previously carried nothing (ADR-0009 §3)
- `RootDirectory` carries the classified slots beyond the terminator; the
  observation reporting them is retained unchanged (ADR-0009 §4.4)
- `cluster_offset` and `read_fat_entry` widened from private to
  `pub(crate)`; neither moved (ADR-0010 §9)
- The in-memory `EvidenceReader` test double moved from `fat_directory.rs`
  into `evidence.rs`, where the trait it implements is declared, so both
  test modules share one rather than duplicating it (ADR-0010 Appendix C)
- The recovery digest line is labelled `recovered` and the comparison line
  `reference`, so the recovered-file hash and the validation hash are
  distinguishable wherever both appear (`docs/SAFETY.md` §10)
- A recovered artifact is named from the entry's own cluster as well as its
  slot and first cluster; a slot alone repeats in every directory cluster,
  so two entries could previously receive one name and the second be
  reported as a file that already existed (ADR-0016 Decision F)
- A destination that cannot be created is reported as not created; it was
  reported as a failure that had left a partial file behind, which an
  unwritable `--output` directory produced for every entry (ADR-0015
  Appendix B.1)
- A failed flush is a failed write: the file is removed and reported as not
  written, where it was reported as written and unverified (ADR-0015
  Appendix B.1)
- A partial file that survives an evidence failure is reported beside that
  failure; the removal's own failure was silent (ADR-0015 Appendix C)
- A partial file left by a failed write is removed when the evidence then
  fails too; only a file still open was removed (ADR-0015 Appendix B.1)
- Recovery caveats print once per volume rather than once per set of
  entries. On a volume holding a recoverable deleted entry past the
  directory terminator they previously appeared twice in one run
- The reporting functions take an options struct rather than a widening
  list of flags (ADR-0013 §13)
- A run that analysed nothing past the evidence digest exits 3. It
  previously exited 0 with nothing on standard output recording that
  nothing had been analysed (ADR-0014 Appendix B.10)
- Recovery caveats are computed from the run's counts rather than tracked
  separately alongside them, so the two cannot disagree (ADR-0014
  Appendix B.9)
- The free-run caveat says nothing has claimed the clusters since the entry
  lost them, where it said since deletion; an orphaned directory's files
  were never deleted
- A deleted directory's heading recovers its first character from the
  long-name component before it, as its entry in the listing already did;
  the two named one directory differently (EXP-0008)
