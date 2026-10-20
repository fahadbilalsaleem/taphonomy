# ADR-0019: exFAT, and What a Recovery From It May Claim

* **Status:** Accepted
* **Date:** 2026-09-26
* **Decision owners:** Taphonomy project
* **Scope:** Reading exFAT volumes, and recovering deleted files from them
* **Related:** `ADR-0002` §3.4, §3.5, §5.2; `ADR-0012`; `ADR-0018`;
  EXP-0009 and its Appendix A; Microsoft, *exFAT file system
  specification*, revision 1.00

---

## 1. Context

`ADR-0002` §5.2 made exFAT the correct second target, after FAT32's
concepts were established; §3.4 records that SD cards larger than 32 GB
use it. Taphonomy identifies exFAT today and does not analyse it.

EXP-0009 measured what Windows 10 leaves when it deletes from exFAT. The
entry set, the FAT chain and the data survive; only the InUse bits, the
bitmap and PercentInUse change. A deleted fragmented file can therefore be
followed by its own chain, which FAT32 never allows (`ADR-0002` §3.5).
The Sleuth Kit 4.15.0 does not do so.

EXP-0009 Appendix A then showed that intact metadata does not mean the
data is the file's. Of 118 deleted files whose metadata passed every
check, 16 held only zero bytes and 3 held another deleted file's data.

---

## 2. Decisions

* **A.** exFAT is the next filesystem, before FAT12, FAT16 and long names.
* **B.** A volume is analysed only from a boot region whose checksum is
  valid.
* **C.** An entry set is used only if its SetChecksum is valid; a deleted
  set, only with its InUse bits restored.
* **D.** A deleted file is recovered only when four conditions hold, and
  is otherwise refused with the condition it failed.
* **E.** A recovery states what was checked. It does not state that the
  data is the file's.
* **F.** Content past ValidDataLength is reported as zeros, never read.
* **G.** A volume is read, never mounted.

---

## 3. Reading the volume

The main boot region is used if its checksum (specification §3.4) is
valid; otherwise the backup region, if its checksum is valid, and the
report says so; otherwise the volume is reported and not analysed. A
volume with two FATs is TexFAT (§3.1.16) and is identified and not
analysed.

The root directory is always a FAT chain (§3.1.10). Other directories
follow their entry's NoFatChain flag (§6.3.4.2). A live entry set whose
SetChecksum (§6.3.3) is invalid is reported and not used. Names are read
from File Name entries; the up-case table is not needed, because nothing
is looked up by name.

---

## 4. Decision D: when a deleted file is recovered

A deleted entry set is a sequence of unused entries that, with the InUse
bit of each set again, forms a File entry set whose SetChecksum is valid.
Any other sequence is listed and not recovered.

Its allocation is read from its Stream Extension entry. With NoFatChain 1
it is one run of clusters, as many as DataLength needs. With NoFatChain 0
it is the chain from FirstCluster, which must stay within the cluster
heap, visit no cluster twice, end in `FFFFFFFFh` (§4.1.3), and be as many
clusters as DataLength needs.

It is recovered only if all of these hold:

1. every cluster of the allocation is free in the bitmap (§7.1.5);
2. no other deleted entry set that claims any of those clusters was
   created at the same time or later;
3. the data up to ValidDataLength is not zero bytes only;
4. the allocation is well formed, as above.

On EXP-0009's image B these conditions recover 99 files, each equal to
the file that was written, and refuse 19: 16 whose clusters hold only
zeros and 3 overwritten by a later file. Without conditions 2 and 3, the
same metadata would have recovered all 118.

Condition 2 uses creation time, including its 10 ms field. Equal times
refuse every file that shares them, because neither can be shown to be
the later.

---

## 5. Decision E: what a recovery claims

A recovered file is reported with the checks it passed: the entry set's
checksum, a free bitmap, a well-formed allocation, no later claim, and
content that is not only zeros. It is not reported as the file's data.
Appendix A shows why: data can be stale, or never written, while all of
its metadata is intact. Condition 3 detects only data that is zeros.

---

## 6. Acceptance

Before the exFAT code is merged:

1. On EXP-0009's image B, Taphonomy recovers exactly the 99 files
   Appendix A's last rule recovers, each equal by SHA-256 to the file
   written; refuses the 19 others with their conditions; refuses
   `FILL006.bin` for its allocated cluster; and lists the six
   `FILL233.bin` sets as holding no data.
2. `FRAG.bin` is among the 99, recovered by its chain.
3. On image A, every live file is listed and equal to the file written.
4. Every existing test passes, and output on every existing fixture is
   unchanged, compared by digest as in `ADR-0018` §5.

