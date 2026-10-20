# Taphonomy Known Issues

This document tracks known limitations, defects, and unresolved failure
modes in Taphonomy that have not yet been fixed, so that users and
contributors can distinguish an untested gap from a confirmed, documented
issue.

---

## No fixture exercises FAT mirroring

Every fixture writes `BPB_ExtFlags` as `0x0000`, meaning mirroring is
enabled and all FATs are maintained. Measured 2026-08-30:

```text
xxd -s $((1048576 + 0x28)) -l 2 -p fixtures/partition/mbr-single-fat32.img
  0000
```

`mkfs.vfat` offers no option to set the field. A fixture exercising the
mirroring-disabled path must poke `ExtFlags` and additionally make the
FATs differ, because `mkfs.vfat` writes them identically and a parser
reading FAT 0 would otherwise produce the same result as one reading
FAT 1.

`parse_boot_sector` validates the active FAT index against the declared
FAT count, and that check has unit coverage. Code that acts on the index
now exists: `enumerate_root` reads the FAT named by `BPB_ExtFlags` when
mirroring is disabled, and FAT 0 otherwise. It is covered by
`the_active_fat_is_read_when_mirroring_is_disabled`, which builds a volume
in memory with `ExtFlags` set to `0x0081`, a stale chain in FAT 0 and the
maintained chain in FAT 1.

What remains untested is the path against evidence a formatting tool
produced. No fixture reaches it, and none can until the fixture described
above exists.

## Long filename patent status unverified

Microsoft held patents on the VFAT long-filename mechanism and litigated
them. Their current status has not been researched. ADR-0007 section 5.5
records this as required before long-name decoding is implemented. It
does not affect enumeration, which counts long-name entries, nor the
recovery of a deleted short name's first character, which reads only a
long-name entry's checksum (`ADR-0009` section 6); neither decodes a name.

## No fixture can exercise a zeroed first-cluster high word

Deleting a file leaves the directory entry's `DIR_FstClusHI` intact under
`mtools`. Measured 2026-09-04, EXP-0003:

```text
HIGHFILE.TXT before deletion   hi=1 lo=2055  cluster 67591
HIGHFILE.TXT after mdel        hi=1 lo=2055  cluster 67591
```

Microsoft implementations are widely reported to zero that word, so a
deleted file beginning above cluster 65,535 loses its start location
entirely on evidence from a Windows host. The FAT32 specification does
not require the behaviour either way.

No fixture can produce the case, because the tool that builds the
fixtures does not produce it. Reaching it requires poking an image under
ADR-0006 section 5.1. Until that fixture exists, any code that handles a
zeroed high word is untested against evidence a formatting tool wrote.

`mdel`, `mrd` and `mdeltree` determinism is measured and is no longer an
open issue; EXP-0003 records it.

---

## FAT-level helpers live in the directory module

`cluster_offset` computes a data cluster's byte offset and `read_fat_entry`
reads one entry from the file allocation table. Neither is about
directories. Both are declared in `src/fat_directory.rs`, where M5 put them
because it was the only caller.

M7 added `src/fat_recovery.rs` as a second caller, so both widened to
`pub(crate)`. ADR-0010 section 9 decided against moving them: the move
would edit a path M5's and M6's tests cover, for an organisational gain
rather than a measured one.

The cost shows in the error type. `RecoveryError::Directory` carries a
`DirectoryError`, and the variants reaching the recovery module through
that path are `OffsetOverflow`, `ClusterOutOfRange` and `BadCluster`. None
of those is a directory failure, so a reader of a recovery error is told
the wrong thing about where it happened. ADR-0010 Appendix B6 records the
same trade.

The fix is to move both into `src/fat.rs`, which is BIOS parameter block
parsing and variant determination and today holds no FAT-table access at
all. The trigger is a third caller, or any change that touches
`fat_directory.rs`'s FAT handling for another reason.

---

## Most fragmentation arrangements are unmeasured

