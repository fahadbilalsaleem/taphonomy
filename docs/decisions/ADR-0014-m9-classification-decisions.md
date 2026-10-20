# ADR-0014: M9 Classification and Reporting Decisions

**Status:** Accepted
**Date:** 2026-09-15
**Milestone:** M9 (`ADR-0002` §8)
**Supersedes:** nothing
**Amends:** nothing. `ADR-0003` is amended by its own appendix, an action of
this record.

---

## 1. Context

`ADR-0002` §8's ninth and final milestone is to classify and report the
result with a confidence level. `ADR-0003` defined the taxonomy a year
before any of it was built, and `ADR-0003` §8 reserved it for this
milestone: the taxonomy becomes binding on the first code that assigns a
confidence level.

This record is that code's decision document. It was prepared by auditing
`ADR-0003` against the tool at `e033ea9`, against the constraining documents
that cite it, and against the measurements in EXP-0003 and EXP-0004.

The audit's principal finding is that the taxonomy cannot be implemented as
written. Three of its six levels have no reachable code path, one of M8's
three outcomes has no level, and one level misdescribes the case it would be
assigned to. §3 records this in full. The decisions that follow are
constrained by it.

---

## 2. Basis

Read and quoted in preparing this record:

* `ADR-0003` §§1, 2, 3.1, 3.2, 3.3, 4.1–4.7, 5, 7, 8, Appendix A
* `ADR-0013` §§3, 7, and Appendix A
* `ADR-0002` §8
* `SAFETY.md` §§10, 12, 13, 14
* `SECURITY.md` §31
* `ADR-0011` §3
* EXP-0003, EXP-0004 and its Appendix A
* `src/main.rs` reporting path, `src/fat_recovery.rs`, `src/validation.rs`

External practice consulted:

* NIST CFTT, *Active File Identification & Deleted File Recovery Tool
  Specification*, Draft 1 of Version 1.1
* Jim Lyle, *Creating Deleted File Recovery Tool Testing Images*, NIST/CFTT,
  AAFS 2012
* Meyer and Roy, *Do Metadata-based Deleted-File-Recovery (DFR) Tools Meet
  NIST Guidelines?*, EAI Endorsed Transactions on Security and Safety, 2020
* ENFSI, *Guideline for Evaluative Reporting in Forensic Science*
* Piriform Recuva's published recovery states, and independent testing of
  their reliability

---

## 3. The taxonomy cannot be implemented as written

Each level in `ADR-0003` §3.2 carries an evidence requirement. Checked
against the tool at `e033ea9`:

| Level | Evidence `ADR-0003` requires | State |
| --- | --- | --- |
| `VERIFIED` | Cryptographic hash match against a reference artifact | reachable; contested by §4.5, resolved in Decision A |
| `STRUCTURALLY_VALID` | Format-aware validator completed without error over the entire artifact | **unreachable** |
| `PARTIAL` | Validator succeeded over a bounded prefix or region; the missing extent is known and recorded | **unreachable** |
| `RECONSTRUCTED` | Reconstruction method and its inputs recorded; validator result recorded separately | reachable, and universal |
| `UNVERIFIED` | Extraction succeeded; no validator exists for the format, or validation could not run | **misdescribes** |
| `UNRECOVERABLE` | Basis for the determination recorded | **unreachable** |

**3.1 `STRUCTURALLY_VALID`.** No format-aware validator exists. M9 is the
last of `ADR-0002` §8's nine milestones and none is scheduled. The level
cannot be assigned by any code path that exists or is planned.

**3.2 `PARTIAL`.** `extract` reads the whole declared size or the run is
refused before any content is read. No bounded-prefix extraction exists, so
no artifact of this shape can be produced.

**3.3 `UNRECOVERABLE`.** `ADR-0003` §3.1 makes a level a property of an
Artifact, and an Artifact is the Validator's output. A refused run produces
no Candidate and therefore never reaches the Validator. `SAFETY.md` §14
closes it independently: confidence classifies a recovered artifact, and
does not classify an operation or a structural finding such as the
identification of a deleted directory entry. A refused entry is a structural
finding.

**3.4 `UNVERIFIED`.** Its evidence column is that no validator exists for
the format, or validation could not run. M8's `NotAttempted` means no
reference was supplied. Validation could have run; nobody asked it to.
Assigning `UNVERIFIED` there would state something untrue about the run.

**3.5 The gap.** A comparison that ran and refuted — M8's `Differs` — has no
level. §4.4's fail-closed default resolves a tie between two applicable
levels; here none applies.

**3.6 Why.** `ADR-0003` §1 records the taxonomy's provenance. The six terms
came from `SAFETY.md` §14 and `PROJECT.md` §6.9, which a documentation audit
found to be the same taxonomy under different names. `ADR-0003`'s task was
to reconcile three incompatible models, and §2's separation of pipeline
state from evidence strength is correct and has held. But the list of levels
was adopted rather than derived, before any pipeline existed. §8 anticipated
half of this by reserving implementation; the speculation was in the list
itself.

---

## 4. Decisions

| | Decision |
| --- | --- |
| A | The classification states method; the comparison result is reported beside it |
| B | Entries that produce no Artifact carry no level |
| C | M9 defines the operation-status axis and records that it is defining it |
| D | Aggregate reporting counts levels, outcomes, and what was never classified |
| E | Unreachable levels are recorded as unreachable, not implemented |

---

## 5. Decision A: the classification states method

**Every artifact this tool produces is classified `RECONSTRUCTED`. A digest
match is reported on the outcome axis and does not promote the level.**

`ADR-0003` §4.2 says a level may only be raised by evidence satisfying the
higher level's requirement, and a match does satisfy `VERIFIED`'s. §4.5 says
`RECONSTRUCTED` describes the method used, not a position between `PARTIAL`
and `UNVERIFIED`, and that a reconstructed artifact records its validator
result separately from its classification. Both cannot govern: a level
cannot be raised out of a category the same ADR excludes from the ordering.

§4.5 governs, for four reasons.

**5.1 Every extraction infers its extent.** EXP-0003 measured that deletion
zeroes the cluster chain in every FAT. The run is computed from a first
cluster and a declared size; contiguity is assumed, never read. `ADR-0013`
Appendix A records that the CFTT specification's term for this is
*estimating content* — recovering beyond what residual metadata explicitly
identifies. That is §3.2's definition of `RECONSTRUCTED`, and it is true of
every recovery the tool performs.