A public claim about exFAT recovery also needs NIST's exFAT
deleted-file-recovery images measured, as EXP-0008 did for FAT.

---

## 7. Not decided here

* **Fixtures.** `mtools` cannot write exFAT, so `ADR-0006`'s method does
  not carry over. How tests get exFAT volumes, and whether EXP-0009's
  images, 54 KB compressed, become test inputs, is decided before code.
* **Stale data other than zeros.** Condition 3 does not detect it.
  EXP-0010 measures a deleted fragmented file whose contiguous reading
  stays inside the volume.
* **Other deleting systems.** Cameras, phones, macOS and Linux were not
  measured.
* **Physical devices.** Reading a card directly, and what Windows writes
  when one is inserted, is phase 2 of EXP-0009.
* **Distribution in a paid product.** The engine is published under
  `ADR-0012`. Whether a commercial product may include exFAT support
  awaits a patent opinion; it does not affect the engine.

---

## Appendix A: how tests get exFAT volumes (2026-10-06)

§7 left open how tests get exFAT volumes and whether EXP-0009's images
become test inputs. They do. `exp9-A.vhd` and `exp9-B.vhd` are the
inputs for §6, and nothing else can stand in for image B. Every number
in §6's first item is a property of the volume as Windows wrote it,
including `IndexerVolumeGuid`, which Windows created in a free cluster
of the deleted `FILL006.bin` between A and B. No tool this project can
run in CI produces that.

Two alternatives were measured. `exfat-fuse` 1.4.0 wrote a four-run file
with NoFatChain 0, and on deletion zeroed all 50 of its FAT entries (0
unchanged), where EXP-0009 P6 found Windows leaves them. A volume
deleted through that driver cannot stand in for B. The Linux kernel
driver was not run; reading `fs/exfat/fatent.c`, freeing a chain clears
bitmap bits and writes no FAT entry, which is a reading and not a
measurement. `mkfs.exfat` 1.2.2 is not reproducible: two runs three
seconds apart differed in 262 bytes, all traced to `VolumeSerialNumber`
at offset 100 of the boot sector and the boot checksum it feeds, and the
tool has no flag to set it. A regenerable exFAT fixture set therefore
stays undecided and is not needed for §6.

The images are committed as `fixtures/exfat/exp9-A.vhd.xz` and
`exp9-B.vhd.xz`, expanded by `scripts/generate-fixtures.sh`, and pinned
in `MANIFEST.sha256` by the digests EXP-0009 recorded, so CI checks
that the committed bytes are the measured ones. They are the first
committed binary test inputs; the README's statement that every test
image is generated from ordinary filesystem tools now carries this
exception. With `xz -9e` the two archives are 26,948 and 27,036 bytes,
53,984 in all, which is §7's 54 KB. Both expanded digests were checked
equal to EXP-0009's. A search of both images for the author's username,
machine name and `c:\users`, in 8-bit and UTF-16, found nothing; the
text remaining in image B is boot code, directory names, `BOOTMGR`, the
`System Volume Information` files and one volume GUID. The entries'
timestamps and UTC offset and the VHD footer's creation time were not
examined.

---

## Appendix B: §6 gates recovery, not read-only analysis (2026-10-20)

§6 lists four conditions that hold "before the exFAT code is merged".
M14 merged exFAT code that reads a volume and recovers nothing: the boot
region (`e7985de`), directories and live entries (`eabedde`), and live
file content (`97c9242`). Of §6's items, 3 and 4 held when each was
merged. Items 1 and 2 did not, and could not: they are about recovering
deleted files from image B, which M14 does not do.

§6 is read as gating the code that recovers deleted files and the claims
§5 constrains, which are claims about deleted data. M14 makes none. It
counts every deleted entry set as not examined, which leaves a volume
that has them reported as incomplete (`ADR-0021` section 6), and
`--recover` writes nothing for an exFAT volume. Item 3, live files on
image A, was the condition that applied to M14, and
`tests/exfat_content.rs` asserts it. Item 4 is checked for every change
by the method of `ADR-0018` §5.

M15, the recovery of deleted files, does not merge until items 1 and 2
hold, with item 4 for its own change. Nothing in §6's four items, or in
§5, changes.

---

## Appendix C: condition 2 compares modification too, and only times that can be ordered (2026-10-21)

