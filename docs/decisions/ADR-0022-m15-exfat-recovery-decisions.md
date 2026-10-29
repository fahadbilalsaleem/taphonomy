# ADR-0022: M15 exFAT Deleted-File Recovery, and the Label It Carries

* **Status:** Accepted
* **Date:** 2026-10-21
* **Decision owners:** Taphonomy project
* **Scope:** How M15 finds deleted exFAT entry sets, decides each by
  `ADR-0019` section 4 as amended by its Appendix C, writes what it
  recovers, counts what it cannot examine, and labels each artifact
* **Related:** `ADR-0003` sections 3.2, 4.1 to 4.7, Appendix C;
  `ADR-0010` Decision B; `ADR-0013` Decision D; `ADR-0014` Decisions A,
  B, E, section 11, Appendices B.5, E, F; `ADR-0015`; `ADR-0016`
  Decision F, Appendix B; `ADR-0018` section 5; `ADR-0019` sections 4
  to 6, Appendices A to C; `ADR-0020`; `ADR-0021`; EXP-0009 and its
  Appendix A; EXP-0011; Microsoft, *exFAT file system specification*,
  Markdown source, SHA-256
  `09454c9663cbd7b33c6dfe3ad37baa1351779ad5ab73034c53fcf864ccdee825`

---

## 1. Context

`ADR-0019` decided when a deleted exFAT file may be recovered and what
a recovery may claim. M14 (`ADR-0021`) reads a volume, its directories
and its live files, and counts every deleted entry set as not examined.
M15 examines them.

Before this record, the rules were tested against three sets of
volumes whose ground truth no tool under test produced. They are
recorded in EXP-0011:

* EXP-0009's images, deleted by Windows 10, against the generator;
* NIST's twenty exFAT deleted-file-recovery images, made on macOS,
  against the tags NIST wrote into every block, against the format of
  the files macOS wrote itself, which NIST did not tag, and, for 54
  recoveries, NIST's sector lists;
* the exFAT image of `forensics-samples`, deleted by Linux, against the
  published original files.

Three findings changed the rules before any code. On NIST's dfr-13,
`ADR-0019` section 4 as written recovers one file whose cluster holds
another file's later append; `ADR-0019` Appendix C corrects it. A rule
proposed in the audit, that any unseen claim refuses every set on the
volume, recovers nothing on any of the twenty, because every one holds
a deleted directory; it was withdrawn, and Decision D replaced it. And
with deleted directories left unread, five of macOS's own files are
recovered over data written by a file inside a deleted directory;
reading deleted directories for their claims (Decision K) prevents
them. Two wrong recoveries remain that no rule found prevents (section
15).

Section numbers are the specification's unless a document is named.

---

## 2. Decisions

* **A.** An exFAT artifact is classified `UNVERIFIED`. The level states
  the method, is the same for every exFAT artifact, and a digest match
  is reported beside it and does not raise it. FAT32 artifacts stay
  `RECONSTRUCTED`.
* **B.** A deleted entry set is an unused File entry (`05h`) followed
  by exactly the secondaries it declares, each unused, which with every
  InUse bit set forms a File set whose SetChecksum is valid and which
  passes every check `ADR-0021` section 4 applies to a live set.
* **C.** Recovery runs in two phases on a volume: the directory walk
  records every claim, and only then is any set decided.
* **D.** Condition 2 is judged over the claims the walk saw, deleted
  directories included. Every claim it could not see is counted as a
  gap.
* **E.** Condition 1 reads the allocation bitmap through its FAT chain
  and first checks the bitmap against the clusters the walk knows are
  in use.
* **F.** Two shapes the conditions do not settle are refused: data with
  a ValidDataLength of 0, and a first cluster with a DataLength of 0.
* **G.** Without `--recover`, content is not read and condition 3 is
  reported as not checked. With it, condition 3 is decided before any
  file is opened.
* **H.** A recovered file is written through `ArtifactWriter`, named as
  a FAT32 artifact is, and abandoned with `abort` on any error.
* **I.** A set refused under a condition is a finding, not a gap.
* **J.** The claims table is held to two bounds this project chose.
* **K.** A deleted directory is read for its claims where it can be,
  and nothing in it is recovered. Recovering from it is M16.

---

## 3. Decision A: the label

`ADR-0014` section 11 reopens the classification when "a means is found
to establish a deleted file's extent from surviving evidence rather
than assumption". exFAT is that means: EXP-0009 measured that Windows
leaves the entry set, the FAT chain and the data. EXP-0011 measured
that macOS leaves them too: the 14 fragmented deleted files on NIST's
DFR-02, 03, 05, 05-braid, 05-nest and 12 are recovered by their chains,
each correct by its tags. Linux left the entry sets and data of the 18
files it deleted; all 18 were contiguous, so its chains were not
exercised.