**5.2 A match does not change the method.** It establishes that the
inference was correct for that entry. `ADR-0003` §4.6 and `SAFETY.md` §10
already bound what a match establishes, and `ADR-0013` Decision D bounds it
further where content is not distinctive.

**5.3 The tool cannot discriminate.** EXP-0004 Appendix A.2 measured five
deleted entries on one volume: three recover content that is not their own
file's, two recover correctly, and the tool reports all five identically.
The two that are correct are correct because nothing reused their clusters,
which is not a property of the method. A level that varied would assert a
discrimination the tool measurably does not have.

**5.4 The varying version exists and fails.** Recuva publishes a per-file
recovery state computed from the same evidence this tool has: its
`Excellent` means no clusters are overwritten by a live file, which is this
tool's *every cluster free*. Independent testing reports the indicator
unreliable in both directions, with files marked `Excellent` recovering
corrupt and files marked `Unrecoverable` recovering intact. Meyer and Roy
measured the mechanism: on a fragmented deleted file whose intervening
clusters are unallocated, Recuva is among the tools that recover data
belonging to other files. That is the case EXP-0004 fixtures.

**5.5 `SECURITY.md` §31, left unexercised deliberately.** §31 states that
validation is required before a result is classified as verified. That is a
conditional ordering constraint and not a requirement that `VERIFIED` be
assigned; it is satisfied where nothing is so classified. Under this
decision a match is reported only after validation has run, so the ordering
holds in substance. The phrase is recorded here as considered and not
overlooked.

§31's list of what a recovered file may contain includes *remnants from
another file*. EXP-0004 Appendix A.2 is the first measurement of that
condition in this tree.

**5.6 Consequence, stated plainly.** The level is constant. §4.7's counts
per level will report every artifact under one name. A field that never
varies carries no information, and that is the correct outcome: the
information is in the outcome axis and in what carries no level at all. A
level that varied would be the misleading version.

---

## 6. Decision B: entries that produce no Artifact carry no level

**No confidence level is assigned to an entry that is refused, ineligible,
or whose assessment or extraction failed.**

`ADR-0003` §3.1 makes a level a property of an Artifact. `SAFETY.md` §14
forbids classifying a structural finding such as the identification of a
deleted directory entry. `Assessment::Ineligible`, `Assessment::RunBroken`,
and the error arms of `assess` and `extract` all terminate before a
Candidate exists.

These entries are reported, and what is reported about them is unchanged by
this milestone. `ADR-0010` Decision C's named refusal stands.

This is a non-conformance with the CFTT specification, which requires a
Recovered Object for each deleted file system object accessible in residual
metadata. `ADR-0013` Appendix A already records it. This decision does not
introduce it and does not resolve it.

---

## 7. Decision C: M9 defines the operation-status axis

**M9 defines the operation-status vocabulary. It is not inherited from
`SAFETY.md` §13.**

`SAFETY.md` §13 is titled Partial Results. It requires that partial recovery
be explicitly represented, illustrates the requirement with two strings, and
states that a partially recovered file must not be presented as a complete
verified file. It defines no status set and names no failure status. `main.rs`
emits no status line; the only `SUCCESS` in it is `ExitCode::SUCCESS`.

Three documents nonetheless attribute a vocabulary to it: `ADR-0003` §5
refers to §13's `SUCCESS` / `PARTIAL` / failure statuses, `ADR-0013` §3.2
refers to §13's statuses, and the M9 handover carries the same reading
forward. The attribution is a pattern rather than a slip, and it is recorded
here so that M9 does not become its fourth instance.

The axis M9 defines:

* A run that completed reports `SUCCESS`, whatever its artifacts are
  classified and whatever its comparisons found. `ADR-0013` §3 settled that
  a differing digest is a finding and not a failed operation, and §3.1
  scoped `SAFETY.md` §12's `Hash mismatch` to source-versus-image integrity.
* A run in which any entry could not be assessed or extracted reports
  `PARTIAL`, which is the condition `SAFETY.md` §13 exists to require be
  explicitly represented.
* Fail-closed conditions are unchanged. `ADR-0013` §3.4's malformed
  reference digest remains the only one in this path, and it halts before
  evidence is read.

The two axes are never merged, per `ADR-0003` §5 and `SAFETY.md` §14.

---

## 8. Decision D: what aggregate reporting counts

**A run reports counts per level, counts per outcome, and a count of entries
that carry no level at all.**

`ADR-0003` §4.7 requires counts per level, forbids a single combined success
figure, and forbids describing a session as successful where any artifact is
below `STRUCTURALLY_VALID` without stating the distribution. Under Decision
E `STRUCTURALLY_VALID` is unreachable, so every artifact is below it and the
distribution is always stated.

The third count is the one the milestone most needs and the one nothing
currently supports: a run-level statement that some entries were never
classified, because they were refused, ineligible or errored. Without it a
reader of the per-level counts cannot tell how much of the volume they
describe.

This cannot be built on the current reporting path. `report_recovery`
returns a `Caveats` of three booleans, `report_partition` returns `()`, and
`print_caveats` runs per volume while §4.7 requires per session. Threading
return values through `report_recovery` → `report_partition` → `inspect` is
a prerequisite commit and carries no classification.

---

## 9. Decision E: unreachable levels are recorded, not implemented

**`STRUCTURALLY_VALID`, `PARTIAL` and `UNRECOVERABLE` are not implemented.
`UNVERIFIED` is not assigned.**

`ADR-0003` §8's reasoning applies to the variants as it applied to the enum:
implementing a level no code path can produce is speculative. A variant that
cannot be constructed is dead code that invites a future reader to construct
it wrongly, which is what `UNRECOVERABLE` would invite on a refused entry in
breach of `SAFETY.md` §14.

`ADR-0003` receives an appendix recording §3's findings, so that its
taxonomy and this tool's reachable subset are reconcilable by a reader who
has only the ADR series. Writing it is an action of this record.

This decision is a statement about this tool at this milestone, not a
rejection of the taxonomy. `STRUCTURALLY_VALID` becomes reachable on the
first format-aware validator. `PARTIAL` becomes reachable on the first
bounded extraction. Either would reopen Decision A, and §11 records that as
the review trigger.

---

## 10. What M9 does not do

* It does not write a recovered file. No output path exists and none is
  introduced here.
* It does not change what any entry currently reports about itself.
* It does not add a level that varies with the outcome. Decision A.
* It does not resolve the CFTT non-conformances recorded in `ADR-0013`
  Appendix A and EXP-0004's external practice section.
