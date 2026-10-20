# ADR-0021: M14 exFAT Read-Only Analysis Decisions

* **Status:** Accepted
* **Date:** 2026-10-20
* **Decision owners:** Taphonomy project
* **Scope:** How M14 reads an exFAT volume's boot region, directories and
  live files, what it refuses and counts, the bounds it holds itself to,
  and the choices the specification leaves open
* **Related:** `ADR-0014` Appendix B.4, B.5, §11; `ADR-0016` Decision E;
  `ADR-0018` §5, §6; `ADR-0019` §3, §6, Decisions B, C, F, Appendix B;
  `ADR-0020`
  §4, §5; `docs/SAFETY.md` §12; EXP-0009 and its Appendix A; Microsoft,
  *exFAT file system specification*, Markdown source, SHA-256
  `09454c9663cbd7b33c6dfe3ad37baa1351779ad5ab73034c53fcf864ccdee825`

---

## 1. Context

`ADR-0019` decided that exFAT is the next filesystem and what a recovery
from it may claim. It left the reading itself to the code: its §3 says which
boot region is used and that entry sets are checked, and nothing about
what a damaged first sector, an unknown entry type or a hostile directory
tree does to a run.

M14 built that reading in three commits: the boot region (`e7985de`),
directories and live entries (`eabedde`), and live file content with its
acceptance test (`97c9242`). Each made choices the specification does not
make, or makes in a way that needed reading closely. The audit for this
record found one place where the code was more lenient than the
specification allows, and `780d6f0` corrected it (Decision L). This
record collects them. Deleted files are M15.

Section numbers below are the specification's unless a document is named.

---

## 2. Decisions

* **A.** The backup boot region may be found by trying each sector size,
  and is used only if exactly one size yields a valid region that
  declares that size.
* **B.** A partition the table declares as type `0x07`, whose first
  sector identifies nothing, is tried as exFAT from its backup boot
  region. No other partition type is.
* **C.** TexFAT is counted as identified and not analysed. A backup
  search that finds two valid regions is counted as a refused boot
  region.
* **D.** What the specification says makes a set, a directory or a volume
  unusable is applied at that level and no wider.
* **E.** A directory walk reads each cluster at most once per volume and
  holds itself to two bounds this project chose.
* **F.** Three new gap kinds: entry sets not used, deleted sets not
  examined, and entries after an end marker.
* **G.** Names stay UTF-16 in the library. The CLI escapes every code unit
  that is not printable ASCII, and the backslash and slash.
* **H.** exFAT entry lines carry a fixed 20-character position column.
* **I.** A file's chain is walked whole before any of its data is read,
  and must be exactly as long as its DataLength needs.
* **J.** Content up to ValidDataLength is read; the rest is handed over
  as zeros.
* **K.** The acceptance tables are produced by other tools, never by
  Taphonomy.
* **L.** A subdirectory's allocation must be exactly its DataLength. A
  chain that goes on past it, a last link that is not end-of-chain, and
  a first cluster with a DataLength of 0 each stop that directory.

---

## 3. The boot region (Decisions A, B, C)

`ADR-0019` Decision B uses the main boot region if its checksum is valid,
otherwise the backup. M14 also requires every ranged field of §3.1 to be
valid, so a region whose checksum matches and whose geometry is impossible
is refused like one whose checksum does not.

**Finding the backup.** The backup starts twelve sectors in (§3), and the
sector size is a field of the main region (§3.1.14). A main region refused
because that field was damaged would send the backup read to the wrong
offset. The backup is read first at the main region's sector size. If it
is refused there, or that size is out of range, each size from 512 to
4096 bytes is tried. A candidate counts only if its own
BytesPerSectorShift is the size it was tried at: a valid 512-byte region
found at the 4096-byte offset is a region, not a backup that starts
there. Two valid candidates are refused as ambiguous, because choosing
one would be a guess. A read failure during the search stops it and is
reported as a read failure.

**Volume length.** A volume that declares more bytes than its partition
holds is refused. The comparison is made in bytes, in `u128`, because
VolumeLength is a 64-bit count of sectors of up to 4096 bytes (§3.1.5).