`ADR-0003` section 3.2 defines `RECONSTRUCTED` as content assembled
"using inference or heuristics rather than surviving filesystem
metadata". An exFAT extent is surviving filesystem metadata, so that
level would misdescribe the method, which is the error `ADR-0014`
section 3.4 refused to make with `UNVERIFIED` for FAT32.

`UNVERIFIED`'s evidence is "Extraction succeeded; no validator exists
for the format, or validation could not run". In section 3.2 the
validator is the format-aware validator `STRUCTURALLY_VALID` and
`PARTIAL` require, and none exists. `ADR-0014` section 3.4 read
"validation" as the reference comparison of M8 and so declined the
level. This record reads it as section 3.2's validator. That is a
re-reading, recorded as one, and it applies to exFAT only: FAT32 stays
`RECONSTRUCTED`, because its extent is inferred.

The level is not raised by a digest match, for the reason `ADR-0014`
Decision A gives and which holds here as strongly. A match does not
change the method. EXP-0009 Appendix A and EXP-0011 measured that
metadata passing every check can still sit over another file's data, or
over zeros, so the level states how the bytes were found and the
outcome axis states what a comparison found. `VERIFIED` stays
unreachable, and `ADR-0014` section 3.5's gap (a refuted comparison has
no level) does not arise, because the level never depends on the
comparison.

The run reports counts per level (`ADR-0003` section 4.7). The summary
line keeps its form, one line headed `artifacts`, and lists each level
with a count above zero: `artifacts    5 UNVERIFIED`, or
`artifacts    2 RECONSTRUCTED, 5 UNVERIFIED`. A run with no exFAT
artifact prints the line it prints today.

`ADR-0003` Appendix C records the change in the reachable subset.

---

## 4. Decision B: what a deleted set is

`ADR-0019` section 4 defines a deleted set as unused entries that, with
InUse set again, form a File set whose SetChecksum is valid. EXP-0009
P3 and P4 measured the three targets on image B. EXP-0011 found all
125 deleted sets on B, and every deleted set on NIST's images and the
Linux image, those inside deleted directories included, valid by that
test.

The collector follows the rule `collect_set` follows for live sets: an
entry that is not what the set needs is handed back, not consumed.
For a deleted set it needs unused secondaries (`40h`, `41h`, or an
unused benign secondary). A live entry inside a deleted set's declared
span ends the set and is read as live, because the slot was reused
and the live entry is the newer writing. The set is then not usable.

A set that fails the checksum or any check of `ADR-0021` section 4 is
not usable. Its fields are not trusted (`ADR-0019` Decision C), so it
claims nothing, and it is counted as a gap (Decision D).

---

## 5. Decisions C and D: two phases, and claims the walk saw

Condition 2 is a statement about every deleted set on the volume, so no
set can be decided while the walk is still finding others.

**Phase 1** is M14's walk, which also records for each usable deleted
set its position, the directory clusters its entries lie in, its
allocation as runs of clusters, its two keys (creation, and last
modification) and its lengths and flags. A set can straddle two
directory clusters, or, at the smallest cluster size, up to seventeen,
so the position of its File entry alone does not reach its last entry.
Names are not held: phase 2 reads each set again from the clusters
recorded and checks its checksum again, so memory does not grow with
name length.

**Phase 2** decides each recorded set (section 6) and reports it.

**Claims.** A usable deleted set claims the clusters of its allocation.
For a chain, that is every cluster the walk from its first cluster
reached, including a chain refused under condition 4, so a malformed
set still protects the clusters it names. A deleted directory's set
claims its own clusters, and every set read inside it claims its
allocation (Decision K). An unused Stream Extension (`40h`) outside any
set, whose File entry was overwritten, claims the clusters its fields
name where they are in range, with no time, so any set it overlaps
cannot be ordered against it and is refused under condition 2.

**Unseen claims are gaps.** The walk cannot see a claim held in a
directory not read (an existing gap), in a deleted directory that cannot
be read (Decision K), in a deleted set that is not usable, or past the
claims table's bound (Decision J). Each is counted, so the run reports
`incomplete`, and condition 2 is judged over the claims seen. The
project decided this on 2026-10-21, on EXP-0011's evidence, measured on
NIST's twenty images (right and wrong judged by NIST's tags and, for
macOS's own files, by their format):

