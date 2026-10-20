# ADR-0003: Recovery Confidence Model

* **Status:** Accepted
* **Date:** 2026-08-29
* **Decision owners:** Taphonomy project
* **Scope:** Classification of recovery results and the pipeline stage that assigns it
* **Supersedes:** `SAFETY.md` §14 and `PROJECT.md` §6.9 as normative sources
* **Reconciles:** `ARCHITECTURE.md` §36

---

## 1. Context

`DOCUMENTATION_AUDIT.md` §4.2 identified three incompatible models for
expressing recovery certainty.

| Source | Model |
|---|---|
| `SAFETY.md` §14 | `VERIFIED`, `HIGH_CONFIDENCE`, `PARTIAL`, `RECONSTRUCTED`, `UNVERIFIED`, `UNRECOVERABLE` |
| `PROJECT.md` §6.9 | verified recovery, probable recovery, partial recovery, reconstructed data, unverified extraction, unrecoverable evidence |
| `ARCHITECTURE.md` §36 | `Candidate` vs `Verified Artifact` |

The first two are the same taxonomy under different names. The third is not a
taxonomy at all.

`SAFETY.md` §14 states that the exact confidence model will be defined
separately before implementation. This ADR is that definition.

---

## 2. Root Cause

Two orthogonal concepts were being expressed on one axis.

**Pipeline state** answers: has this thing been through the validator yet?
This is what `ARCHITECTURE.md` §36 describes.

**Evidence strength** answers: how strong is the claim the evidence supports?
This is what `SAFETY.md` §14 and `PROJECT.md` §6.9 describe.

A result cannot have an evidence-strength classification before validation,
because the classification is the validator's output. Treating `Candidate` as
a peer of `VERIFIED` conflates a stage with a verdict.

---

## 3. Decision

### 3.1 Two-stage pipeline

```text
Recovery algorithm
        ↓
   Candidate            (no confidence level; not yet assessed)
        ↓
    Validator
        ↓
    Artifact            (carries exactly one confidence level)
```

A Candidate is a proposition. An Artifact is a proposition plus a verdict.

A Candidate must never be presented to a user as a recovered file, written to
the recovery output directory, or counted in a recovery result summary.

`ARCHITECTURE.md` §36's distinction is retained in full and is understood as
this pipeline boundary, not as a confidence classification.

### 3.2 Confidence taxonomy

Exactly one of the following is assigned to every Artifact.

| Level | Definition | Evidence required |
|---|---|---|
| `VERIFIED` | Recovered content is byte-identical to a known-good reference | Cryptographic hash match against a reference artifact |
| `STRUCTURALLY_VALID` | Content parses completely and consistently as its claimed format; no reference exists for comparison | Format-aware validator completed without error over the entire artifact |
| `PARTIAL` | Content is incomplete, and the portion recovered is structurally sound | Validator succeeded over a bounded prefix or region; the missing extent is known and recorded |
| `RECONSTRUCTED` | Content was assembled using inference or heuristics rather than surviving filesystem metadata | Reconstruction method and its inputs recorded; validator result recorded separately |
| `UNVERIFIED` | Bytes were extracted; no validation was possible | Extraction succeeded; no validator exists for the format, or validation could not run |
| `UNRECOVERABLE` | Evidence is insufficient to attempt recovery | Basis for the determination recorded |

### 3.3 Renaming of HIGH_CONFIDENCE

`SAFETY.md` §14's `HIGH_CONFIDENCE` is renamed `STRUCTURALLY_VALID`.

`SAFETY.md` §14 itself requires that confidence be based on measurable
evidence rather than subjective language. "High confidence" is subjective
language and states a degree of belief rather than an observation.
"Structurally valid" states what was measured.

`PROJECT.md` §6.9's "probable recovery" maps to the same level and is
likewise replaced.

---

## 4. Rules

### 4.1 Single level

An Artifact carries exactly one confidence level. Levels are not combined,
averaged, or expressed as ranges or percentages.

### 4.2 No upgrade without evidence

A level may only be raised by evidence that satisfies the higher level's
requirement. It may never be raised by absence of contrary evidence.

### 4.3 Downgrade is always permitted

Any level may be lowered when evidence is found to be weaker than assessed.

### 4.4 Default on uncertainty

When a validator cannot determine which of two levels applies, the lower
applies. This follows the fail-closed requirement of `SAFETY.md` §12.

### 4.5 RECONSTRUCTED is not a strength ordering