**Type `0x07`.** NTFS and exFAT share MBR type `0x07`. A real NTFS volume
identifies itself from sector 0 and never reaches this rule. A partition
of that type whose sector 0 identifies nothing may be an exFAT volume
whose first sector is damaged, and its backup region is tried. If one
validates, the partition is reported as "filesystem exFAT, identified
from its backup boot region", with sector 0's problem on its own line
(`Event::ExFatIdentifiedFromBackup`), and analysed. TexFAT and an
ambiguous backup get the same heading, so the report never says
"unknown" and then names a filesystem. Otherwise the partition is
reported unidentified exactly as before. That includes a read failure
during the attempt, which is not itself reported: a known limitation,
because the partition is then counted as unidentified rather than
unread. Other partition types are not tried: a damaged first sector on
a partition declared as something else is not a reason to read it as
exFAT.

**What is counted.** TexFAT (§3.1.16, two FATs) is a valid volume this
tool does not analyse, so it joins the other filesystems identified and
not analysed (`unsupported`). An ambiguous backup is a boot region that
could not be used, so it is `boot_rejected`. Neither is a new gap kind.

---

## 4. What refuses a set, a directory and a volume (Decision D)

§8.2 and §6 grade unrecognised and invalid entries by what they make
invalid. M14 applies each at its own level:

| Found | Specification | M14 |
| --- | --- | --- |
| Unrecognised critical primary in the root | §8.2 item 1: the volume is invalid | Root not read; volume not analysed; `root_unread` |
| Critical primary other than File outside the root | §8.2 item 1: that directory is invalid | Directory not read; `directories_unread` |
| Unrecognised critical secondary in a set | §6.4.1.2.1, §8.2 item 3: the set is unrecognised | Set not used; `exfat_sets_refused` |
| Unrecognised benign primary, TexFAT Padding included | §6.3.1.2.2, §7.10: the set is unrecognised | Set not used; `exfat_sets_refused` |
| Benign secondary after the names | §6.4.1.2.2: may be ignored | Ignored, not counted |
| Entry type `80h` | §6.2.1: invalid | Entry not used; `exfat_sets_refused` |

The four critical primaries revision 1.00 defines (Allocation Bitmap,
Up-case Table, Volume Label, File) are all recognised, as §6.3.1.2.1
requires. The Volume GUID is the one benign primary recognised.

A File set is used only if its SetChecksum (§6.3.3) is valid, which
`ADR-0019` Decision C requires; beyond the counts and entry types needed
to gather the set, no field is interpreted until it is. It is also
refused if it is malformed: fewer than two secondaries, a first
secondary that is not a Stream Extension, a NameLength outside 1 to
255, or a count of File Name entries other than `ceil(NameLength / 15)`
(§7.6.3, §7.7). §7.7 makes that count the only valid one, and holding to
it exactly is what stops a name from being read past the entries that
hold it: the class of CVE-2023-4273, a stack overflow in the Linux exFAT
driver when name entries carried more than 255 characters. Tests cover
255 units, 18 name entries and a set of 255 secondaries.

A set's allocation is refused where §6.2, §6.3.4 and §7.6 rule it out.
That covers a ValidDataLength above DataLength, a directory whose two
lengths differ or whose DataLength exceeds 256 MB (§7.6.7), a DataLength
larger than the cluster heap, a first cluster outside the heap, and a
contiguous run past its end. Each refusal carries one of 25 reasons
(`SetRefusal`).

Each entry whose InUse bit is clear is skipped. Only an unused File
entry (`05h`) is counted, because only it begins a deleted set
(Decision F).

---

## 5. The directory walk and its bounds (Decisions E, L)

A FAT can describe gigabytes and a directory is allowed 256 MB (§9.5), so
neither is loaded. FAT entries are read through a one-sector cache, a
directory is streamed in chunks of 4 KiB, and one entry set, at most 256
entries, is held at a time.

Every cluster read as directory data is remembered for the whole volume,
and none is read twice. A cycle, or two entries naming the same
directory, therefore ends the second reading instead of looping. That
directory stops with a reason, and so does one whose chain links outside
the heap, reaches a cluster marked bad (§4.1.3), or runs past 256 MB.
The depth bound is FAT32's, 128 levels (`ADR-0016` Decision E).