| Rule | Deleted directories | Right | Wrong |
| --- | --- | ---: | ---: |
| `ADR-0019` section 4 as written | not read | 658 | 8 |
| As amended (Appendix C) | not read | 656 | 7 |
| As amended | read for claims (M15) | 656 | 2 |
| As amended, any unseen claim refuses the volume | read for claims | 0 | 0 |

The last row is what `ADR-0019` section 4's words "recovered only if
all of these hold" mean read literally. Every NIST volume holds a
deleted directory, so it recovers nothing. The only wrong recoveries it
avoids that the third row does not are the two of section 15, and it
avoids them only by recovering nothing at all. This decision relaxes the
literal reading, and records that it does: a recovery reports "no later
claim among the sets examined", never "no later claim".

The ordering itself, including how creation and modification times and
their offsets are compared, is `ADR-0019` Appendix C.

**Algorithm.** A comparison of every pair is quadratic in the number of
sets, and a hostile volume can hold millions. Phase 2 sweeps the claimed
runs once in cluster order, and keeps, for each stretch of clusters
between run boundaries, the latest key, the second latest, and whether
two keys there cannot be ordered. A set is refused under condition 2
where, anywhere in its runs, the second-latest key is at or after its
own creation key, or two keys cannot be ordered. That is the pairwise
rule, decided in O(R log R) for R runs. The tests compare it with a
direct pairwise check on small volumes.

---

## 6. The order conditions are decided in, and what each reports

Each set is decided in this order, and reports the first condition it
fails, by its number in `ADR-0019` section 4:

1. **Condition 4, allocation well formed.** `read_content`'s checks
   (`ADR-0021` section 8): in the heap, a contiguous run inside it, or
   a chain walked whole that is exactly as long as DataLength needs and
   ends in `FFFFFFFFh`. Decision F's two shapes are refused here.
2. **Condition 1, every cluster free** in the bitmap (Decision E).
3. **Condition 2, no later claim** among the sets examined (section 5,
   `ADR-0019` Appendix C).
4. **Condition 3, not zeros only**, up to ValidDataLength (Decision G).

A set with no allocation (FirstCluster 0, DataLength 0) is reported as
holding no data, which is how `ADR-0019` section 6 reports the six
`FILL233.bin` sets.

---

## 7. Decision E: the bitmap

§7.1.3 says the bitmap's FirstCluster is "the first cluster of the
cluster chain, as the FAT describes". The Sleuth Kit (`51d8672`),
exfatprogs (`8b42748`) and Linux (`f47d223`) all read it as contiguous
from that cluster. Taphonomy follows the specification. The chain is
held to the content rules (walked whole, exactly ceil(DataLength /
cluster size) long, ending in `FFFFFFFFh`), and the bitmap is read
through a one-sector cache, as the FAT is, because it can be 512 MiB.
Every image in EXP-0011 has a one-cluster bitmap, so the two readings
cannot be told apart on any of them. Unit tests hold this.

**The bitmap is checked before it is believed.** CVE-2025-40307 (Linux,
fixed by `exfat_test_bitmap_range`) is a crafted volume whose bitmap
marks the bitmap's own clusters free. To a recovery tool the same trick
makes structure clusters look free. Before any set is decided, every
cluster the walk itself read or located as structure (the root and
every directory read, the bitmap, the up-case table) must be marked in
use. On all 23 images of EXP-0011 they are.

If the bitmap's chain or this check fails, the bitmap does not describe
the volume, condition 1 cannot be shown for any set, and every deleted
set on the volume is counted as not assessed.

---

## 8. Decision F: two shapes refused

* **ValidDataLength 0, DataLength above 0.** §7.6.5 makes every byte
  past ValidDataLength undefined, so no byte of the file was recorded
  as written, and "not zeros only" has nothing to test. Refused under
  condition 3.
* **A first cluster and a DataLength of 0.** exfatprogs treats it as an
  error (`ER_FILE_FIRST_CLUS`), and since `780d6f0` Taphonomy refuses
  it for a directory. Refused under condition 4. The cluster it names
  is still a claim.

Neither shape occurs on any image of EXP-0011. Unit tests hold both.
The live-file case of the second (`KNOWN_ISSUES.md`) is unchanged by
this record.

---

## 9. Decision G: condition 3 and `--recover`

`ADR-0010` Decision B keeps content unread unless the operator asks.
Without `--recover`, a set that passes conditions 4, 1 and 2 is
reported as such with condition 3 not checked, as FAT32 reports a free
run it did not read.

With `--recover`, the content up to ValidDataLength is first read until
a non-zero byte is found, which is usually the first chunk. A set whose
content is zeros only is refused before any file is opened, so no file
is ever written and then removed for it, and `abort` keeps the meaning
`ADR-0020` gives it, an evidence failure.

