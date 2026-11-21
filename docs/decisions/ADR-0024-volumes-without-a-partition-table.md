# ADR-0024: A Volume With No Partition Table Is Read, and Sector 0 Is Classified First

* **Status:** Accepted
* **Date:** 2026-11-21
* **Decision owners:** Taphonomy project; decided by the planning session
  under the decision-making the author delegated on 2026-11-13 and
  restated on 2026-11-20
* **Scope:** How the run decides whether sector 0 is a partition table
  or the boot record of a volume written there, and what it does with
  each
* **Related:** `ADR-0005`; `ADR-0014` Appendix B.10; `ADR-0018` and its
  Appendix A; `ADR-0019` section 3; `ADR-0022`; EXP-0011;
  `KNOWN_ISSUES.md`

---

## 1. Context

`ADR-0005` made the MBR the first partition scheme, and every image the
run read began with one. A volume can also be written at sector 0 with
no partition table: Windows formats removable media that way, and
`mkfs.vfat`, `mkfs.exfat`, FatFs and relan's exfat do when given a whole
device or image.

The run parsed sector 0 as an MBR whatever it held. Those four
formatters leave bytes 446 to 509 zero, which parse as a valid table
listing no partition, so the run reported `coverage complete`, `volumes
analysed 0` and exited 0 over a volume that may hold deleted files: a
claim of completeness the run had not earned. Windows writes boot code
there, whose bytes give boot indicators other than 0 or 0x80; those
images were refused as a table and reported `coverage none`.
`KNOWN_ISSUES.md` recorded only the second case, from two of Nordvik and
Axelsson's images (2026-10-29).

## 2. How others decide (sources read 2026-11-21)

* **Linux**, `block/partitions/msdos.c`, `msdos_partition()`: after the
  signature, "Reject this in case the boot indicator is not 0 or 0x80";
  slot 1 is then taken as a FAT file system if its reserved-sector,
  FAT-count and media fields are valid.
* **util-linux libblkid**, `libblkid/src/partitions/dos.c`,
  `probe_dos_pt()`: the same boot-indicator rejection, then "this is
  probably either the boot sector of a FAT filesystem or a DOS-type
  partition table", rejecting the table when a vfat, exFAT or NTFS probe
  recognises the sector.
* **The Sleuth Kit**, `tsk/vs/dos.c`, `dos_load_prim_table()` in test
  mode: rejects the table when the OEM name is MSDOS, MSWIN, NTFS or
  FAT ("Not a great check, but we can't really test the table
  entries").

## 3. Measurement

`scripts/experiments/sector-zero-rules.py` classifies sector 0 of each
image under each rule and prints a count per rule. Ground truth is how
each image was made. Over 92 images:

* **68 with a partition table**: the 31 fixtures, the three CFReDS FAT
  images of EXP-0008, NIST's twenty exFAT images, EXP-0009's A and B,
  forensics-samples' Linux image, and the 11 GPT disks among Nordvik
  and Axelsson's images; 66 carry the 0x55AA signature. Every rule calls
  all 66 tables.
* **24 volumes at sector 0**: the six volumes of EXP-0011 Appendix A,
  the six of Appendix C, the fixture `fat32-no-partition-table.img`
  this decision adds, and 11 of Nordvik and Axelsson's images (their
  22 images that convert to raw, the 16 GB one set aside). The kernel's
  rule calls 22 tables and 2 nothing; libblkid's partition prober calls
  22 volumes and leaves 2 to its file system probes; The Sleuth Kit's
  OEM test calls all 24 tables; the rule decided here calls all 24
  volumes.

None of the 92 carries a boot record beside a used partition entry.
Through the release build of v0.2.0, 9 of Nordvik and Axelsson's 11
volumes and all 13 made here reported `coverage complete` with nothing
analysed; their 2 Windows-formatted ones reported `coverage none`.

## 4. Decisions

* **A.** Sector 0 is classified before it is parsed as a table. With no
  0x55AA signature, the run goes on as before. Otherwise a boot
  indicator other than 0 or 0x80 means sector 0 is not a table: it is a
  volume's boot record if `filesystem::identify` identifies one, and
  neither if it does not. With every boot indicator 0 or 0x80, a boot
  record `identify` identifies makes the image one volume if no entry is
  used (type and sector count both non-zero), and ambiguous if any is.
  Otherwise sector 0 is a table, read exactly as before.
* **B.** A volume at sector 0 is analysed as a partition is, over the
  whole image, by the same code, after the line `partition table  none:
  volume boot record at sector 0 (<filesystem>)` and under the heading
  `whole image`.