Deletion zeroes the cluster chain in every FAT, measured in EXP-0003, so
nothing in the evidence says a deleted file occupied its clusters
contiguously. The recovery path computes the run the entry implies,
refuses it where any cluster is in use, and extracts where every cluster
is free.

The failure that follows is this tool's characteristic false positive: a
deleted file that was fragmented, whose clusters have since been freed,
produces a run that passes the allocation check and a digest that is
plausible and wrong.

Since M10 it also produces a **file**. With `--output` the artifact is
written whole, at the size the entry declared, and the run reports that
what landed matches what was read — which it does. Nothing about the file
on disk distinguishes it from a correct recovery: not its size, not its
name, not the verification line beside it. The operator is handed a
plausible wrong file rather than a plausible wrong digest, which is the
worse of the two, and only a reference digest they already hold can tell
them which they have.

Two arrangements are measured. Both are built with `mkfs.vfat`, `mcopy` and
`mdel` alone, with no poked field, by the technique EXP-0004 records, and
both share the generator's `build_fragmented_volume`.

`fat32-fragmented-deleted.img` holds a deleted file whose three clusters
were not adjacent, with the clusters between its fragments freed as well.
On it, three of the five deleted entries recover content that is not their
own file's, and the tool reports all five identically: every cluster free,
no refusal, a digest for each.

`fat32-fragmented-live-gap.img` is the same construction with the clusters
between the fragments still allocated to live files. The run the fragmented
entry implies reaches cluster 5, which the live `S2.BIN` holds, so the
entry is refused before any content is read. This is the arrangement the
tool handles correctly, and the refusal is reached with no byte written by
this project, where `fat32-recover-collision.img` needs a poked size to
produce one.

The run reports both images as covered completely, which is correct and is
a statement about reach alone: everything the tool could reach was
analysed. Three of the first image's five recoveries are still wrong.
Coverage is not an accuracy claim and must not be read as one.

Three arrangements this document previously listed as unmeasured are in
fact measured. The record is corrected here rather than carried forward:

* **More than two fragments.** `FRAG.BIN` occupies clusters 4, 6 and 8 in
  both images, which is three fragments and not two. EXP-0004 records the
  `mshowfat` output, and the generator asserts that arrangement on every
  run rather than assuming it.
* **An active file lying between the fragments.** That is
  `fat32-fragmented-live-gap.img`. EXP-0004 Appendix A.5 measured the
  construction and its determinism before it became a fixture.
* **Overwritten files.** `S3.BIN` and `S5.BIN` are deleted entries whose
  single cluster was reallocated to `FRAG.BIN` and never freed again, so
  both recover `FRAG.BIN`'s content rather than their own. EXP-0004
  Appendix A.2 records the digests and notes that this falsifies ADR-0010
  section 10.2.

What remains unmeasured is large files, a file wrapping from the end of the
volume to its beginning, and fragments in non-sequential order.

The last of those is blocked rather than merely unbuilt. `mtools` starts
each free-cluster search from the FAT32 FSINFO next-free hint and, once
that search has wrapped, takes the free clusters it finds in ascending
order, so a file whose logical fragment order runs backwards has no
allocation path. Both routes to one are closed: ADR-0006 section 5.2
rejects mounting, and a poked FAT chain or `mdoctorfat` falls under
section 5.1's objection to hand-built structure and outside the CFTT
specification's own testing scope, which excludes file system metadata that
has been corrupted, modified or otherwise manipulated.

NIST's CFTT deleted file recovery suite defines seventeen test cases, and
the tree now holds the equivalent of two of them, so no score across the
suite can be computed. Until more are built, item 5 of the README's
development sequence stays open.

Validation against a reference does not close the remainder. It detects a
wrong digest only where the operator already holds the right one, which is
not the case the requirement is about.

---

## Three output-path outcomes are untested end to end

With `--output`, each artifact is written, flushed, read back and compared
with the digest taken from the evidence. `ADR-0015` Appendices B and C
record what remains open.

**A failed flush and an unreadable written file.** A failed flush is
reported as not written and the file is removed. A written file that cannot
be read back is reported as `NOT VERIFIED` and left in place. Neither can be
reached without a failing device, so both are reasoned from the code rather
than measured.