---

## 10. Decision H: writing

A recovered set's content is read by `read_content`, which re-checks
the allocation and hands over zeros past ValidDataLength without
reading them (`ADR-0019` Decision F), and pushed through
`ArtifactWriter`. Every error ends in `abort`, including an evidence
error part way through, which `ADR-0021` section 8 left to this
milestone. `abort` never removes a file the run did not create.

The name is `Destination::path`'s
`p<partition>-c<cluster>-s<slot>-first-<n>.bin`, with the cluster and
slot of the set's File entry and its first cluster. `ADR-0020` section
4 left this open. The position is unique on a volume, because every
cluster is read as a directory at most once (`ADR-0021` section 5),
and the partition makes it unique on the image (`ADR-0016` Appendix B).
No byte of a name reaches the path.

---

## 11. Decision I: findings and gaps

`ADR-0014` Appendix B.5 makes a run incomplete wherever a gap is
counted. FAT32 counts a refused deleted entry as a finding, because it
was analysed and the answer was no. exFAT follows it: a set refused
under a condition, or holding no data, is a finding. What could not be
examined is a gap. `ADR-0014` Appendix F lists the kinds. The count of
deleted sets not examined, M14's placeholder, is replaced by the
examination.

**exFAT keeps counts of its own.** FAT32's counts are not reused, even
where a name would fit. `RunCounts::artifacts()` is `free_runs` less
`not_extracted`, `differed()` is built on it, and `matched` and
`free_runs` decide caveats the CLI prints, so an exFAT event counted
in any of them would change what a FAT32 run reports. exFAT has its own
counters for sets not assessed, not extracted and not delivered, for
artifacts, matches, and each condition's refusals. Where a summary line
states the same thing for both filesystems (`entries not assessed`,
`artifacts not written`, `compared`), it prints the sum. A run with no
exFAT volume therefore prints what it prints today, which the identity
proof of section 14 checks.

On EXP-0009's images, which hold no deleted directory and no gap of
any other kind, the run then reports `complete`.

---

## 12. Decision J: bounds

Two bounds are this project's choice and not the specification's. Each
is labelled so in the code and is a parameter a test can make small:

* `MAX_CLAIMED_RUNS = 2^20`, runs held in the claims table for one
  volume;
* `CLAIM_LINKS_PER_CLUSTER = 4`: FAT links followed across every
  deleted chain on one volume are held to four times its ClusterCount.