`RECONSTRUCTED` describes the method used, not a position between `PARTIAL`
and `UNVERIFIED`. A reconstructed artifact records its validator result
separately from its classification.

### 4.6 Hash semantics

`VERIFIED` establishes byte equality with a reference. It does not establish
that the artifact is the file the user intended to recover. This restates
`SAFETY.md` §10 and must not be weakened in user-facing output.

### 4.7 Aggregate reporting

A recovery session reports counts per level. It must not report a single
combined success figure, and must not describe a session as successful when
any artifact is below `STRUCTURALLY_VALID` without stating the distribution.

---

## 5. Relationship to Operation Status

Confidence classifies an artifact. `SAFETY.md` §13's `SUCCESS` / `PARTIAL` /
failure statuses classify an operation. These are separate axes and must not
be merged.

An operation may complete with status `SUCCESS` while producing artifacts
classified `UNVERIFIED`.

---

## 6. Consequences

### Positive

* One normative taxonomy across all documents
* `ARCHITECTURE.md` §36 is preserved rather than discarded
* Every level is defined by a measurable condition
* Fail-closed default is explicit

### Negative

* `SAFETY.md` §14 and `PROJECT.md` §6.9 require amendment to reference this ADR
* `STRUCTURALLY_VALID` requires a format-aware validator per supported format,
  which is more work than a magic-byte check
* Formats with no validator yield `UNVERIFIED`, which may appear pessimistic

---

## 7. Required Amendments

| Document | Change |
|---|---|
| `SAFETY.md` §14 | Replace the candidate list with a reference to this ADR |
| `PROJECT.md` §6.9 | Replace the six-term list with a reference to this ADR |
| `ARCHITECTURE.md` §36 | Add a note that Candidate/Artifact is the pipeline boundary defined here |

These amendments are documentation-only and carry no implementation
dependency.

---

## 8. Implementation Note

No part of this model is implemented. The taxonomy becomes binding on the
first code that assigns a confidence level, which under ADR-0002 §8 is
milestone M9.

Implementing the enum earlier than M9 would be speculative. The definition
exists now so that earlier milestones do not encode a conflicting model by
accident.

---

## Appendix A: Implementation Note Corrected, and §2 Read (2026-09-09)

`ADR-0012` and `ADR-0010` set the pattern this follows: dated records are
corrected by appendix and never rewritten. Two corrections and one reading,
recorded on completion of M8 under `ADR-0013`.

### A.1 §8's first sentence is no longer true

§8 states that no part of this model is implemented. That was true when it
was written. It was already strained by M7, which delivered §3.1's pipeline
boundary in code — `extract` returns bytes and a digest and classifies
nothing, so what it produces is a Candidate in this model's sense, and
`ADR-0010` §3.1 recorded that reading.

M8 falsifies it. `src/validation.rs` is the Validator stage of §3.1: it
takes a recovered digest and a reference and returns what the comparison
established.

What remains unimplemented is the taxonomy and the rules built on it. §8's
second and third sentences are unaffected and still govern: the taxonomy
becomes binding on the first code that assigns a confidence level, which
under `ADR-0002` §8 is M9, and implementing the enum earlier would be
speculative.

The state of this model at M8:

| Section | Subject | State at M8 |
| --- | --- | --- |
| §3.1 | Two-stage pipeline | Implemented. `fat_recovery` produces Candidates, `validation` is the Validator. |
| §3.2 | Six-level taxonomy | Not implemented. No level name appears in `src/` or `tests/`. |
| §3.3 | Renaming of `HIGH_CONFIDENCE` | Not reached; nothing assigns a level to rename. |
| §4.1 | Single level | Not reached. |
| §4.2 | No upgrade without evidence | Applied as a principle, not as code. `ADR-0013` §8.2 applies its mirror to a digest match on non-distinctive content. |
| §4.3 | Downgrade always permitted | Not reached. |
| §4.4 | Default on uncertainty | Applied. A missing reference yields `NotAttempted`, which is never read as a pass. |
| §4.5 | `RECONSTRUCTED` is not an ordering | Not reached. |
| §4.6 | Hash semantics | Applied. `ADR-0013` Decision D and §8 bound what a match establishes. |
| §4.7 | Aggregate reporting | Not implemented, and deliberately so. `ADR-0013` Decision I keeps M8 to per-entry reporting. |
| §5 | Relationship to operation status | Applied. `ADR-0013` §3 relies on it to make a differing digest a finding rather than a failed operation. |

An Artifact, in §3.1's sense, still does not exist. M8 produces the evidence
a verdict would be computed from and stops there.