* It does not measure any fragmentation arrangement beyond the one EXP-0004
  built.

---

## 11. Review trigger

This record is reopened if any of the following becomes true:

* a format-aware validator is built, making `STRUCTURALLY_VALID` reachable;
* a bounded or partial extraction is built, making `PARTIAL` reachable;
* a means is found to establish a deleted file's extent from surviving
  evidence rather than assumption, which would make the classification vary
  on something measured;
* `ADR-0003` is amended in a way that changes §4.5.

The first three would each give the level something real to vary on, and
Decision A exists only because none of them does.

---

## 12. Implementation order

1. Thread return values through the reporting path. No classification.
2. Introduce the classification and assign it. Decisions A, B, E.
3. Operation status. Decision C.
4. Aggregate reporting. Decision D.
5. Tests, including that no entry under Decision B carries a level.
6. `ADR-0003` appendix; `README.md`, `CHANGELOG.md` and `PROJECT.md` §6.9
   reviewed against what was built.

---

## 13. Consequences

**Positive.** The tool states how it obtained every byte it reports, and
declines to assert a discrimination it has been measured not to have. The
two axes `ADR-0003` §5 separates stay separated in code. A reader of the
run-level counts can tell what fraction of a volume they cover.

**Negative.** The headline feature of `ADR-0002` §8's final milestone is a
constant. A reader expecting a varying confidence indicator will find the
information elsewhere in the output, and the ADR series must be read to
understand why. Three of six levels exist in a governing ADR with no code
behind them, which is a discrepancy a future maintainer will have to
re-derive if `ADR-0003`'s appendix is not written.

---

## 14. Open after this decision

* Whether `PROJECT.md` §6.9 and `ARCHITECTURE.md` §36 need amendment once
  the classification exists in code. Not assessed here.
* Whether the constant level should appear in output at all, or only in the
  aggregate. Decided during implementation, recorded by appendix.
* The sixteen CFTT test cases EXP-0004 does not cover.

---

## Appendix A: M9 as built, and three corrections to Decision C (2026-09-16)

Steps 1 and 2 of §12 are complete. Preparing step 3 required auditing
Decision C against the tool and measuring what it would fire on, which
found the decision mis-justified and its trigger set empty. §5's Decision A
and §9's Decision E are unaffected and were implemented as written.

The body is not rewritten.

---

### A.1 Coverage gaps, measured

Every fixture was run with `--recover` at `41df92c`, with `stdout` and
`stderr` captured separately.

| Gap | Images | Stream |
| --- | --- | --- |
| MBR table rejected, nothing analysed | `bad-signature`, `no-signature`, `partition-beyond-end` | `stderr` only |
| GPT detected, unsupported | `gpt-protective` | `stdout` |
| Boot sector rejected, volume not analysed | `fat32-bad-root-cluster`, `fat32-oversized-volume`, `fat32-undersized-fat` | `stdout` |
| Filesystem not identified, volume not analysed | `mbr-four-partitions`, all four partitions | `stdout` |
| An entry could not be assessed or extracted | none | — |

Eight of nineteen images carry a gap. Ten are analysed end to end.
`mbr-empty.img` is neither: it declares no partition entries, so nothing
was missed and nothing was uncovered.

---

### A.2 Decision C's trigger set is exercised by nothing

§7 fires the `PARTIAL` status on a run in which any entry could not be
assessed or extracted. That is the last row of the table above, and no
fixture produces it. It is the narrowest gap the tool can have and the
only one the fixture set does not contain.

Meanwhile the four kinds that do occur are not named by Decision C at all,
including three whole-image failures and one whole-image refusal.

The trigger set is therefore replaced. §A.7 states the new one.

---

### A.3 The attribution to `SAFETY.md` §13 is unsupported

§7 states that the `PARTIAL` status is the condition `SAFETY.md` §13
exists to require be explicitly represented. §13 in full is four
sentences: that partial recovery is allowed but must be explicitly
represented, two example `Status:` strings, and that a partially recovered
file must not be presented as a complete verified file.

It states no purpose. §7's claim about what §13 exists to do is the
record's own reading presented as the document's content.

§13 is also ambiguous. "Partial recovery" reads either as a run that
partly succeeded, which is coverage, or as recovery of part of a file,
which this tool never performs: §3.2 records that `extract` reads the whole
declared size or the run is refused before any content is read. Its only
unambiguous sentence concerns a file.

This appendix does not resolve that ambiguity and does not need to. §13 is
not the warrant for the axis. §A.4 is.

That §7 over-attributed to §13 is worth recording plainly, because §7's own
subject is that three documents attribute a vocabulary to §13 which §13
does not define. It became the fourth.

---

### A.4 What does warrant the axis

**`SAFETY.md` §14, which is unambiguous and constraining.** It states that
confidence classifies a recovered artifact, does not classify an operation,
and does not classify a structural finding such as the identification of a
deleted directory entry. Forbidding confidence from classifying an
operation presupposes that operations are classified. On a reading where
nothing classifies them, §14 reserves a job no part of the tool performs.

`ADR-0011` §3 makes `SAFETY.md` a constraining document, which is not
updated to match an implementation that violates it. So the axis is not
M9's to decline.

§14 sources that sentence to `ADR-0003` §5, and §5 attributes the
vocabulary to `SAFETY.md` §13, which defines none. The chain is circular
and terminates in an illustration. The presupposition is real; no document
supplies the vocabulary. Decision C was correct that M9 defines it.

**A measured harm in not having it.** Failures are split across streams.
`BOOT SECTOR REJECTED`, the GPT line and an unidentified filesystem print
to `stdout`; an MBR parse failure, a sector read failure and a partition
read failure print to `stderr`. `inspect` returns `Ok(())` in every one of
those cases and `main` maps that to `ExitCode::SUCCESS`
(`src/main.rs:107`), so the process exits zero.

An operator capturing `stdout` alone therefore receives, for
`bad-signature.img`, `no-signature.img` and `partition-beyond-end.img`, a
report in which nothing indicates that no partition was analysed, from a
process that succeeded. A status on `stdout` is the only element that would
surface it.

---

### A.5 An anomaly is not a gap

`mbr-type-mismatch.img` prints `MISMATCH: declared type 0x07 disagrees with
observed filesystem FAT32` and then analyses its volume completely.
`fat32-hidden-mismatch.img` behaves the same way. Both are findings about
evidence that *was* covered.