The second scales with the volume because a fixed bound does not fit
one. A single deleted file of 64 GiB at 4 KiB clusters is 2^24 links,
and a fixed bound of that size would leave every set after it on a large
card unassessed. Each cluster belongs to one chain at a time, so a
volume whose deleted chains follow more links than it has clusters is
following the same links again. On every image of EXP-0011 the clusters
claimed by deleted sets are at most 1.00 times the cluster count
(NIST's dfr-06, one large file), and four times leaves room for clusters
reused by several deleted files. A hostile volume that points millions
of sets at one long chain reaches the bound after reading four times the
FAT, not millions of times.

Reaching either leaves the sets not yet recorded unrecorded: each is
counted as not assessed, and their claims are unseen (section 5). The
worst-case memory of the table is computed from the types' sizes when
they exist and recorded in the implementation's commit and in
`KNOWN_ISSUES.md`, as `ADR-0021` section 5 did. It is not estimated
here.

---

## 13. Decision K: deleted directories are read for their claims

A file written inside a directory that was later deleted still wrote
its clusters. If the walk does not read that directory, its claims are
unseen, and a deleted set elsewhere can be recovered over its data. On
NIST's images, with deleted directories not read, five of macOS's own
files (`/.Trashes/._501`, which begins `00 05 16 07`) are recovered
holding the gzip data of an fseventsd log written inside a deleted
`/.fseventsd`. Reading deleted directories for their claims refuses all
five and loses no correct recovery (section 5's table).

**Which deleted directories are read.** One whose set is usable, whose
allocation is well formed (condition 4), every cluster of which is free
in the bitmap, and none of which was read before. Then its entries are
its own: a cluster in use again, or read already, may hold someone
else's. It is read as any directory is, within the same bounds, and a
deleted directory found inside it is treated the same way.

**What is taken from it.** Every set in it is a deleted set, whatever
its own InUse bit says, because macOS leaves the entries inside a
deleted directory marked in use (EXP-0011). Each such set's allocation
is a claim, with its two keys, exactly as a deleted set's. No set in it
is decided or recovered: each is counted as a deleted set not examined,
a gap (`ADR-0014` Appendix F). Recovering them is M16, decided by the
project on 2026-10-21, so that M15 stays one capability.

**A deleted directory that cannot be read** is counted as a gap, and
its claims are unseen (section 5). On NIST's images eleven cannot,
each `/.Trashes/501`, whose cluster is in use again.

EXP-0011 measured what M16 would add: recovering inside readable deleted
directories adds 23 correct recoveries on NIST's images and no wrong
one, and on the Linux image the 18 deleted files inside its deleted
directories equal the published originals by SHA-256.

---

## 14. Acceptance

M15 merges when all of these hold:

1. **`ADR-0019` section 6 items 1 and 2** on image B, against the
   generator.
2. **Image A**, against the generator: 96 recovered, each equal to the
   file written; 4 refused under condition 1 (the clusters of the live
   `FRAG.bin`); 16 refused under condition 3; 6 holding no data.
3. **Agreement with EXP-0011.** Run with `--recover` (without it,
   condition 3 is not checked, and B's 16 zero-only sets would not be
   decided), on images A and B, NIST's eleven MBR images and the Linux
   image, the binary's decision for every set equals the decision of
   EXP-0011's scripts under rule `moec` (deleted directories read for
   claims), and every set it recovers is right by that image's ground
   truth: the generator for A and B; NIST's tags, or for macOS's own
   files their format; the originals for the Linux image. The two wrong
   recoveries of section 15 are expected, and must be the only ones. Two
   implementations written apart must agree. The scripts do not
   implement the bitmap
   check, orphan-stream claims, the bounds or Decision B's hand-back
   rule; none of these arises on these images, where no deleted set is
   unusable and no Stream Extension is orphaned, so the agreement covers
   conditions 1 to 4 and not those.
4. **Identity** (`ADR-0018` section 5) on every other image, the 31
   `.img` fixtures and the three CFReDS images, against the binary
   before M15, proved twice. On A, the copy of A with its first exFAT
   sector zeroed, and B, every line of the live listing is unchanged.
5. **Unit tests** for what no image exercises: the offset rule (dfr-15
   holds differing bytes, but never on two deleted sets that overlap),
   Decision F's shapes, a bitmap that is a chain, the bitmap check, an
   orphan stream, a live entry inside a deleted span, a deleted set
   straddling two directory clusters, a deleted directory read for
   claims and one that cannot be (cluster in use, already read, chain
   malformed), a set marked in use inside a deleted directory, the
   sweep against a direct
   pairwise check, both bounds, and an evidence error part way through a
   write. Each fails when its rule is removed.

NIST's nine GPT images cannot be run through the binary until GPT is
read. Their figures are EXP-0011's scripts' only.

---

## 15. Consequences

* An exFAT volume's deleted files are recovered where the amended
  conditions hold, labelled `UNVERIFIED`, with the checks they passed.
* **Two wrong recoveries remain on NIST's images under every rule
  measured except the literal one.** On dfr-05 and dfr-02,
  `/.Trashes/._501` is recovered holding fseventsd gzip data. On
  dfr-05 a deleted fseventsd log inside the deleted `/.fseventsd`
  claims the same cluster, and its creation and modification times are
  both earlier than `._501`'s creation, so the timestamps do not order
  what the cluster holds. On dfr-02 no entry the walk found, deleted
  directories included, claims that cluster. Both are stale data that
  is not zeros, which `ADR-0019` section 7 already names and condition 3
  cannot detect.
  A recovery's report states the checks it passed and never that the
  data is the file's (`ADR-0019` section 5), and the label is
  `UNVERIFIED`.
* A run reports `incomplete` on any volume holding a deleted directory,
  an unusable deleted set or an orphan stream. Every NIST image does.
* The summary line can list two levels.
* Recovery reads every claimed chain through the FAT and the bitmap
  through a cache; neither is loaded whole.

---

## 16. Not decided here

* **Recovering files inside deleted directories:** M16.
* **Stale data other than zeros.** Condition 3 detects zeros only.
  EXP-0010 remains open.
* **The cost of the offset rule** on a volume whose files span a
  daylight-saving change, where offset bytes differ and pairs are
  refused (`ADR-0019` Appendix C). No image measures it.
* **GPT**, which NIST's nine GPT images need.
* **Publication of the exFAT modules**, which still awaits the patent
  opinion (`ADR-0021` section 14).

---

## Appendix A: the bitmap check covers live files, and three bounds (2026-10-21)

Section 7 checks the bitmap against the structure the walk knows is in
use: the root and every directory read, the bitmap and the up-case
table. Before it is implemented, an audit found that this leaves live
files out, and that condition 1 is the only condition that keeps a
deleted set off a live file's data: condition 2 compares deleted sets
with each other. A bitmap that marks a live file's cluster free, whether
crafted or left by a volume removed during a write, lets a deleted set
pass condition 1 over that file. exfatprogs' checker (`8b42748`,
`fsck/fsck.c`) treats exactly this as an error, `ER_FILE_INVALID_CLUS`,
"cluster ... is marked as free".