### A.2 §2's last paragraph, read

§2 states that a result cannot have an evidence-strength classification
before validation, because the classification is the validator's output.

Read literally alongside §8, the two cannot both hold once a validator
exists: if the classification is the validator's output and M8 builds the
validator, then M8 assigns classifications, and §8's reservation of the
taxonomy for M9 is broken by the milestone that precedes it.

The reading this project takes, recorded here so M9 does not inherit the
ambiguity: **the validator produces the evidence a classification is
computed from, not the name.** §2's purpose is to establish an ordering —
nothing may be classified before it has been validated — and that ordering
is what `SECURITY.md` §31 states independently, that validation is required
before a result is classified as verified. The sentence is about sequence,
not about which component owns the enum.

`ADR-0013` section 7 records the same reading from the other side and is the
decision this appendix supports.

### A.3 What this appendix does not change

No decision in this ADR is amended. The taxonomy, its rules, and the
milestone at which it becomes binding are unchanged. §7's required
amendments are unaffected.

---

## Appendix B: the model implemented, and the subset M9 leaves reachable (2026-09-19)

`ADR-0014` Decision E names the writing of this appendix as one of its
actions, so that this ADR's taxonomy and the subset of it this tool can
produce are reconcilable by a reader holding only the ADR series.

Recorded on completion of M9's implementation, at `4fb13eb`. Every state
below was checked against the tree rather than against `ADR-0014`.

### B.1 §8 is now spent

Appendix A.1 corrected §8's first sentence. Its second and third are now
discharged rather than corrected: `src/confidence.rs` assigns a level, so
the taxonomy is binding from this milestone, exactly as §8 reserved it.

| Section | Subject | State at M9 |
| --- | --- | --- |
| §3.1 | Two-stage pipeline | Implemented, unchanged from Appendix A.1. |
| §3.2 | Six-level taxonomy | One variant implemented. The other five are recorded and not constructed: `ADR-0014` Decision E. |
| §3.3 | Renaming of `HIGH_CONFIDENCE` | Not reached. Nothing is classified `STRUCTURALLY_VALID`. |
| §4.1 | Single level | Satisfied. An artifact carries one level, and the run reports it once. |
| §4.2 | No upgrade without evidence | Applied, and never exercised: a match does not promote. B.2 below. |
| §4.3 | Downgrade always permitted | Not reached. One level exists, so there is nothing to lower to. |
| §4.4 | Default on uncertainty | Applied. A missing reference yields `NotAttempted`, never read as a pass. |
| §4.5 | `RECONSTRUCTED` is not an ordering | Governs. It decides B.2. |
| §4.6 | Hash semantics | Applied. What a match establishes is bounded in the output itself. |
| §4.7 | Aggregate reporting | Implemented. B.3 below. |
| §5 | Relationship to operation status | Applied, with a third axis added. B.4 below. |

### B.2 One level is reachable, and it is not `VERIFIED`

Every artifact is `RECONSTRUCTED`. `ADR-0014` Decision A records the
reasoning and is not restated here beyond what a reader of this ADR needs:

§3.2 defines `VERIFIED` as byte-identity with a known-good reference, and M8
implements exactly that comparison, so §4.2 would permit the promotion. §4.5
forbids it, because `RECONSTRUCTED` names the method rather than a position
in an ordering, and a level cannot be raised out of a category this ADR
excludes from the ordering. §4.5 governs: every extraction this tool
performs infers its extent, since deletion zeroes the cluster chain, so a
match establishes that the inference was right for that entry and not that
the method was different.

EXP-0004 measured why that matters. On one volume, three of five deleted
entries recover content that is not their own file's, and the tool reports
all five identically. A level that varied would assert a discrimination the
tool measurably does not have.

§4.2 is not weakened by this. It remains binding as written for any level
that becomes reachable later.

What would make another level reachable: a format-aware validator reaches
`STRUCTURALLY_VALID`, and a bounded extraction reaches `PARTIAL`. Either
reopens `ADR-0014` Decision A, which records both as review triggers.

### B.3 §4.7 as implemented

The session statement is printed once per run, at indentation zero, after
the per-volume output.

* **Counts per level.** One line, `artifacts N RECONSTRUCTED`, printed where
  `--recover` was passed. One level is reachable, so one line carries the
  distribution; the count varies and the name does not.
* **No combined success figure.** The status word names coverage rather than
  success, so that a run which analysed everything it could reach does not
  read as a claim that its recoveries are right: `ADR-0014` Appendix B.6.