* **C.** Its recovered files are named with partition number 0.
* **D.** A sector 0 that is neither, or ambiguous, is reported in one
  line and nothing is analysed: coverage `none`, exit status 3
  (`ADR-0014` Appendix B.10). Ambiguity fails closed (`CLAUDE.md`
  section 39).
* **E.** Every image whose sector 0 is a table is reported exactly as
  before.

## 5. Reasoning

The boot-indicator test comes first because a boot record keeps code
where a table's entries would be, and that is the commonest way a boot
record shows itself as no table; it is the kernel's and libblkid's first
test. The probe comes second because the formatters measured here leave
those bytes zero, so the indicators pass and only the sector's own
structure tells the two apart: libblkid's order is the only one of the
three that classifies all 24 volumes. The probe is `identify`, the
identification every partition's volume already goes through, so a
volume at sector 0 must pass the checks a volume in a partition passes.
An OEM name is no test: two of the formatters write `mkfs.fat` and
`EXFAT   `, which The Sleuth Kit's list does not contain. A sector that
is both a valid boot record and a table with a used entry is not
resolved by guessing; either reading could miss the other's data.

## 6. Acceptance (2026-11-21)

* **Unchanged where sector 0 is a table.** `scripts/identity-proof.sh`
  ran the build before this change (`2bc7a76`) and the build after it
  over the 31 fixtures, the three CFReDS images and EXP-0009's A, B and
  zeroed A, in three modes each, from the same working directory: the
  two tables are equal byte for byte.
* **A volume alone reads as it does inside a partition.**
  `scripts/experiments/sector-zero-differential.sh` wraps each of the 24
  volumes in an MBR with `scripts/experiments/mbr-wrap-sparse.py` (the
  same bytes as one partition at sector 2048) and runs the binary on
  both with `--recover --output`: on all 24, the same exit status, the
  same report once the lines that must differ are normalised, and the
  same recovered files byte for byte (10 files on 6 volumes). The
  partitioned path was accepted against EXP-0011 in `ADR-0022`
  Appendix F; this equality carries it to the volume alone. It was shown
  able to fail: with the v0.2.0 build all 24 volumes differ.
* **Ground truth for the 10 files.** Four, on relan's two volumes of
  EXP-0011 Appendix C, are right by that appendix's tags; the fixture's
  one is checked against its original by
  `a_fat32_volume_with_no_partition_table_is_read_and_its_deleted_file_recovered`;
  of Nordvik and Axelsson's five, one holds its own name as the
  experiment wrote it and one is AppleDouble by its header, as its `._`
  name requires; three are macOS's own index files (`remapping0`,
  `remapping1`, `journal.1`) whose format no signature checked here
  identifies, so they are not judged.

## 7. Held by tests

In `src/partition.rs`:
`zero_entry_area_and_a_fat32_boot_record_is_a_volume`,
`zero_entry_area_and_an_exfat_boot_record_is_a_volume`,
`boot_code_in_the_entry_area_and_an_exfat_boot_record_is_a_volume`,
`a_bad_boot_indicator_and_no_boot_record_is_neither`,
`a_boot_record_listing_a_used_entry_is_ambiguous`,
`an_entry_with_a_type_and_no_sectors_or_sectors_and_no_type_is_not_used`,
`an_mbr_with_grub_style_jump_code_and_a_used_entry_is_a_table`,
`all_zero_entries_and_no_boot_record_is_a_table` and
`a_sector_with_no_signature_is_left_to_the_table_parser`. In
`src/analysis.rs`: `a_sector_zero_not_analysed_leaves_nothing_analysed`
and `a_whole_image_past_two_tebibytes_is_given_the_extent_of_its_first_two`.
In `tests/no_partition_table.rs`:
`a_fat32_volume_with_no_partition_table_is_read_and_its_deleted_file_recovered`,
`an_exfat_volume_decides_and_recovers_alike_inside_a_partition_and_alone`,
`a_sector_zero_that_is_neither_a_table_nor_a_boot_record_analyses_nothing`
and
`a_sector_zero_that_is_a_boot_record_and_lists_a_partition_analyses_nothing`.
Removing any rule of Decision A, the whole-image analysis, the
partition number 0 or the coverage of Decision D fails at least one.

## 8. Not decided here

* GPT. The 11 GPT disks above are reported as before: detected, not
  analysed.
* FAT12, FAT16 and NTFS, at sector 0 or in a partition: identified, not
  analysed.
* A volume at sector 0 whose boot sector is damaged: it is classified
  as a table or as neither; it is not read from its backup boot region,
  which the exFAT path tries only for a partition of a declared type.
* Images past 2 TiB: a volume alone is given the extent of the image's
  first 2 TiB, and one whose geometry reaches past it is refused as
  larger than its extent, a reported gap.
* Hybrid layouts that carry both a boot record and a table on purpose;
  none was measured.