**The check covers live allocations.** Before any set is decided, every
cluster of each live file's and live directory's allocation, as its set
describes it, must be marked in use: a run where NoFatChain is set,
otherwise the chain, followed until it has the clusters DataLength
needs or the FAT does not go on. Every cluster reached counts. A live
directory that was not read is covered this way too. The project
decided this on 2026-10-21.

**Three readings the implementation is to follow.**

* A cluster taken as a directory's next cluster counts even where
  reading it then failed, because the FAT marks it bad or its FAT entry
  could not be read. That directory's allocation consumes it, which
  §7.1.5.1 says a set bit describes, and §7.1.5.1 describes a cluster
  the FAT marks bad as not available for allocation either way. Linux
  (`f47d223`, `exfat_find_free_bitmap`) allocates the first clear bit
  without reading the FAT, so a volume whose bad clusters are not marked
  in use is not safe for Linux to write.
* The root must hold exactly one up-case table entry: "The valid number
  of Up-case Table directory entries is 1" (§7.2). With none, or more
  than one, the bitmap is not believed. Linux and exfatprogs use the
  first entry and The Sleuth Kit lists every one, none of them with an
  error; Windows and macOS were not tested. Refusing is stricter than
  each, and costs assessment, never a wrong recovery.
* The up-case table is located through its FAT chain (§7.2.3), held to
  the rules section 7 holds the bitmap's chain to. Its TableChecksum is
  not verified: §7.2.2 requires that before the table is used, and the
  check does not use it.

**Three bounds, each this project's choice and not the
specification's,** besides the two of section 12, which are unchanged.
Each is to be labelled so in the code.

* `MAX_UP_CASE_BYTES = 131072`: an uncompressed table, one 2-byte
  mapping for each of 65,536 code units (§7.2.5). The specification sets
  no maximum, and exfatprogs treats a longer table as an error. It holds
  the chain followed to 256 clusters. A length of 0 is refused too;
  Linux and exfatprogs treat it as an error.
* `MAX_LIVE_ALLOCATIONS = 2^22` live sets recorded, to be a field of
  `Limits` so a test can make it small. A record of four fields, of 4,
  4, 1 and 1 bytes, measures 12 bytes with its alignment, which holds
  the record to 48 MiB. A volume with more is not believed, because a live
  file not recorded could be marked free.
* `LIVE_CLUSTERS_PER_CLUSTER = 4`: the clusters looked up for live
  allocations, as a multiple of ClusterCount. Each cluster belongs to
  one allocation at a time, so a consistent volume needs at most one
  times; four is section 12's multiple, and leaves room for allocations
  that overlap.

**When it runs.** After the walk of live directories and before any
deleted directory is read: Decision K reads a deleted directory only
where its clusters are free, so once one is read its clusters would
fail the check.

**Measured.** EXP-0011 Appendix A. On 29 volumes, EXP-0011's 23 and six
made by three more formatters, every cluster of structure and of live
allocations is marked in use, and live allocations take at most 0.9997
times the volume's clusters. FatFs writes an up-case table of 4,104
bytes; every other volume has the recommended 5,836. What no volume
exercises, a bad cluster in a directory's chain, a missing or second
up-case entry, a fragmented bitmap or up-case chain, and each bound, is
left to unit tests, which the implementation is to carry.

If the check fails, every deleted set on the volume is counted as not
assessed, as section 7 says.

---

## Appendix B: phase 1's deleted sets, the entry count, and a bound (2026-10-22)

Section 5's phase 1 records each usable deleted set. Before it is
implemented, an audit settled four readings of Decision B, found that
one count the command line prints changes, and found that the record
needs a bound.

**How a deleted set is read.**

* An unused File entry that declares fewer than two secondaries is not a
  File set, as a live one is not (`ADR-0021` section 4). It consumes
  nothing, so what follows it is read as entries of its own, as after a
  live File entry refused for the same reason.