**A partial file that survives an evidence failure.** When the evidence
fails part way through an artifact and the partial file cannot be removed,
the run reports both, as `ADR-0015` section 9 requires. The reporting is
tested, but not the path through the whole extraction: no single directory
can allow a file to be created and then refuse its removal without root.

**The read-back does not reach the storage medium.** It establishes what the
destination filesystem returns for the file after a successful flush, which
on Linux is normally served from the page cache. "matches what was read"
means that, and not that the medium holds the bytes. Reading past the cache
would need `unsafe` code or a new dependency, and neither is adopted.

---

## A directory the walk declines to read hides everything inside it

`ADR-0016` Decision E stops the walk at a cluster already read and 128
levels below the root, and Decision C refuses a deleted directory whose
first cluster does not identify itself. Each is reported as one gap, and the
count is of directories the walk reached, not of what they hold.

`fat32-nested-directories.img` measures it: the 129th level is refused and
counted, and the 130th is named only inside the cluster that was refused, so
it is never reached and never counted. A single gap can therefore stand for
a subtree of any size, and the report says which directory it was.

The orphan search (`ADR-0017`) reaches part of what such a gap hides, and
only part. It never reads a directory the walk declined, but it finds any
directory below one whose own first cluster still names itself and is
free. `fat32-deleted-nested.img` measures both: the 129th level stays
unread, and the 130th, named only inside it, is found by the search.

---

## Four test suites are dominated by whole-image reads

`tests/cli_arguments.rs` runs the binary rather than calling into the
crate, because argument handling and exit status are decisions `main`
makes about `argv` and are not reachable from the library. Six of its
eleven tests open a fixture, and the binary hashes the whole 64 MB image
before it reports anything.

`tests/coverage_reporting.rs` runs the binary for the same reason: coverage
and the exit status derived from it are decided once per run. Its eleven
tests spawn it twelve times, each on a fixture.

`tests/recovery_output.rs` runs the binary because the destination checks
are decisions `main` makes before the library is called. Each of its nine
tests spawns it once, naming a 64 MB fixture.

`tests/orphaned_directories.rs` no longer belongs in this list. Five of
its six tests call `taphonomy::analysis::run` directly and compute no
digest. The sixth runs the binary once, for three lines of output no
other test asserts.

On 2026-09-26 its median fell from 8.39 to 4.31 seconds over three runs
before the rewrite and three after. The runs were not interleaved and
the timings were unstable: `tests/coverage_reporting.rs`, which the
rewrite did not touch, ranged from 8.79 to 23.18 seconds in the same
session. The figures show the suite roughly halved, and no more.

`tests/fat32_recovery_fixtures.rs` is comparable, and for a different
reason. It calls into the crate and spawns no process, so the cost is not
the binary: it is opening 64 MB fixtures. Measured 2026-09-23 at `75deb58`:

```text
tests/coverage_reporting.rs      11 tests  30.20s
tests/recovery_output.rs          9 tests  19.89s
tests/cli_arguments.rs           11 tests  16.66s
tests/fat32_recovery_fixtures.rs 24 tests   8.40s
```

Every other harness in the workspace finishes in under a fifth of a second,
and the nine of them together took 0.33 seconds in that run. Wall times vary
between runs: the same day at `739d308`, `coverage_reporting` took 45.83
seconds. Reading subdirectories lengthened them again: a run over the
subtree fixture now recovers three files rather than reporting one gap.

The orphan search added to every run of the binary. `ADR-0017` section 10
condition 7 measured it on 2026-09-24, `c3278fe` against `4bd85d8`, three
rounds interleaved, medians:

```text
tests/coverage_reporting.rs      30.51s  33.45s  +2.94s
tests/recovery_output.rs         18.14s  19.42s  +1.28s
tests/cli_arguments.rs           14.50s  15.65s  +1.15s
tests/fat32_recovery_fixtures.rs  8.73s   8.56s  none
```