**A subdirectory's allocation is its DataLength.** §7.6.7 says a
directory's DataLength is "the entire size of the associated
allocation", and §4.1.3 makes `FFFFFFFFh` the only valid last entry of
a chain. Until `780d6f0`, a subdirectory was read up to its DataLength
and the FAT entry of its last cluster was never looked at, so a chain
that went on had its further clusters neither read nor counted. A
subdirectory with a first cluster and a DataLength of 0 was accepted
with nothing read. Both stopped silently, which is what `SAFETY.md` §12
forbids. Now, once a chained subdirectory's DataLength is covered:

* end-of-chain ends it, as before;
* a next cluster stops it with `ChainLongerThanData`;
* a link outside the heap, or to a free cluster, stops it with
  `LinkOutOfRange`;
* a first cluster with a DataLength of 0 stops it with
  `ClusterWithoutData`, before anything is read.

Each is counted as a directory not read, and entries listed before the
fault stand, as with every other fault part way through a directory. A
contiguous subdirectory has no FAT entries to consult (§6.3.4.2) and is
unchanged. So is the root, which has no DataLength and ends where its
chain ends (§3.1.10).

Two bounds are this project's choice and not the specification's. Each is
labelled so in the code, and each is a parameter (`Limits`) that a test
can make small:

* `MAX_DIRECTORY_CLUSTERS = 2^20`, clusters read as directories on one
  volume. 4 GiB of directories at 4 KiB clusters. Reaching it counts the
  directory being read and every one queued as not read.
* `MAX_QUEUED_DIRECTORIES = 2^17`, directories named and not yet read. A
  directory that would exceed it is counted as not read and never held.

**Worst-case memory.** Computed from the types' sizes, which were measured
with `std::mem::size_of` under rustc 1.98.0, and not measured as a running
process:

| Held | Count | Bytes each | Total |
| --- | --- | --- | --- |
| Path node of a directory read, alive while a descendant waits | 2^20 | 48 + 510 name | 585.1 MB |
| Queued directory: entry, its node and name | 2^17 | 32 + 48 + 510 | 77.3 MB |
| Set of clusters read, at 2^20 entries (2^21 buckets of 5 bytes) | 1 | | 10.5 MB |
| **Sum** | | | **672.9 MB** |

The first row is bounded by the cluster cap, not the queue: a directory
that names others has at least one cluster, so at most 2^20 can be
ancestors. Allocator overhead is not included; at 16 bytes for each of a
node's two allocations it adds about 38 MB. The figure is therefore
about 0.67 GB, or about 0.71 GB with that overhead. It assumes every
name is 255 units, every directory read has a waiting descendant, and
the queue is full at once. While the set grows past 2^20 entries, its
old table is held too, about 5 MB more.

Reading is bounded the same way: every cluster is read as a directory at
most once, so directory reading cannot exceed the cluster heap.

---

## 6. Gap kinds (Decision F)

`ADR-0014` Appendix B.5 makes coverage incomplete wherever any gap is
counted. Three kinds are added, each one per occurrence:

* **Entry sets not used** (`exfat_sets_refused`): a set or entry refused
  under section 4 above. Its content may be real and was not analysed.
* **Deleted sets not examined** (`exfat_deleted_sets_not_examined`): an
  unused File entry. M14 does not read deleted sets, so a volume that
  has them cannot be reported completely analysed. M15 replaces this
  count with an examination.
* **Entries after an end marker** (`exfat_end_marker_residue`): a
  directory with entries of a type other than `00h` after its end
  marker, which §6.2.1 says cannot occur. They are counted, one per
  directory, and not interpreted. The rest of the directory is read to
  find them, within the same bounds, because a directory reported as
  read while its tail was skipped would overstate what was analysed.

---

## 7. Names and the listing (Decisions G, H)

A name is UTF-16 in the evidence (§7.7.3) and stays a sequence of code
units in the library. A sink decides how to show it. The CLI uses one
function, `escaped_name`:

* a code unit from `0x20` to `0x7E` is printed as itself, except `\` and
  `/`;
* every other code unit, and those two, is printed as `\u{XXXX}`, four
  uppercase hexadecimal digits, one escape per code unit, so a surrogate
  pair is two escapes and a lone surrogate is shown, not replaced.

Escaping the backslash makes the rendering reversible. Escaping the slash
keeps the directory path in a heading unambiguous: both characters are
illegal in a name (§7.7.3, Table 35), so a name that holds one is
evidence worth showing exactly. No control character reaches a terminal,
which is the weakness CWE-150 names (improper neutralisation of escape,
meta or control sequences). The other characters Table 35 forbids and
that are printable (`" * : < > ? |`) are printed as themselves. A name
never becomes a path on disk, so they cannot reach one.

An exFAT entry line's position, `c<cluster> s<slot>`, is padded to 20
characters, the width of `c4294967295 s1048575`: the largest cluster
number a `u32` holds, and the last slot of a 32 MiB cluster (§9.2), which
holds 1,048,576 entries. No position is wider, so every entry keeps one
line and its columns. Wrapping long positions onto a second line was
rejected: it breaks one line per entry, and on a real volume it would
have nearly doubled the listing.

---

## 8. Content (Decisions I, J)

`ExFatReader::read_content` refuses a directory, and re-applies the
allocation checks of section 4, because an `ExFatFile`'s fields are
public and may not have come from a listing.

**The chain is walked before data is read.** With NoFatChain 0, links 1 to
n-1 must each name a next cluster in the heap and link n must be
`FFFFFFFFh`, where n is the clusters DataLength needs. This proves no
cluster repeats without remembering any: each cluster has one successor,
so a repeat would loop and never reach the end mark. A chain that ends
early is refused. So is one that goes on past n, and that refusal is this
project's choice. §6.3.4.2 sizes a contiguous allocation from DataLength.
For a chain, §6.2.3 says DataLength is the size of the data the
allocation contains, and no clause says whether the chain may be
longer. Implementations differ (section 10). A file whose chain says
more than its length is evidence that disagrees with itself, and
reading it as if it did not would hide that.

**ValidDataLength.** Bytes up to ValidDataLength are read, each cluster at
most once, in chunks of at most 64 KiB, so memory is one chunk whatever
the file's size. The bytes from ValidDataLength to DataLength are handed
over as zeros and never read: §7.6.5 says that data is undefined and that
reads return zeros, and `ADR-0019` Decision F adopts it.

**Errors.** Every error except an evidence read failure is found before
the first byte is handed over. A read failure part way through leaves
the bytes already handed over, and the caller decides what they mean.
M15, which writes content through `ArtifactWriter`, must call `abort`
on such an error, as `ADR-0020` Decision C requires.

---

## 9. The acceptance tables (Decision K)

`tests/exfat_content.rs` reads every live file on EXP-0009's images A and
B through the public API and compares the set both ways, nothing missing
and nothing extra, by size and SHA-256, against
`fixtures/exfat/exp9-A.expected.tsv` (121 files) and
`exp9-B.expected.tsv` (119 files).

The tables were produced by The Sleuth Kit 4.12.1 (`fls -r -p -u -F -o
128`, then `icat -o 128` per file), with its five virtual entries
removed. 120 of A's rows and 117 of B's also equal the output of
`scripts/experiments/exp-0009-generate.py`. The rest are Windows' own
files under `System Volume Information`, for which The Sleuth Kit is the
only reference. Where the generator wrote a file, the generator is the
truth and The Sleuth Kit a second reading. Each table's header says
where it came from and that Taphonomy did not produce it. A table
produced by the code under test would test nothing.

Two of the checks above, reading past ValidDataLength and skipping the
chain walk, are caught only by unit tests: on both images every live
file's ValidDataLength equals its DataLength and every chain is well
formed.

---

## 10. Prior art: a chain longer than its data

Read at pinned revisions; no code was copied.

| Implementation | Revision | A chain longer than DataLength needs |
| --- | --- | --- |
| exfatprogs `fsck.exfat` | `8b42748` | An error, `ER_FILE_SMALLER_SIZE`, "more clusters are allocated", for files and directories, repaired by truncating the chain |
| Linux `fs/exfat` | `f47d223` | Not looked at: `exfat_map_cluster` maps only the clusters the size needs |
| The Sleuth Kit | `sleuthkit-4.12.1` | Not looked at: `fatfs_make_data_runs`, which exFAT uses, stops at the size |
| Taphonomy M14, content | `97c9242` | Refused, `ExFatContentError::ChainLongerThanData` |
| Taphonomy M14, directories | `780d6f0` | Directory not read, `ExFatDirectoryError::ChainLongerThanData` |