* **The distribution clause.** Under §4.5, `RECONSTRUCTED` is outside the
  ordering, so no artifact this tool produces is *below*
  `STRUCTURALLY_VALID` and that clause does not engage. The distribution is
  stated anyway, because §4.7's first sentence requires it unconditionally.
  `ADR-0014` Appendix B.7 corrects Decision D, which had reached the same
  conclusion from the premise that every artifact is below it.
* **What carries no level.** Entries that produced no artifact are counted
  on their own line by what stopped them, never with a level attached:
  `ADR-0014` Decision B.
* **The outcome axis stays separate.** Comparison counts print on their own
  line, as counts of what the reference established, not as a level.

Measured on `fat32-recover-run.img` with `--recover` and `BIG.TXT`'s digest:

```text
coverage     incomplete
  volumes analysed           1
  directories not read       1
artifacts    2 RECONSTRUCTED
compared     1 matched, 1 differed
```

### B.4 §5's two axes, and the third M9 adds

§5 keeps confidence, which classifies an artifact, apart from `SAFETY.md`
§13's status, which classifies an operation. M9 adds coverage, which
classifies neither: it states how much of the evidence the run analysed.

The reading this project takes, recorded so that a later milestone does not
merge them: coverage is §5's operation axis realised for this tool, named
for what it measures. It is reported on its own line, never combined with a
level or with a comparison, and `ADR-0014` Appendix B.5 derives it from what
the run did not analyse rather than from what it recovered.

The process exit status derives from coverage alone. A run that analysed
nothing past the evidence digest exits 3. A differing digest still exits
zero under `ADR-0013` §3, and cannot occur in a run whose coverage is
`none`: a comparison requires an artifact, an artifact requires a volume
analysed, and a volume analysed is what `none` excludes.

### B.5 §7's required amendments are discharged

Checked at `4fb13eb`: `SAFETY.md` §14 refers to this ADR as the single
definition, `PROJECT.md` §6.9 refers to it in place of its six-term list,
and `ARCHITECTURE.md` §36 carries the Candidate/Artifact note. None remains
outstanding.

### B.6 What this appendix does not change

No decision in this ADR is amended. The six definitions in §3.2 stand as
written, including the five no code path constructs. §§4.1 to 4.6 stand as
rules. A level that is not implemented is not retired, and §7 is unaffected.

---

## Appendix C: a second level reachable (2026-10-21)

M15 recovers deleted exFAT files, whose extent is read from surviving
metadata, and `ADR-0022` Decision A classifies each such artifact
`UNVERIFIED`. This appendix records the change to the subset Appendix B
recorded, so that a reader holding only the ADR series can reconcile
§3.2 with what the tool produces.

| Section | Subject | State at M15 |
| --- | --- | --- |
| §3.2 | Six-level taxonomy | Two variants implemented: `RECONSTRUCTED` for FAT32, `UNVERIFIED` for exFAT. The other four are recorded and not constructed. |
| §4.1 | Single level | Satisfied. Each artifact carries one level. |
| §4.2 | No upgrade without evidence | Applied. A digest match raises neither level: `ADR-0014` Decision A for FAT32, `ADR-0022` Decision A for exFAT. |
| §4.4 | Default on uncertainty | Applied, and extended to ordering: where two deleted sets cannot be ordered, both are refused (`ADR-0019` Appendix C). |
| §4.5 | `RECONSTRUCTED` is not an ordering | Governs FAT32 artifacts. exFAT artifacts are not `RECONSTRUCTED`, because their extent is not inferred. |
| §4.7 | Aggregate reporting | Implemented for two levels: the summary line lists each level with a count above zero. |

`UNVERIFIED` is assigned under the reading of its evidence column that
`ADR-0022` section 3 records: no format-aware validator exists. That
reading is confined to an extent read from metadata. Appendix B.2's
reasoning for FAT32 is unchanged.

**§4.7's distribution clause now engages.** Appendix B.3 found that the
clause, which forbids describing a session as successful where an
artifact is below `STRUCTURALLY_VALID` without stating the
distribution, did not engage, because `RECONSTRUCTED` lies outside the
ordering. `UNVERIFIED` lies inside it, below `STRUCTURALLY_VALID`, so a
run with an exFAT artifact meets the clause. It is satisfied as B.3
describes: the status word names coverage, not success, and the
`artifacts` line states the count of every level present.

No decision of this ADR is amended. The six definitions of §3.2 stand
as written, and §4.1 to §4.6 stand as rules.