A status that fired on them would merge a finding about content with a
statement about coverage, which is the merging `SAFETY.md` §14 and
`ADR-0003` §5 forbid. Neither is a gap and neither affects the status.

---

### A.6 Coverage states the tool's reach, not what was missed

`mbr-four-partitions.img` was expected, from its declared partition types,
to contain two unsupported Linux volumes. Measured, all four of its
partitions report `no boot signature: found 0000, expected 55aa`,
including both `0x0c` entries: none was ever formatted.

So the true statement is that the tool could not identify what is in those
extents, not that it does not support what is there. From ground truth they
are empty, and nothing in the evidence tells the tool that.

A coverage statement therefore reports what the run did not analyse. It
must not imply anything about what was there, because the tool cannot know.
`mbr-empty.img` is the control that keeps the two apart: nothing found,
nothing uncovered, and any status other than a successful one would be
wrong.

---

### A.7 The trigger set, replacing §7's

A run has a coverage gap where any of the following occurred:

1. the partition table could not be parsed, so no partition was analysed;
2. a partition table was parsed but is GPT, which is unsupported;
3. a partition's first sector could not be read;
4. a partition's filesystem could not be identified, or was identified as
   one this tool does not analyse;
5. a FAT32 boot sector was rejected;
6. a root directory could not be enumerated;
7. an entry's assessment or extraction failed.

Kinds 6 and 7 are unexercised: no fixture produces either. A test for
them requires fixtures that do not exist, and building them is not this
milestone's work. They are included because the code paths exist and a
status that ignored them would be wrong on evidence the tool may meet.

`Assessment::Ineligible` and `Assessment::RunBroken` are **not** gaps. Both
are reported refusals with a stated basis, which `ADR-0010` Decision C
treats as correct behaviour, and both concern one entry rather than the
run's reach. Decision B already denies them a confidence level; this
appendix denies them a status effect for the same reason.

`Ok(None)` from `assess` is not a gap. The entry is not a deleted file and
nothing about it was skipped.

---

### A.8 The status is derived, not tracked

The status is computed from the coverage counts rather than maintained
alongside them, so that the two cannot disagree. Where the count of
uncovered partitions and entries is zero the run reports success; otherwise
it reports a gap and the counts state its extent.

This follows CFTT's disk imaging practice, where an acquisition over a
drive with unreadable sectors is reported as *qualified* rather than equal,
with the count and location of what could not be read recorded alongside.
The qualification is the enumeration; the word is its summary.

---

### A.9 §12's steps 3 and 4 become one

§12 listed the operation status and the aggregate reporting as separate
steps. Under §A.8 the status is a function of the counts, so building it
before them would mean building a status with nothing to derive from. They
are one commit.

---

### A.10 Steps 1 and 2 as built

**Step 1** (`4cc673c`) was listed as threading return values through the
reporting path. It also moved `print_caveats` from `report_root_directory`
to `inspect` and reduced its indentation to zero, because a block printed
once per run at volume indentation would read as a statement about the last
volume printed. `ADR-0013` Appendix C.6's measurement was preserved: a run
over `fat32-recover-run.img` still emits the free-run paragraph once.

**Step 2** (`41df92c`) implemented Decisions A and E as
`src/confidence.rs`. It is filesystem-independent and parallel to
`validation`, rather than inside `fat_recovery`, which is FAT-specific and
which exFAT and NTFS would then have to import a general concept from.
`classify` was rejected as a name: `src/fat_directory.rs:492` already
declares `pub fn classify` for deciding an entry's kind, and the word
appears 34 times in the crate with that meaning.

No derivation function was written. Under Decision A the level follows from
the method, which is fixed, so a function would take no argument that could
change its answer. `Confidence::of_inferred_run` is an associated constant
whose name states the premise instead.

**§14's open question on output is decided: the level appears in the
aggregate only, not on each entry.** Each entry already states the
inference in concrete terms — its run, its extent and every cluster's
allocation state — so a constant word per line adds a label rather than a
qualification. Warning research finds attention to a repeated identical
stimulus falling measurably after two or three exposures, and names the
failure mode of a uniform label as protecting the author rather than
informing the reader. That research concerns warnings meant to change
behaviour rather than data fields, so it is recorded as a reason for
caution and not as proof. The decisive point is the first: the per-entry
lines are already qualified.

---

### A.11 External practice recorded, changing no decision

Researched while auditing Decision C. None of it bears on M9, and it is
recorded here so it is not lost.

Deleted-file recovery tools are scored on precision, recall and F1 against
NIST CFTT ground truth, per test case, with the mean F1 across suites
forming an overall figure; `AutoDFBench 1.0` implements this over 63 test
cases. These are validation-time measures computed against known ground
truth, not runtime output, so they belong in this project's test suite and
experiment records rather than in anything M9 prints.

Three gaps against that frame:

* Taphonomy parses no directory entry timestamps, so the MAC-times
  parameter is unmeasurable against it rather than measured badly.
* The fragmentation test cases score how many complete file blocks match
  ground truth. This tool hashes the whole declared size and emits one
  binary verdict, so on `fat32-fragmented-deleted.img` it reports only that
  the digest differs, where two of three clusters are in fact the file's.
  Per-cluster digests would make the tool measurable on those cases.
* One test case of seventeen is built, so no score can be computed.

---

### A.12 What this appendix does not change

Decisions A, B, D and E stand as written. Decision C's axis stands; its
justification, its trigger set and its silence on anomalies are corrected
above. §11's review triggers are unaffected. No decision here concerns
what the tool recovers, refuses or validates.

---

## Appendix B: coverage re-measured, the status made three-valued, and exit status decided (2026-09-17)

Designing step 3 required auditing Appendix A's trigger set and derivation
against the tool at `51d4ae0` before building on them. The audit found a
coverage gap that §A.7 does not name and that four fixtures carry, a
derivation that cannot tell a rejected partition table from an empty one,
and an inconsistency in Decision D. It also required deciding two things no
document decides: where the aggregate lives and what the exit status
reports.

The body and Appendix A are not rewritten.

---

### B.1 Basis

Read and quoted in preparing this appendix:

* this record's §§5, 7, 8, 10, 11, 12, and Appendix A in full
* `ADR-0003` §§4.5, 4.7, 5
* `ADR-0011` §3
* `ADR-0013` Appendix C.4 and C.6
* `SAFETY.md` §§12, 13, 14
* `CLAUDE.md` §§6, 9, 11, 14, 39
* `ARCHITECTURE.md` §§20, 27
* `RESEARCH_LOG.md` conclusion 6
* `src/main.rs` in full; `src/partition.rs` `parse_mbr`;
  `src/filesystem.rs` `identify`; `src/fat_recovery.rs` `Assessment` and
  `Ineligible`; `src/validation.rs` `validate`