* Otherwise the set consumes, up to its declared count, the unused
  secondaries that follow: any entry whose EntryType bits `C0h` equal
  `40h`. That is a Stream Extension (`40h`), a File Name (`41h`) or an
  unused benign secondary (`60h` to `7Fh`), as Decision B lists, and
  also `42h` to `5Fh`, unused critical secondaries this version does
  not recognise, which Decision B did not name. A live set is collected
  the same way, taking any in-use secondary. A set holding one of
  `42h` to `5Fh` is to fail its checks as a live set does, so it is
  counted and not recorded. The set stops at the first entry that is
  not an unused secondary, which is handed back (Decision B). The
  project decided this widening on 2026-10-23.
* A set that is complete but fails its SetChecksum or a check of
  `ADR-0021` section 4 has still consumed its own entries. They are not
  entries outside a set, so an unused Stream Extension among them is
  not an orphan (`ADR-0014` Appendix F).
* A set cut short, by a live entry, an end marker, another unused File
  entry or the end of the directory's data, is not usable, even where
  the entries read would pass every check.

A usable set is recorded with the position of its File entry, the
clusters its entries lie in, its attributes, FirstCluster, DataLength,
ValidDataLength and NoFatChain, and the bytes of its creation and
modification timestamps, 10 ms increments and UTC offsets, uninterpreted
(`ADR-0019` Appendix C interprets them). A set that is not usable is
counted and not recorded: its fields are not trusted, so it claims
nothing.

**The entry count changes.** Each directory's summary prints how many
entries were read at its top level, and the code documents the count as
counting an entry set once, however many secondaries it holds. M14 did
not read deleted sets as sets, so it counted each unused entry of one
on its own. Reading them as sets makes the count what its documentation
says, and lowers it in every directory that holds a deleted set; no
other line of output is to change. Of the 37 images section 14's
identity proof runs over, the count is to change on three. On A and B
the root's count falls from 491 and 497 to 247; the copy of A with its
first exFAT sector zeroed differs from A only in that sector and is
expected to change as A does, which section 14 item 4's identity proof
is to show.
Section 14 item 4 asks that every line of the live listing on these
three images be unchanged: every line listing a live entry is to be,
and the count line is to change as stated here. The project decided
this on 2026-10-22.

**A bound on the record.** `MAX_DELETED_SETS = 2^20` usable sets
recorded on one volume, to be a field of `Limits` so a test can make it
small. A choice of this project, not the specification's, and the same
number as section 12's claims bound. A record is to hold a position (16
bytes), a pointer into the list of clusters, the allocation fields and
the two times, which as laid out in the trial implementation measures
64 bytes with alignment: 64 MiB when full, and up to 96 MiB while it
grows and the old and new buffers are both held. The clusters a set's
entries lie in are held in one list beside it, one or two for almost
every set and at most 17. A usable set past the bound is counted as not
recorded, and is not assessed, as section 12 says of sets past its
bounds.

**Measured.** EXP-0011 Appendix B. On the 23 volumes of EXP-0011, 985
deleted sets lie outside deleted directories, every one usable (judged
by the checksum and the Stream Extension's place only; EXP-0011
Appendix B, Limitations); at most 262 lie on one volume; 12 straddle
two directory clusters; and no declared span is cut short. A set cut
short, an unusable set, a set holding one of `42h` to `5Fh`, a count
too small and each bound are left to unit tests, which the
implementation is to carry.

---

## Appendix C: condition 2, orphan claims and the claims bounds (2026-10-26)

Before phase 2's claims and condition 2 are implemented, an audit read
`ADR-0019` Appendix C and section 5 of this record against EXP-0011's
scripts, and benchmarked the readings on volumes made with two writers
(EXP-0011 Appendix C). It found one reading that fails its benchmark,
one description that the decided rule cannot use, and three things
section 5 leaves open. The project decided the reading of condition 2,
OffsetValid, the orphan's claim and bound, and the read error on
2026-10-26, and the claims bounds' order and the algorithm on 2026-10-29.

**A set is judged by its rivals' times against its own creation.**
`ADR-0019` Appendix C says that where two keys cannot be compared,
"condition 2 does not hold for either". Read literally, a set whose own
modification falls under another offset byte, as a daylight-saving
change makes it, is refused wherever a rival created under its
creation's byte shares a cluster. EXP-0011's scorer reads it otherwise.
On volumes relan's exfat wrote with a file created under one offset and
modified under another, the literal reading refuses that file, which
holds its own data, and the scorer's reading recovers it; neither
recovers a file wrongly. On EXP-0011's 23 volumes no pair sharing a
cluster mixes offset bytes, so the readings never differ there. Condition
2 is therefore to read: a set is refused where another claimant sharing
one of its clusters has a creation or modification key that cannot be
compared with this set's creation key, or one at or after it. The set's
own modification key does not decide its own case. What shows that a
set wrote last is its creation against each rival's two times, and that
comparison is the same under both readings, so this refuses no fewer
sets holding another file's data. A pair that cannot be ordered still
refuses the rival whose creation the set's times cannot be compared
with. This amends `ADR-0019` Appendix C's sentence; nothing else in it
changes.