Section 4's condition 2 orders deleted sets by creation time. EXP-0011
tested it on NIST's twenty exFAT images, made on macOS, against the
tags NIST wrote into every block and, for macOS's own files, which NIST
did not tag, against their format. With deleted directories read for
their claims, as M15 reads them (`ADR-0022` Decision K), it recovers 658
sets correctly and 3 wrongly. Two of the three are stale data no rule
found prevents (`ADR-0022` section 15). The third is on dfr-13,
`/Ursa/B104-beta=s-s=D=s-s=C-1.txt`, whose cluster holds segment 3 of
`B083-alpha=s-s=C=a-a=D-2.txt`. NIST's build steps for that image append
to files after others are created. A file created earlier and extended
later into a rival's freed clusters is still the earlier by creation
time, so the rule recovers the rival over data the extension wrote.

**Condition 2 becomes:** no other deleted entry set that claims any of
those clusters was created, or last modified, at the same time as this
set's creation or later. Equal times still refuse every set that shares
them.

**Which times are compared.** A set's creation key is its
CreateTimestamp, Create10msIncrement and CreateUtcOffset (§7.4.5). Its
modification key is its LastModifiedTimestamp, LastModified10msIncrement
and LastModifiedUtcOffset (§7.4.6). Two keys are compared only where:

* both UtcOffset bytes have OffsetValid set (§7.4.10.2) and are the same
  byte; and
* every field of both is in range: DoubleSeconds 0 to 29, Minute 0 to
  59, Hour 0 to 23, Month 1 to 12, Day from 1 to the last day of that
  month (§7.4.8), and 10msIncrement 0 to 199 (§7.4.9).

They are then compared as local times: year, month, day, hour, minute,
and DoubleSeconds times 2000 plus 10msIncrement times 10 milliseconds.
Where two keys cannot be compared, the two sets cannot be ordered, and
condition 2 does not hold for either.

**Why the offset bytes must be equal, not converted.** §7.4.10 makes
the timestamps local and the offset their distance from UTC. Converting
both to UTC would be the specification's reading, but Nordvik and
Axelsson (*It is about time*, FSI: Digital Investigation 42-43 (2022)
301476) measured that macOS writes the offset with its sign switched,
and EXP-0011 saw it independently: NIST's offset byte `0x90` reads as
UTC+4 h, where NIST's log records the deletions in EDT, UTC-4 h. A
conversion would order a macOS-written set against a Windows-written
one wrongly. Comparing local times under the same byte is immune to that
error. A sign error between two sets carrying the same wrong byte
cuts both ways equally and leaves their order unchanged. An offset
marked valid and wrong in some other way cannot be detected, and is a
recorded limit.

**What it costs.** On NIST's twenty images, with deleted directories
read for their claims:

| | Right | Wrong |
| --- | ---: | ---: |
| Section 4 as written | 658 | 3 |
| As amended here | 656 | 2 |

No other decision changes. The two correct sets it refuses, dfr-13's
`B148-beta=s-s=C=a-a=D-1.txt` and `B151-beta=s-s=C=a-a=D-4.txt`, hold
their own data although a rival
claiming their clusters was modified after they were created. That is
the price of failing closed. On EXP-0009's images A and B the amendment
changes no decision: section 6 item 1's 99, 3, 16, 1 and 6 still hold
on B, and A's 96, 4, 16 and 6 on A, measured under this amendment with
an implementation written apart from Taphonomy and from EXP-0009's
scripts (EXP-0011, rule `moe`). On every one of these images, all the
deleted sets of a volume carry one valid offset byte (`0x90`, or `0x94`
on NIST's dfr-15 and dfr-17), so no two sets with different bytes claim
a cluster and the rule on offsets changes no decision; unit tests hold
it. A
volume does hold differing bytes: on dfr-15, seven live sets carry
`0xEC` (UTC-5 h) beside deleted sets carrying `0x94` (UTC+5 h), the
pattern of two writers of which one switched the sign. Which systems
wrote them was not established. The rule's cost on a volume whose files
span a daylight-saving change, where offset bytes differ and pairs are
refused, is not measured.

**Claims seen.** Condition 2 is judged over the claims the walk saw,
and every claim it could not see is counted as a gap (`ADR-0022`
section 5). Deleted directories are read for their claims where they
can be (`ADR-0022` Decision K); eleven of NIST's twenty volumes hold one
that cannot be. Read literally, "recovered only if all of these hold"
would refuse every set on a volume holding an unseen claim, and the
audit's proposal that did so, counting every deleted directory, recovers
nothing on NIST's images. The project decided on 2026-10-21 to judge
over the claims seen. A recovery therefore states "no later claim among
the sets examined".

Section 4's other conditions, section 5, and section 6's four items are
unchanged. Section 6 item 1's figures hold under the amended rule.