* `scripts/generate-fixtures.sh`, the builders of every fixture named below
* `tests/cli_arguments.rs`; `tests/fat32_directory_fixtures.rs`
  `the_residue_fixture_ends_early_and_reports_what_follows`

External sources consulted:

* NIST CFTT, *Active File Identification & Deleted File Recovery Tool
  Specification*, Draft 1 of Version 1.1, §§4, 5.1, 6.1
* NIST CFTT, *Digital Data Acquisition Tool Specification*, Draft 1 for
  Public Review of Version 4.0, §6.1
* NIST CFTT, *Disk Imaging Tool Specification*, Version 3.1.6
* CIPA DC-009, *Design rule for Camera File system*, DCF 2.0, 2010 edition
* GNU diffutils manual, *Invoking diff* and *Invoking cmp*
* rsync exit values, and the rsync mailing list and Back In Time issue
  tracker on exit value 23
* `fsck(8)`
* Monitoring Plugins development guidelines and `Monitoring::Plugin::Functions`
* *The Rust Programming Language*, §12.3, and `rust-lang/book` issues 2145
  and 3606

---

### B.2 Subdirectories are a coverage gap, and four fixtures carry one

`report_root_directory` lists a directory entry and does not read the
directory it names. §A.6's rule is that a coverage statement reports what
the run did not analyse. The contents of a listed directory were not
analysed, so a listed directory is a gap, whether live or deleted, and
whether or not it holds anything: from the evidence the tool cannot know.

Measured over every fixture with the binary built at `51d4ae0`:

```text
for f in fixtures/partition/*.img; do o=$(./target/debug/taphonomy "$f" 2>&1); printf '%s dirs=%s unread=%s\n' "$(basename "$f")" "$(grep -cE 'directory( {2,}|, cluster)' <<<"$o")" "$(grep -ciE 'subdirector|not entered' <<<"$o")"; done
```

| Image | Directory listed and not read | Created by |
| --- | --- | --- |
| `fat32-root-entries` | live `/logs` | `mmd` in its fixture function |
| `fat32-deleted-entries` | deleted `/gone` | `build_deleted_volume` |
| `fat32-recover-run` | deleted `/gone` | `build_recovery_volume` |
| `fat32-recover-collision` | deleted `/gone` | `build_recovery_volume` |

Every other image returned `dirs=0`, and all nineteen returned `unread=0`:
no output line anywhere states that a directory was not read.

`fat32-deleted-residue` is built by `build_deleted_volume` and returned
`dirs=0`. It was predicted to return 1, and the prediction was wrong. The
fixture's one poked byte lands in slot 5, which held `/gone`, and makes it
the terminator. The test that asserts that arrangement records that the
terminator branch consumes the slot before the residue branch sees it. That
image lists no directory, and nothing is uncovered by this kind.

**§A.1's "ten are analysed end to end" is therefore six:** `mbr-empty` aside,
they are `mbr-single-fat32`, `mbr-type-mismatch`, `fat32-hidden-mismatch`,
`fat32-root-multicluster`, `fat32-deleted-residue` and
`fat32-fragmented-deleted`. §A.1 measured whole-image and whole-volume gaps
and did not test for this one. The claim went further than the measurement.

**External requirement.** The CFTT deleted file recovery specification's
first core requirement, DFR-CR-01, reads: "The tool shall identify all
deleted File System-Object entries accessible in residual metadata." Its §4
gives files and directories as the most common File System-Objects, and
its definition of residual metadata records that a deleted directory's
first data block usually remains accessible. Deleted entries inside `/logs`
or `/gone` fall within the requirement, and this tool does not identify
them. That is a non-conformance, recorded here beside those in `ADR-0013`
Appendix A, and not resolved by this milestone.

**Why it matters beyond the fixtures.** CIPA DC-009 defines the `DCIM`
directory directly under the root as the DCF image root directory, and
places the directories that hold image files directly under it. On a
DCF-conformant camera card the root directory this tool reads holds no
image file. Without this gap kind the run would report such a card as
completely covered.

`Assessment::Ineligible(Ineligible::Directory)` therefore stops being one
of §A.7's refusals. A deleted directory is counted once, as this kind.

---

### B.3 Kind 1 includes an unreadable first sector, reachable without a fixture

Where sector 0 cannot be read, `inspect` prints the error to `stderr` and
never calls `parse_mbr`. §A.7's kind 1 says the table "could not be
parsed"; here it was never read.
Measured with an empty file outside the working tree:

```text
: > /tmp/taphonomy-empty.img && ./target/debug/taphonomy /tmp/taphonomy-empty.img 2>/tmp/taphonomy-empty.err; echo "exit=$?"; echo "--- stderr"; cat /tmp/taphonomy-empty.err
path         /tmp/taphonomy-empty.img
size         0 bytes
read         0 bytes
sha256       e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855

exit=0
--- stderr
error: i/o failure reading /tmp/taphonomy-empty.img: failed to fill whole buffer
```

Nothing on `stdout` indicates that nothing past the hash was analysed, and
the process exits zero. It is §A.4's measured harm on a second path. Kind 1
covers both a table that could not be read and one that could not be
parsed.

---

### B.4 Which kinds are reachable, and by what

§A.7 records kinds 6 and 7 as unexercised and states that testing them
requires fixtures that do not exist. At `51d4ae0` the position is:

| Kind | Detected in | Reached by |
| --- | --- | --- |
| 1, table not parsed | `inspect` | `bad-signature`, `no-signature`, `partition-beyond-end` |
| 1, sector 0 not read | `inspect` | no fixture; an empty file, §B.3 |
| 2, GPT | `inspect` | `gpt-protective` |
| 3, partition first sector not read | `report_partition` | nothing that is a static regular file |
| 4, filesystem not identified | `report_partition` | `mbr-four-partitions` |
| 4, identified, not analysed | `report_partition` | no fixture |
| 5, boot sector rejected | `report_partition` | `fat32-bad-root-cluster`, `fat32-oversized-volume`, `fat32-undersized-fat` |
| 6, root directory not enumerated | `report_root_directory` | no fixture |
| 7, assessment or extraction failed | `report_recovery` | no fixture |
| 8, directory listed and not read | `report_root_directory` | four fixtures, §B.2 |