All nine pairs of the three suites that run the binary moved the same way;
the fourth calls the crate and never runs the search. Timing the debug
binary alone, five runs each on four fixtures, put the search at about 0.4
seconds a run: one 64-byte read for nearly every one of the 127,006
clusters a fixture volume holds. `tests/orphaned_directories.rs` took 13.76 seconds at
`3ff77f7`.

Measured 2026-09-25 at `8b08d41`, with one change: `sha2` alone built at
`opt-level = 3` in the dev profile (`Cargo.toml`). Three rounds, each
running `cargo test --workspace --no-fail-fast` without the change and
then with it; medians:

```text
                                  before    after
tests/cli_arguments.rs            20.46s     2.43s
tests/coverage_reporting.rs       41.95s     6.40s
tests/orphaned_directories.rs     17.16s     2.72s
tests/recovery_output.rs          26.14s     4.92s
tests/fat32_recovery_fixtures.rs   9.86s     0.97s
whole command                    116.67s    17.62s
```

Every suite fell by roughly five to ten times. Only `sha2` runs
optimised, so the fall is attributed to hashing. It is the same
whole-image read, done by a faster hasher, not a smaller read.

A fixture small enough for tests that only need an argument decision
would help `tests/cli_arguments.rs`, `tests/coverage_reporting.rs` and
`tests/recovery_output.rs`, not `tests/fat32_recovery_fixtures.rs` and
not `tests/orphaned_directories.rs`'s one remaining binary test, which
asserts renderings rather than an argument decision. Reaching those
decisions without a whole-image read would likewise help only the
three. Neither is needed yet, but the entry should not be read as
naming a single suite.

---

## The orphan search's cost on real media is unmeasured

The search reads the first 64 bytes of every cluster the walk did not
reach, one read at a time. A fixture volume holds 127,006 clusters; a 32 GB
card formatted with 32 KB clusters holds about a million. EXP-0008 timed a
release build on 1 GiB CFReDS images at about ten seconds each, which
includes hashing every byte, and did not separate the search's share.
Reading many cluster starts per call would cut the reads by the same
factor. `ADR-0017` promises nothing about speed, so no change is made
without a measurement that needs it.

---

## FAT12 and FAT16 are identified and not analysed

Every CFReDS FAT image carries a FAT12 or FAT16 partition and a FAT16
partition beside its FAT32 one. EXP-0008 found ten of the fifteen
deleted files on three of those images on partitions the tool identifies
correctly and does not read. The report states each as a filesystem not
analysed, so coverage is honest; reach is not.

---

## A file fragmented around a live file is refused where another tool recovers it

On `dfr-02-fat.dd` the run a deleted file's entry implies crosses a live
file, and the tool refuses it (`ADR-0010` Decision C). The Sleuth Kit
passes over the allocated clusters and recovers the file, and EXP-0008
measured its recovery equal to the sectors NIST documents. Its rule is an
inference too, which that layout rewards; on the interleaved layouts of
DFR-05 the clusters it would pass over are free. Whether to adopt it,
labelled as the inference it is, is a decision for its own ADR, measured on
those images first.

---

## No fuzz testing exists

`SECURITY.md` sections 35 and 36 call for fuzzing the parsers. None exists:
there is no fuzz target and no fuzzing dependency. The MBR, boot sector and
directory entry parsers read untrusted bytes, are bounds-checked, and have
tests for the malformed inputs the fixtures and unit tests construct.
Inputs nobody thought to construct are untested, and that is the gap
fuzzing closes.

---

## Two `ParseError` variants are unreachable from the only caller

`ParseError::LengthOverflow` and `ParseError::ShortSector` are both
unreachable from `parse_mbr`'s only call site, `src/analysis.rs::run`.

`ShortSector` needs a short read from `read_exact_at`, which either
succeeds whole or fails outright and is counted as `table_unread` before
`parse_mbr` is called.

`LengthOverflow` needs `start_lba` plus `sector_count` to overflow `u64`.
Both are `u32`, so their sum as `u64` cannot reach `u64::MAX`.