A checker treats the condition as damage; readers that only need the data
never see it. Taphonomy reports on evidence, so it takes the checker's
view, for content and for directories. exfatprogs also treats a first
cluster with a DataLength of 0 as an error (`ER_FILE_FIRST_CLUS`).
Since `780d6f0` Taphonomy does too for a directory; it still accepts it
for a file.

---

## 11. How it was proved

Only Stage 3 could change what the CLI prints. Stage 2 added a library
module the CLI did not call, and nothing outside its own module calls
Stage 4's `read_content`. Stage 3 was proved by the method of `ADR-0018`
§5 against the binary before it, over the 31 `.img` fixtures and the
three CFReDS images of EXP-0008. Output on all 34 was byte-identical,
and stayed so through Stage 3's follow-up changes. Seventeen deliberate
faults each made a test fail. These counts come from the working record
of the milestone; the commit messages do not carry them.

Image A's listing was compared with The Sleuth Kit's. On 2026-10-20 a
copy of image A with its first exFAT sector zeroed was run through the
binary at `8ea143c`. It was identified from its backup boot region,
found by trying each sector size, and its listing from the root
directory on was byte-identical to image A's. The live file counts on
both images, 121 and 119, equal The Sleuth Kit's.

`780d6f0` was proved the same way against the binary at `8ea143c`, over
37 images: the 31 `.img` fixtures, EXP-0009's images A and B, the copy
of A with its first exFAT sector zeroed (SHA-256 `4eb24363…bf856a9`),
and the three CFReDS images, whose digests were checked against
EXP-0008 first. Each was run three ways, 111 runs per binary, recording
exit status and the digests of standard output, standard error and
every file written. Two independently written scripts each found the
base and new tables byte-identical, and every image's digest unchanged
by the runs. A read-only scan of A and B, written separately from the
code, had already found neither shape on either image: each has one
live subdirectory. With the chain check removed, four of the six new
tests failed; with the zero-length check removed, the fifth did; the
sixth is the contiguous control (working record). All 498 tests pass.

---

## 12. `ADR-0019` §6 read against M14

`ADR-0019` §6 lists four conditions "before the exFAT code is merged".
M14 merged read-only exFAT code with items 3 and 4 met. How §6 is read
against that is recorded where the condition is, in `ADR-0019` Appendix
B: items 1 and 2 gate the recovery of deleted files, and M15 does not
merge until they hold.

---

## 13. Consequences

* The CLI analyses exFAT volumes: boot region, every directory, every live
  file. A run on an exFAT volume with deleted sets reports `incomplete`
  until M15.
* `--recover` writes nothing for an exFAT volume. Its deleted sets are
  counted as not examined.
* The modules `exfat_boot`, `exfat_directory` and `analysis::exfat` are
  public, because the CLI, which is a separate crate in `src/main.rs`,
  and the integration tests use them. That is not a decision about
  publication (section 14).
* `read_content` hands zeros past ValidDataLength over as ordinary
  chunks, so `ArtifactWriter` takes them through `push` as it takes any
  bytes. This answers `ADR-0020` §5's first item for live files; M15
  confirms it when it writes one.

---

## 14. Not decided here

* **Which components are published.** `ADR-0018` §6 deferred it to exFAT
  because of exFAT's patent status, and no patent opinion has been
  obtained. The modules are `pub` within the GPL engine, and no stability
  is promised for them.
* **NameHash** (§7.6.4) is not verified. It needs the up-case table, and
  it proves only a mismatch.
* **Allocations of benign secondaries**, such as Vendor Allocation
  entries (§7.9), are neither read nor counted.
* **A non-directory with a first cluster and a DataLength of 0** is
  accepted (section 10).
* **Deleted files, the allocation bitmap's contents, the name of an exFAT
  artifact** (`ADR-0020` §4) and **the label a recovery carries**
  (`ADR-0014` §11) belong to M15 and `ADR-0022`.