**Kind 3 is not unexercised but unreachable from any fixture.** `parse_mbr`
rejects a partition of zero sectors and one that ends past the evidence,
and `image_sectors` is computed from the bytes actually read. `VBR_SIZE` is
one sector. Every accepted partition's first sector therefore lies inside
what was read, and the read fails only on an I/O error or on evidence that
changes during the run.

**Kind 4's second half is unexercised.** `identify` recognises FAT12,
FAT16, exFAT and NTFS, and every `mkfs.vfat` call in the generator is
`-F 32`.

**The first half of kind 1 needs no fixture**, as §B.3 measured.

Kinds 6, 7 and the second half of 4 have no fixture, and kind 3 cannot
have one, so their tests go on the derivation in §B.5.

---

### B.5 The status has three values, and §A.8's derivation is replaced

§A.8 derives success where "the count of uncovered partitions and entries
is zero". A rejected partition table and a GPT disk have no partitions
parsed, so both counts are zero and the rule reports success, which is what
`mbr-empty` reports. §A.6 names `mbr-empty` as the control that must keep
those apart.

It also cannot distinguish a run that analysed nothing from one that
analysed four volumes of five. `SAFETY.md` §12, which is constraining,
closes with: "A failure must never be silently converted into a partial
success." A two-valued status reports both runs as the same gap.
`CLAUDE.md` §14 separately requires that expected absence, malformed data,
unsupported format and I/O failure be differentiated, which is what an
enumeration by kind does and a single count does not.

**The derivation.** A volume is analysed where its root directory was
enumerated.

| Status | Gaps of any kind | Volumes analysed |
| --- | --- | --- |
| complete | none | any, including none |
| incomplete | at least one | at least one |
| none | at least one | none |

Applied to the measurements of §A.1, §B.2 and §B.3:

| Status | Images |
| --- | --- |
| complete | `mbr-empty`, `mbr-single-fat32`, `mbr-type-mismatch`, `fat32-hidden-mismatch`, `fat32-root-multicluster`, `fat32-deleted-residue`, `fat32-fragmented-deleted` |
| incomplete | `fat32-root-entries`, `fat32-deleted-entries`, `fat32-recover-run`, `fat32-recover-collision` |
| none | `bad-signature`, `no-signature`, `partition-beyond-end`, `gpt-protective`, `fat32-bad-root-cluster`, `fat32-oversized-volume`, `fat32-undersized-fat`, `mbr-four-partitions`, and an empty file |

Seven, four and eight, so every value varies on fixtures that exist. The
three boot-sector-rejected images each declare exactly one partition in
their generator functions, which is what places them under `none`.

This table is derived from measurements and has not itself been measured,
because no status is printed yet. Step 5's tests assert it.

**One limit.** No fixture mixes an analysed volume with one that was not.
`incomplete` is reached on fixtures only through kind 8. The mixed case is
tested on the derivation.

**External practice.** The Monitoring Plugins convention separates a check
that found a problem from a check that could not run, and the Perl library
that implements it records that "If no test were performed successfully
the state will still be UNKNOWN." `fsck(8)` keeps errors left uncorrected
and an operational error as separate conditions. Both correspond to the
separation between `incomplete` and `none`.

---

### B.6 The word is `coverage`, not `SUCCESS`

§A.8 has the run report success. On `fat32-fragmented-deleted`, whose
status is `complete`, three of five recoveries are not their own file's.
Beside those lines the word `SUCCESS` reads as a statement about recovery,
and `CLAUDE.md` §6 lists a claim to have successfully recovered data among
those that need evidence. The status therefore names
what it measures: `coverage complete`, `coverage incomplete`,
`coverage none`. The exact line format is step 3's.

**§A.8's precedent, checked at source.** "Qualified" is from the 2001 Disk
Imaging Tool Specification, Version 3.1.6, whose test cases give expected
results such as "src compares qualified equal to dst". Version 4.0 of 2004,
the Digital Data Acquisition Tool Specification, does not use the word: its
DI-RM-07 requires that the tool "notify the user of the error type and the
error location". §A.8's attribution is correct for the superseded
specification. The current form is the enumeration without a summary word,
and here the word stays because the operator needs a single line to find on
`stdout`, as §A.4 measured.

**Recorded as a departure, not a breach.** `SAFETY.md` §13 illustrates its
requirement with `Status: SUCCESS` and `Status: PARTIAL`, and `ADR-0003` §5
and `ADR-0013` §3 use `SUCCESS` as an example. §7 and §A.4 record that
§13 defines no vocabulary. No code or test depends on either string.

---

### B.7 Decision D contradicts Decision A

§8 states: "Under Decision E `STRUCTURALLY_VALID` is unreachable, so every
artifact is below it and the distribution is always stated."

§5 resolved the conflict between `ADR-0003` §4.2 and §4.5 in favour of
§4.5, under which `RECONSTRUCTED` is not a position in the ordering. It
cannot then be below `STRUCTURALLY_VALID`. The conclusion survives on a
different premise: §4.7's first sentence requires counts per level
unconditionally, so the distribution is stated because every session must
state it, not because every artifact is below anything.

---

### B.8 What the aggregate contains

**One line for artifacts.** The level is constant under Decision A, but the
count of artifacts is not: it runs from zero to the number of extractions.
§A.10 keeps the word off per-entry lines, and nothing retires the count.
§4.7's first sentence is unconditional, and one line satisfies it.
`Confidence` has one variant, so no structure keyed by level is built.

**One line for comparison.** `validate` returns `NotAttempted` exactly when
no reference is supplied, and Decision I compares one reference against
every extraction. Within one run the outcome counts therefore have two
shapes: every artifact not compared, or none. The line states whichever
holds, and never repeats the artifact count as a third figure.

**Deleted entries that produced no artifact, counted by `Assessment`
variant.** `ADR-0013` Appendix C.4 asked for this so that an operator
looking for a file knows the question went unanswered for part of the
volume, and it measures the shortfall against DFR-CR-02, which requires a
Recovered Object for each deleted entry. A split into refusals and errors
misses two cases:

* a free run not read because `--recover` was not passed, which is every
  free run in a default invocation and is neither a refusal nor an error;
* `Ineligible`, which is mostly not a refusal of a file: a long-name
  component, a volume label and an invalid entry are not files, while an
  entry with a size of zero, a reserved first cluster or a run out of range
  is.