`table_rejected`'s only live cause at HEAD is a missing or wrong boot
signature.

Found while drafting ADR-0014 Appendix D (2026-10-01). Not fixed, and not
blocking.

---

## The two removal paths disagree about a file that is already gone

In `src/extraction.rs`, `remove_partial`, used when the evidence fails
part way through, treats a file that is already gone as removed. The
`Broken` and failed-flush arms of `settle` use
`fs::remove_file(..).is_ok()`, so a file already gone is reported as
`Failed { removed: false }`. Reached only if the file disappears between
its creation and its removal; no test or fixture reaches it.

Found while moving the code in M13 (`ADR-0020` Decision E) and preserved
exactly. Not fixed.

---

## exFAT directory walk: worst-case memory is computed, not measured

At both of the walk's safety bounds, `MAX_DIRECTORY_CLUSTERS` and
`MAX_QUEUED_DIRECTORIES`, the memory held is about 0.67 GB, or about
0.71 GB with allocator overhead. The figure is computed from the types'
sizes and has not been measured on a running process.

Recorded in `ADR-0021` section 5.

---

## A read failure while probing a `0x07` partition is not reported

A partition of type `0x07` whose first sector identifies nothing is
probed for an exFAT backup boot region. If a read fails during that
probe, the failure is not reported, and the partition is counted as
unidentified rather than unread.

Recorded in `ADR-0021` section 3 and `ADR-0014` Appendix E.

---

## A read failure during the exFAT backup search stops the search

When the main boot region of an exFAT partition is refused, the backup
is searched for by trying each sector size. A read failure ends the
search at once: the sizes not yet tried are not tried, and the
partition is reported as unread (`search_backup` in
`src/exfat_boot.rs`).

Recorded in `ADR-0021` section 3.

---

## If the exFAT root fails part way, its subdirectories are not counted

The root is read first, and the subdirectories it names are queued. If
the root then fails, the walk of that volume stops. The subdirectories
already queued are neither read nor counted as directories not read.
The root's failure is the gap counted (`root_unread`), and the volume is
not counted as analysed (`report_volume` in `src/analysis/exfat.rs`).

Found while documenting M14 (2026-10-20). Not fixed.

---

## An exFAT file with a first cluster and a DataLength of 0 is accepted

exfatprogs' checker treats such an entry as an error
(`ER_FILE_FIRST_CLUS`). Taphonomy has refused it for a directory since
`780d6f0`, and still accepts it for a file.

Recorded in `ADR-0021` sections 5 and 10.

---

## A contiguous exFAT file is read one cluster at a time

`ExFatReader::read_content` reads a file whose clusters are one
contiguous run cluster by cluster, as it reads a chain. Its speed has
not been measured, so nothing is changed.

Seen in `read_content` in `src/exfat_directory.rs`. Not fixed.

---

## `ExFatReader::read_content` leaves partial output on an evidence error

Every other error is found before the first byte is handed over. A read
failure part way through leaves the bytes already handed over, and
they stand. M15, which writes content through `ArtifactWriter`, must
call `abort` on such an error.

Recorded in `ADR-0021` section 8 and `ADR-0020` Decision C.

---

## exFAT NameHash is not verified

A File set's NameHash (specification section 7.6.4) is not checked.
Checking it needs the up-case table, and it proves only a mismatch.

Recorded in `ADR-0021` section 14.

---

## Allocations of exFAT benign secondaries are neither read nor counted

A benign secondary entry after a set's names is ignored. Where it
describes an allocation, as a Vendor Allocation entry does
(specification section 7.9), those clusters are neither read nor
counted as a gap.

Recorded in `ADR-0021` section 14.

---

## Most exFAT gap kinds are reached only by unit tests

EXP-0009's images A and B reach one exFAT gap kind, "deleted sets not
examined", and no other. The other two new kinds, and the exFAT cases
of the older kinds, are reached only by unit tests on volumes built in
memory. No fixture holds them.

Recorded in `ADR-0014` Appendix E.