**OffsetValid is still required.** FatFs writes `00h` into every offset
byte. On volumes it wrote where later files reused deleted files'
clusters, condition 2 refuses every set sharing a cluster, two of them
holding their own data, and recovers none wrongly; taking the offsets as
valid and equal would recover those two. A byte with OffsetValid clear
says nothing of the time zone, and comparing two such times would trust
that their writers shared one, which is what the requirement exists to
refuse. The cost is a recorded limit.

**An orphan Stream Extension claims as a deleted set's allocation
would.** Section 5 says an orphan "claims the clusters its fields name
where they are in range". It is to claim a run where NoFatChain is set,
cut at the heap's end, and otherwise the chain from its first cluster,
followed until it has as many clusters as DataLength needs or the FAT
does not go on; nothing where its first cluster is outside the heap or
its DataLength is 0. It has no time, so every set sharing a cluster with
it is refused under condition 2. On a volume FatFs wrote, where a new set
overwrote a deleted file's File entry and left its Stream Extension, the
deleted file whose clusters that file had reused is refused with orphan
claims and recovered wrongly without them. An orphan is an unused
Stream Extension (`40h`) outside any set, as phase 1 reads sets
(Appendix B), in a directory the walk reads.

**Orphans share the deleted sets' bound.** Phase 1 keeps orphans within
`MAX_DELETED_SETS`, sets and orphans together. An orphan past it is
counted, and its claim is unseen.

**The claims bounds.** Claims are followed in the order the walk found
them, deleted sets first, then orphans. Where a claimant's claim would
pass `MAX_CLAIMED_RUNS` or `CLAIM_LINKS_PER_CLUSTER` times ClusterCount
FAT links, its runs followed so far are let go: it and every claimant
after it are not recorded, as section 12 says. Links are counted once
per FAT entry read for a claim. A chain that reaches a cluster twice
counts it once in its claim.

**The algorithm.** Section 5 describes a sweep keeping, per stretch of
clusters, "the latest key, the second latest, and whether two keys there
cannot be ordered". Under the rule above, whether a rival can be ordered
against a set depends on the set's creation byte, so one flag per stretch
cannot say it. The implementation is instead to decide in two passes over
a tree that adds a count to a range of stretches and answers the largest
count in a range. The first, with every claimant counted, takes away for
each offset byte the claimants whose two keys both carry it; a set whose
creation carries it is then refused as unordered where any claimant but
itself is left on one of its stretches. The second adds, for each byte,
those claimants in order of their later key, and refuses a set where any
claimant but itself whose later key is at or after its creation lies on
one of its stretches. Both are O(R log R) for R runs. Tests are to
compare it with a direct check of every pair. The worst-case memory at
both bounds is to be recorded with the implementation, as section 12
says.

**A read error inside a deleted set.** Phase 1 is to let a set's
clusters go before passing up an error met inside it, so the list of
clusters holds only recorded sets'. Sets recorded before the error stay
recorded and claim: they were seen. The directory not read whole is a
gap already, and its unseen claims with it.

**EXP-0011's scorer treats every deleted set as a rival**, unusable or
not; Decision B says an unusable set claims nothing. No set on EXP-0011's
images is unusable, so no figure depends on the difference.

**An equal, valid offset byte is not proof of one clock.** Appendix C
of `ADR-0019` calls comparing local times under the same byte immune to
macOS's switched sign. That holds where one writer wrote both times. A
card used by Windows and by macOS can carry one byte meaning opposite
offsets, and Linux can stamp `80h`, valid UTC, on digits another writer
stored as local time (RESEARCH_LOG, 2026-10-29, findings 6, 10 and 11).
Two sets may then be ordered wrongly, and the earlier recovered. This
widens the limit `ADR-0019` Appendix C records. It is to be measured on
the published images of Nordvik and Axelsson's study before stage 6
decides whether condition 2 needs a margin around close times; the
project decided this on 2026-10-29.

**What no public image exercises:** a pair mixing offset bytes, an orphan
Stream Extension, a claimant past either bound, a chain reaching a
cluster twice, and an error inside a deleted set. Volumes made here hold
the first two (EXP-0011 Appendix C); unit tests, which the
implementation is to carry, are to hold every one, each failing when its
rule is removed.