The counts follow the variants: refused because a cluster is in use; free
and not read; not assessed, and free with extraction failed, both of which
are also kind 7; ineligible, by reason, with the non-file reasons not
counted as files that went unrecovered. A deleted
directory is counted under kind 8 only.

**Coverage.** Volumes analysed, and gaps by kind, from which §B.5 derives
the status.

---

### B.9 Where it lives: `src/main.rs`, beside `Caveats`

Placement in the library was proposed and examined first, and rejected on
the evidence.

* `ARCHITECTURE.md` §20 lists status and confidence in an operation's
  structured result, and §27 has future interfaces call the layer the CLI
  calls. `ADR-0011` §3 classes `ARCHITECTURE.md` as intent: "It binds
  nothing until corresponding code exists." It is the project's direction,
  not a requirement.
* `CLAUDE.md` §11, which is constraining, states: "The CLI must not contain
  recovery algorithms." A count of what was covered and a status derived
  from it are reporting, not recovery.
* `CLAUDE.md` §9, which is also constraining, states: "Do not create
  abstractions without a demonstrated requirement." No consumer other than
  the CLI exists.
* *The Rust Programming Language* §12.3 recommends moving a binary's logic
  into `lib.rs`, partly because `main` cannot be tested directly. Issues
  2145 and 3606 on its own tracker record that other functions in a binary
  crate can be unit tested, and the workspace already runs the binary's
  harness: `cargo test --workspace` prints a `running 0 tests` line for it.
  The testing argument for the library therefore does not hold.

The derivation is written as a function taking the counts and returning the
status, so it is unit tested in `src/main.rs` without opening a fixture.

**Review trigger.** A second consumer of the counts, such as a structured
output mode or another interface, moves the counts and the derivation into
the library.

**`Caveats` is computed from the counts, not merged alongside them.** Each
of its three booleans is a projection of a count: a free run, a match, a
difference. Two values carried separately along the same path can disagree,
which is §A.8's own reason for deriving the status rather than tracking it.
`ADR-0013` Appendix C.6's distinction is not breached: it concerns what the
paragraphs say, and none counts or aggregates findings.

---

### B.10 Exit status: `none` exits 3; everything else is unchanged

No record decides the exit status of a run with a coverage gap. The only
source for the current behaviour is the documentation comments on
`report_partition` and `report_root_directory`, which state that a failure
there does not change it.

**Decision.** A run whose coverage is `none` exits 3. A run whose coverage
is `complete` or `incomplete` exits 0, as now. Exit 1 for evidence that
could not be opened or hashed, exit 2 for an argument error, and exit 0 for
a differing digest under `ADR-0013` §3 are unchanged. 3 is used because 1
and 2 already carry meanings in `main`.

**Why `none`.** §A.4 and §B.3 measured runs that analysed nothing past the
hash and exited zero, and `SAFETY.md` §12 forbids converting a failure
silently. `CLAUDE.md` §39's preference for explicit failure over a best
guess points the same way, though it is written about source and
destination ambiguity and is not relied on here.

**Why not `incomplete`.** Two considerations, one from the test suite and
one external, decide it.

* The primary recovery fixture is `incomplete`. `tests/cli_arguments.rs`'s
  three tests that assert exit 0 all use `fat32-recover-run`, which carries
  a deleted `/gone`.
* Under §B.2's CIPA DC-009 finding, every DCF-conformant camera card is
  `incomplete` while subdirectories are not read. A non-zero code on that
  state would be the tool's ordinary result. rsync's exit value 23 shows
  what follows: its users describe it on the rsync mailing list as meaning
  anything without its own code, and report ignoring it, and Back In Time's
  tracker proposes treating it as a warning. The gap is stated on `stdout`
  instead, where §A.4 locates the harm.

GNU diffutils likewise does not count a binary comparison as trouble even
though its output does not capture every difference.

**Considered and not acted on.** GNU `diff` and `cmp` exit 0 where inputs
are the same, 1 where they differ, and 2 on trouble. That is the closest
convention to a comparison, and it disagrees with `ADR-0013` §3's exit 0 on
a differing digest. `ADR-0013` decided that on its own grounds and this
milestone does not reopen it.

This is a contract change and is its own commit, after the aggregate.

---

### B.11 Implementation order, replacing §A.9's merged step and those after it

**Step 3.** Coverage counts including kind 8 and both halves of kind 1, the
three-valued status derived from them, the aggregate block of §B.8 before
the caveats, and `Caveats` computed from the counts. One commit, per §A.9.

**Step 4.** Exit status 3 for `none`. One commit.

**Step 5.** Tests. The derivation, unit tested for all three values, the
mixed-volume case, and kinds 3, 6, 7 and the second half of 4. The binary:
an empty file exits 3 and reports `coverage none`; `mbr-empty` reports
`coverage complete`; `fat32-recover-run` reports `coverage incomplete` and
exits 0; one fixture from §A.1's rejected tables exits 3. And Decision B: no
entry without an artifact carries a level.

**Step 6.** `ADR-0003` appendix recording which levels are unreachable;
`README.md`, `docs/development/CHANGELOG.md`, `docs/PROJECT.md` §6.9 and
`src/lib.rs`'s crate documentation; the documentation comments in
`src/main.rs` that state a failure does not change the exit status.

---

### B.12 Parked, and not this milestone's

* A terminator slot's surviving bytes. `fat32-deleted-residue`'s slot 5
  still holds the remaining 31 bytes of the entry that was `/gone`, and
  they are neither listed nor counted. Reporting them changes what an entry
  reports, which §10 keeps out of M9.
* A fixture holding two volumes of which one is analysed and one is not.
* Reading subdirectories, which would close kind 8 and the DFR-CR-01
  non-conformance of §B.2.

---

### B.13 What this appendix does not change

Decisions A, B and E stand as written. Decision C's axis and §A.4's warrant
for it stand. §A.5 and §A.6 stand, and §B.2 applies §A.6. §A.10 stands: the
level appears in the aggregate only. §11's review triggers are unaffected.

Replaced: §A.1's count of images analysed end to end; §A.7's list, extended
by kind 8 and the first half of kind 1, and its statement of which kinds are
unexercised; §A.7's classing of a deleted directory as a refusal; §A.8's
two-valued derivation and its word; §8's premise that every artifact is
below `STRUCTURALLY_VALID`; and §12's steps from 3 onward.

No decision here concerns what the tool recovers, refuses or validates.

---

## Appendix C: B.2's gap closed, and the directory B.2 could not see (2026-09-24)

The body and Appendices A and B are not rewritten.

### C.1 B.2's gap no longer arises

B.2 made every listed directory a coverage gap, because the tool did not
read the directories it listed. `ADR-0016` reads every directory the root
reaches, a live one along its chain and a deleted one from its first
cluster. §11 condition 8 there required the two tests B.2's gap made
necessary to be replaced by tests of the gaps that remain; neither test
exists at `3ff77f7`. A listed directory is now a gap only where one of
`ADR-0016`'s own kinds applies. Appendix B.2's measurement stands as a
record of the tool at `51d4ae0`.

### C.2 `fat32-deleted-residue` holds a directory after all

B.2 measured that image as listing no directory, and concluded that
nothing on it was uncovered by the kind. That was true of what the tool
could see: the fixture's poke makes `/gone`'s root slot the terminator, so
no entry names it. `/gone`'s own cluster survived, and `ADR-0017`'s search
finds it, at cluster 5, empty. `tests/orphaned_directories.rs` asserts it.
B.2's statement was about the listing, and the listing was right; the
directory was outside the tool's reach until M12.

---

## Appendix D: a partition entry can be a gap without rejecting the table (2026-10-01)

`A.7`'s taxonomy of gaps, as later extended, did not have a category for
a partition table entry that is individually malformed — zero length,
or claiming sectors beyond the evidence — while the table itself is
well-formed. Before this appendix, `ZeroLengthPartition` and
`PartitionBeyondEnd` were parse errors: either one caused the whole
table to be refused, and every partition on the image, however
well-formed, went unanalysed.

`RunCounts` gains `partitions_anomalous`: a partition entry rejected as
malformed and not analysed, while the table's other entries were. It is
counted in `gaps()` alongside `table_rejected` and the rest.

This narrows `A.5`, it does not contradict it. `A.5`'s examples — a
declared-type mismatch, a hidden-sectors disagreement — are findings
about evidence that *was* analysed; the partition itself was read, and
the anomaly is content the run observed. A dropped partition entry is
the opposite case: its extent is never read at all, which is exactly
what `A.7` already defines a gap to be. The two kinds of anomaly differ
on the one fact that decides status under `A.5`'s reasoning — whether
the evidence behind the finding was covered — so both stand.

`ADR-0005` §3.3 already established the principle this corrects for:
"conditions worth reporting rather than silently accepting," not causes
for refusing the whole table. `ZeroLengthPartition` and
`PartitionBeyondEnd` are now `Anomaly` variants, not `ParseError`
variants; the malformed entry is dropped from the returned partition
list and not analysed, every other entry is parsed and analysed
normally, and the anomaly is reported.

Two behaviours follow, both measured:

* A table whose only entry is malformed (fixture
  `partition-beyond-end.img`) still reports nothing analysed and exits
  3 — the same outcome as before this change, now reached through
  `partitions_anomalous` rather than a table-level refusal.
* A table with some malformed entries beside valid ones (fixture
  `mbr-truncated-last-partition.img`, modelling a disk image that was
  truncated mid-acquisition) now has its valid partitions listed and
  analysed, the malformed one reported as an anomaly and a gap, and the
  run exits 0 with partial rather than complete coverage.

`table_rejected` is unchanged and still covers what makes a table
itself invalid: a missing signature. (`LengthOverflow` is a third
`ParseError` variant, but `start_lba` and `sector_count` are both
`u32`; their sum as `u64` cannot overflow, so no input reaches it. That
appears to be unreachable at this call site, independent of this fix —
worth a separate look, not corrected here.)

---

## Appendix E: exFAT adds three gap kinds and widens four (2026-10-20)

M14 analyses exFAT volumes (`ADR-0021`). §A.7's taxonomy, as later
extended, named FAT32 structures. This appendix records how exFAT's
findings are counted. Every count below is a gap under §B.5, so a run
that has any of them is `incomplete`, or `none` if no volume was
analysed.

**Three new kinds**, each counted once per occurrence:

* `exfat_sets_refused`, printed "entry sets not used": an entry set, or
  a single entry, that was not used because its SetChecksum was wrong,
  it was malformed, or it was not recognised (`ADR-0019` Decision C,
  `ADR-0021` section 4). Its content may be real and was not analysed.
* `exfat_deleted_sets_not_examined`, printed "deleted sets not
  examined": an unused File entry, the start of a deleted entry set.
  M14 does not examine deleted sets; M15 does. Counting them is what
  keeps a volume with deleted files from being reported complete
  (`SAFETY.md` §12).
* `exfat_end_marker_residue`, printed "entries after end marker": a
  directory holding entries of a type other than `00h` after its end
  marker, which the specification says cannot occur. Counted once per
  directory; the entries are not interpreted.

These are new kinds and not cases of the old ones. They describe content
inside a directory that was read, where the old kinds describe a
partition, volume or directory that was not.

**Four existing kinds now include exFAT:**

* Kind 4, identified and not analysed: TexFAT, an exFAT volume with two
  FATs, is counted here (`unsupported`).
* Kind 5, boot sector rejected: an exFAT volume whose main and backup
  boot regions were both refused, or whose backup search found two
  valid regions, is counted here (`boot_rejected`).
* Kind 6, root not enumerated: an exFAT root that could not be read,
  including one holding an unrecognised critical primary entry, which
  makes the volume invalid (`root_unread`).
* Kind 8, directory named and not read: an exFAT directory that could
  not be read, or was read only in part, for any reason
  `ExFatDirectoryError` names, including the allocation faults of
  `ADR-0021` Decision L; a directory deeper than the depth bound; and
  the directories left when a safety bound is reached
  (`directories_unread`).

**Reached by.** EXP-0009's images A and B reach
`exfat_deleted_sets_not_examined` (122 and 125) and nothing else, so
both report `incomplete`, measured 2026-10-20. The other two new kinds,
and the exFAT cases of kinds 4, 5, 6 and 8, are reached by unit tests
on volumes built in memory, not by fixtures. A copy of image A with its
first sector zeroed reaches the backup boot region and reports the same
as A.

**One limit.** When a partition of type `0x07` whose first sector
identifies nothing is probed for an exFAT backup region and a read
fails, the partition is counted under kind 4 as unidentified, not under
kind 3 as unread. `ADR-0021` section 3 records it as a known limitation.

§11's trigger, a deleted file's extent established from surviving
evidence, is not met by M14, which recovers nothing. M15 meets it, and
`ADR-0022` takes it up.
