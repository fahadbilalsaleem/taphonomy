#!/usr/bin/env bash
#
# Generate synthetic evidence fixtures for partition table parsing.
#
# Fixtures are synthetic disk images with known layouts. They are NOT
# committed to the repository; this script is the committed artefact, and the
# fixtures are reproducible from it.
#
# See docs/development/DEVELOPMENT_ENVIRONMENT.md section 11.
#
# Requirements:
#   sfdisk                  fdisk
#   sgdisk                  gdisk
#   mkfs.vfat               dosfstools
#   mcopy, mmd, mdel, mrd   mtools
#   mdeltree                mtools
#   minfo, mshowfat         mtools
#   od, sha256sum           coreutils
#   truncate, tr            coreutils
#   xz                      xz-utils
#
# No root privileges are required. No loop devices are used. No filesystem is
# mounted. Every image is a regular file.
#
# Determinism: mkfs.vfat --invariant fixes values that would otherwise derive
# from the clock or a random source. Repeated runs of this script must produce
# byte-identical images. That property is checked by verify-fixtures.sh.
#
# mtools writes wall-clock time into directory entries, and FAT stores
# those timestamps in local time with no timezone field. SOURCE_DATE_EPOCH
# and TZ are therefore both exported below: either alone leaves fixture
# bytes dependent on when or where the script runs. Measured in ADR-0006
# section 4.

set -euo pipefail

readonly SECTOR=512
readonly PART_START=2048          # 1 MiB, the conventional first-partition LBA
readonly IMAGE_SECTORS=131072     # 64 MiB
readonly FAT32_LABEL="TAPHFIX"
readonly VBR_OFFSET=$((PART_START * SECTOR))   # 1048576, byte offset of the
                                               # volume boot record

# Pins every timestamp mtools writes. See ADR-0006 section 4.
export SOURCE_DATE_EPOCH=1577836800   # 2020-01-01 00:00:00 UTC
export TZ=UTC

OUT_DIR="${1:-fixtures/partition}"
readonly OUT_DIR

require() {
    command -v "$1" >/dev/null 2>&1 || {
        printf 'error: required tool not found: %s (%s)\n' "$1" "$2" >&2
        exit 1
    }
}

require sfdisk "apt install fdisk"
require sgdisk "apt install gdisk"
require mkfs.vfat "apt install dosfstools"
require mcopy "apt install mtools"
require mmd "apt install mtools"
require mdel "apt install mtools"
require mrd "apt install mtools"
require mdeltree "apt install mtools"
require minfo "apt install mtools"
require mshowfat "apt install mtools"
require sha256sum coreutils
require od coreutils
require truncate coreutils
require tr coreutils
# Expands the committed exFAT images below. They are the one input here that
# no tool in this repository builds.
require xz xz-utils

mkdir -p "$OUT_DIR"

# Writes a zero-filled image of IMAGE_SECTORS sectors, or of the number of
# sectors given.
blank_image() {
    local path="$1" sectors="${2:-$IMAGE_SECTORS}"
    rm -f "$path"
    dd if=/dev/zero of="$path" bs="$SECTOR" count="$sectors" \
        status=none
}

# Writes a single byte value at a byte offset, without extending the file.
poke() {
    local path="$1" offset="$2" value="$3"
    printf "$(printf '\\x%02x' "$value")" |
        dd of="$path" bs=1 seek="$offset" conv=notrunc status=none
}

note() {
    printf '  %s\n' "$1"
}

# Writes a 16-bit little-endian value at a byte offset.
poke_le16() {
    local path="$1" offset="$2" value="$3"
    poke "$path" "$offset"       "$(( value & 0xff ))"
    poke "$path" "$((offset+1))" "$(( (value >> 8) & 0xff ))"
}

# Writes a 32-bit little-endian value at a byte offset.
poke_le32() {
    local path="$1" offset="$2" value="$3"
    poke "$path" "$offset"       "$(( value & 0xff ))"
    poke "$path" "$((offset+1))" "$(( (value >> 8) & 0xff ))"
    poke "$path" "$((offset+2))" "$(( (value >> 16) & 0xff ))"
    poke "$path" "$((offset+3))" "$(( (value >> 24) & 0xff ))"
}

# Byte offset of the root directory, computed from the volume's own BIOS
# parameter block rather than assumed.
#
# mkfs.vfat derives FATSz32 from the volume geometry, so this offset is not a
# constant across fixtures. A hardcoded value would poke a different structure
# if the geometry ever changed, and the resulting image would still parse.
#
# od reads in host byte order, which matches FAT's little-endian layout on
# every platform this project targets.
root_dir_offset() {
    local path="$1" vbr="$2"
    local bps spc rsv nf fsz rc
    bps=$(od -An -tu2 -j $((vbr + 11)) -N 2 "$path" | tr -d ' ')
    spc=$(od -An -tu1 -j $((vbr + 13)) -N 1 "$path" | tr -d ' ')
    rsv=$(od -An -tu2 -j $((vbr + 14)) -N 2 "$path" | tr -d ' ')
    nf=$( od -An -tu1 -j $((vbr + 16)) -N 1 "$path" | tr -d ' ')
    fsz=$(od -An -tu4 -j $((vbr + 36)) -N 4 "$path" | tr -d ' ')
    rc=$( od -An -tu4 -j $((vbr + 44)) -N 4 "$path" | tr -d ' ')
    printf '%d\n' $(( vbr + (rsv + nf * fsz) * bps + (rc - 2) * spc * bps ))
}

# Writes a byte only if the byte already present is the one expected.
#
# The four boot sector fixtures poke constant offsets and need no such check.
# This one pokes an offset computed at run time, where a wrong offset would
# corrupt a different structure and still produce an image that parses.
poke_expecting() {
    local path="$1" offset="$2" expected="$3" value="$4"
    local current
    current=$(od -An -tx1 -j "$offset" -N 1 "$path" | tr -d ' ')
    if [ "$current" != "$expected" ]; then
        printf 'error: %s offset %s holds 0x%s, expected 0x%s\n' \
            "$path" "$offset" "$current" "$expected" >&2
        exit 1
    fi
    poke "$path" "$offset" "$value"
}

# ---------------------------------------------------------------------------
# 1. Valid MBR, single FAT32 partition.
#    The expected-success case.
# ---------------------------------------------------------------------------
fixture_single_fat32() {
    local path="$OUT_DIR/mbr-single-fat32.img"
    printf 'mbr-single-fat32.img\n'
    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0x1a2b3c4d
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c, bootable
EOF

    # --mbr=n prevents mkfs.vfat writing its own fake MBR over ours.
    # --offset writes the filesystem into the partition, not at sector 0.
    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    note "one FAT32 partition, type 0x0c, bootable, LBA ${PART_START}"
}

# ---------------------------------------------------------------------------
# 2. Four primary partitions.
#    Exercises the full entry table.
# ---------------------------------------------------------------------------
fixture_four_partitions() {
    local path="$OUT_DIR/mbr-four-partitions.img"
    printf 'mbr-four-partitions.img\n'
    blank_image "$path"

    local span=$(( (IMAGE_SECTORS - PART_START) / 4 ))

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0x11223344
unit: sectors
${path}1 : start=${PART_START}, size=${span}, type=c
${path}2 : start=$((PART_START + span)), size=${span}, type=c
${path}3 : start=$((PART_START + span * 2)), size=${span}, type=83
${path}4 : start=$((PART_START + span * 3)), size=$((span - 1)), type=83
EOF

    note "four primary partitions, types 0x0c, 0x0c, 0x83, 0x83"
}

# ---------------------------------------------------------------------------
# 3. Valid signature, no partition entries.
#    A partitioned-but-empty disk is valid, not an error.
# ---------------------------------------------------------------------------
fixture_empty_table() {
    local path="$OUT_DIR/mbr-empty.img"
    printf 'mbr-empty.img\n'
    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0x55667788
EOF

    note "valid 0x55AA signature, zero partition entries"
}

# ---------------------------------------------------------------------------
# 4. No signature at all.
#    Must be rejected. SAFETY.md section 12: fail closed.
# ---------------------------------------------------------------------------
fixture_no_signature() {
    local path="$OUT_DIR/no-signature.img"
    printf 'no-signature.img\n'
    blank_image "$path"
    note "first sector entirely zero, no 0x55AA at offset 0x1FE"
}

# ---------------------------------------------------------------------------
# 5. Plausible entries, corrupted signature.
#    The dangerous case: everything looks right except the one field that
#    says the table is real. Must be rejected, not parsed optimistically.
# ---------------------------------------------------------------------------
fixture_bad_signature() {
    local path="$OUT_DIR/bad-signature.img"
    printf 'bad-signature.img\n'
    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0x99aabbcc
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c
EOF

    # 0x1FE = 510, 0x1FF = 511. Valid is 0x55 0xAA.
    poke "$path" 510 0x00
    poke "$path" 511 0x00

    note "valid partition entry, signature overwritten with 0x0000"
}

# ---------------------------------------------------------------------------
# 6. Partition extending beyond the end of the image.
#    Must be rejected. DEVELOPMENT_ENVIRONMENT.md section 18: validate
#    offsets and lengths against actual bounds, never trust declared values.
# ---------------------------------------------------------------------------
fixture_partition_beyond_end() {
    local path="$OUT_DIR/partition-beyond-end.img"
    printf 'partition-beyond-end.img\n'
    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0xdeadbeef
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c
EOF

    # Overwrite the sector-count field of entry 1 with 0xFFFFFFFF.
    # Entry 1 begins at 0x1BE; the sector-count field is at entry+12,
    # i.e. 0x1BE + 12 = 0x1CA = 458.
    local i
    for i in 458 459 460 461; do
        poke "$path" "$i" 0xff
    done

    note "entry 1 declares 0xFFFFFFFF sectors, far beyond the 64 MiB image"
}

# ---------------------------------------------------------------------------
# 7. GPT protective MBR.
#    A GPT disk presents a valid MBR containing one 0xEE partition spanning
#    the disk. An MBR-only parser that does not recognise this will report a
#    single whole-disk partition for every GPT disk it sees.
#    ADR-0005 section 4 requires this be detected and reported.
# ---------------------------------------------------------------------------
fixture_gpt_protective() {
    local path="$OUT_DIR/gpt-protective.img"
    printf 'gpt-protective.img\n'
    blank_image "$path"

    sgdisk --disk-guid=00000000-0000-4000-8000-000000000000 \
           --new=1:2048:0 \
           --partition-guid=1:00000000-0000-4000-8000-000000000001 \
           --typecode=1:0700 \
           "$path" >/dev/null 2>&1

    note "GPT with protective MBR, entry type 0xEE at LBA 0"
}

# ---------------------------------------------------------------------------
# 8. Declared partition type disagrees with actual content.
#    The partition entry declares type 0x07 (NTFS/exFAT) but the volume is
#    FAT32. A partition type byte records intent, not content. Taphonomy must
#    identify the filesystem from the volume's own structure and report the
#    disagreement rather than trusting either source.
# ---------------------------------------------------------------------------
fixture_type_mismatch() {
    local path="$OUT_DIR/mbr-type-mismatch.img"
    printf 'mbr-type-mismatch.img\n'
    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0x0badf00d
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=7
EOF

    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    note "declared type 0x07 (NTFS/exFAT), actual content FAT32"
}

# ---------------------------------------------------------------------------
# 9. FAT32 volume declares more sectors than its partition contains.
#    TotSec32 at BPB offset 0x20 is corrupted to claim 200,000 sectors
#    inside a 129,024-sector partition. Declared geometry must be checked
#    against the partition's actual extent, not trusted outright.
# ---------------------------------------------------------------------------
fixture_fat32_oversized_volume() {
    local path="$OUT_DIR/fat32-oversized-volume.img"
    printf 'fat32-oversized-volume.img\n'
    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0xfa730001
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c, bootable
EOF

    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    poke_le32 "$path" $((VBR_OFFSET + 0x20)) 200000

    note "volume declares more sectors than its partition contains"
}

# ---------------------------------------------------------------------------
# 10. FAT32 hidden sector count disagrees with the partition start.
#     mkfs.vfat writes zero to BPB_HiddSec for a volume formatted at an
#     offset, so poking zero changes nothing. HiddSec at BPB offset 0x1C
#     is corrupted to 9999 instead, a value the volume could not have
#     started at.
# ---------------------------------------------------------------------------
fixture_fat32_hidden_mismatch() {
    local path="$OUT_DIR/fat32-hidden-mismatch.img"
    printf 'fat32-hidden-mismatch.img\n'
    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0xfa730002
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c, bootable
EOF

    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    poke_le32 "$path" $((VBR_OFFSET + 0x1C)) 9999

    note "hidden sector count records a start the volume does not have"
}

# ---------------------------------------------------------------------------
# 11. FAT32 root directory cluster number below the first valid cluster.
#     RootClus at BPB offset 0x2C is corrupted to 1. Cluster numbers 0 and
#     1 are reserved; the first valid cluster is 2.
# ---------------------------------------------------------------------------
fixture_fat32_bad_root_cluster() {
    local path="$OUT_DIR/fat32-bad-root-cluster.img"
    printf 'fat32-bad-root-cluster.img\n'
    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0xfa730003
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c, bootable
EOF

    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    poke_le32 "$path" $((VBR_OFFSET + 0x2C)) 1

    note "root directory cluster number below the first valid cluster"
}

# ---------------------------------------------------------------------------
# 12. FAT32 FAT too small to describe the volume's cluster count.
#     FATSz32 at BPB offset 0x24 is corrupted to 4 sectors, far too few to
#     hold entries for the clusters the volume otherwise declares.
# ---------------------------------------------------------------------------
fixture_fat32_undersized_fat() {
    local path="$OUT_DIR/fat32-undersized-fat.img"
    printf 'fat32-undersized-fat.img\n'
    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0xfa730004
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c, bootable
EOF

    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    poke_le32 "$path" $((VBR_OFFSET + 0x24)) 4

    note "FAT too small to describe the volume's cluster count"
}

# ---------------------------------------------------------------------------
# 13. FAT32 volume with entries in its root directory.
#     Every other fixture is an empty filesystem. This one carries the four
#     root-entry shapes M5 must enumerate: a pure 8.3 name, a lowercase 8.3
#     name recorded through the NTRes flags, a name too long for 8.3 which
#     produces preceding attribute-0x0F entries, and a subdirectory.
#     The volume label written by mkfs.vfat -n is a fifth shape,
#     attribute 0x08.
#
#     File contents are fixed short strings so that later milestones have
#     known-good data to recover and hash against.
# ---------------------------------------------------------------------------
fixture_fat32_root_entries() {
    local path="$OUT_DIR/fat32-root-entries.img"
    printf 'fat32-root-entries.img\n'
    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0xfa730005
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c, bootable
EOF

    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    local work
    work="$(mktemp -d)"

    printf 'taphonomy hello fixture\n' > "$work/hello.txt"
    printf 'taphonomy readme fixture\n' > "$work/readme.md"
    printf 'taphonomy long name fixture\n' > "$work/annual.txt"

    # Order is fixed, so directory entry order is deterministic.
    MTOOLS_SKIP_CHECK=1 mcopy -i "${path}@@${VBR_OFFSET}" \
        "$work/hello.txt" ::/HELLO.TXT
    MTOOLS_SKIP_CHECK=1 mcopy -i "${path}@@${VBR_OFFSET}" \
        "$work/readme.md" ::/readme.md
    MTOOLS_SKIP_CHECK=1 mcopy -i "${path}@@${VBR_OFFSET}" \
        "$work/annual.txt" "::/annual report 2019.txt"
    MTOOLS_SKIP_CHECK=1 mmd -i "${path}@@${VBR_OFFSET}" ::/logs

    rm -rf "$work"

    note "root directory with 8.3, lowercase, long-name and directory entries"
}

# ---------------------------------------------------------------------------
# 14. FAT32 volume whose root directory spans more than one cluster.
#     Fixture 13 has a root directory of one cluster, so its FAT entry is an
#     end-of-chain mark and nothing in the fixture set exercises cluster
#     chain walking. This fixture does.
#
#     At 512 bytes per cluster the root directory holds sixteen 32-byte
#     entries. The volume label takes the first, so twenty files require a
#     second cluster. The names are pure uppercase 8.3, which produces one
#     entry each and no long-name entries, so the entry count is predictable
#     rather than emergent.
#
#     Every file carries content. A zero-length file records a first cluster
#     of zero and consumes no cluster, which would leave the root directory
#     growing into the adjacent cluster and let a walk that ignores the FAT
#     pass. With content, the files consume clusters before the root needs to
#     grow, and the root's chain is non-contiguous. See ADR-0008 section 8.3.
# ---------------------------------------------------------------------------
fixture_fat32_root_multicluster() {
    local path="$OUT_DIR/fat32-root-multicluster.img"
    printf 'fat32-root-multicluster.img\n'
    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0xfa730006
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c, bootable
EOF

    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    local work
    work="$(mktemp -d)"

    # Brace expansion rather than seq, so the ordering is fixed by the shell
    # and no tool beyond those already required is introduced.
    local n
    for n in {01..20}; do
        printf 'taphonomy fixture file %s\n' "$n" > "$work/FILE$n.TXT"
        MTOOLS_SKIP_CHECK=1 mcopy -i "${path}@@${VBR_OFFSET}" \
            "$work/FILE$n.TXT" "::/FILE$n.TXT"
    done

    rm -rf "$work"

    note "root directory spanning two clusters, twenty 8.3 entries"
}

# ---------------------------------------------------------------------------
# 15. Deleted directory entries.
#
#     What deletion destroys and what survives was measured in EXP-0003.
#     ADR-0009 records the decisions these two fixtures exist to test.
# ---------------------------------------------------------------------------

# Builds the volume both deleted-entry fixtures share.
#
# Creation order fixes slot order, and slot order is what the tests assert.
# The resulting root directory is:
#
#    0  volume label
#    1  REUSE1.TXT              live, in the slot REUSED.TXT vacated
#    2  REUSE2.TXT              live, in a vacated long-name component slot
#    3  deleted long-name       the surviving half of a two-entry set
#    4  deleted PARTIA~1.TXT    first byte recoverable from slot 3's checksum
#    5  deleted GONE            a removed subdirectory
#    6  deleted long-name       last component, stored first
#    7  deleted long-name       first component, adjacent to its short entry
#    8  deleted COMPLE~1.TXT    first byte recoverable from either component
#    9  KEEP.TXT                live, after the deleted entries
#   10  deleted PLAIN.TXT       no long-name set; first byte unrecoverable
#   11  terminator
build_deleted_volume() {
    local path="$1" label_id="$2"

    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: ${label_id}
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c, bootable
EOF

    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    local work
    work="$(mktemp -d)"

    printf 'taphonomy reused slot fixture\n'     > "$work/reused.txt"
    printf 'taphonomy partial set fixture\n'     > "$work/partial.txt"
    printf 'taphonomy complete set fixture\n'    > "$work/complete.txt"
    printf 'taphonomy surviving entry fixture\n' > "$work/keep.txt"
    printf 'taphonomy no long name fixture\n'    > "$work/plain.txt"
    printf 'taphonomy first reuse fixture\n'     > "$work/reuse1.txt"
    printf 'taphonomy second reuse fixture\n'    > "$work/reuse2.txt"

    local img="${path}@@${VBR_OFFSET}"

    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/reused.txt" ::/REUSED.TXT
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/partial.txt" \
        "::/partial long name.txt"
    MTOOLS_SKIP_CHECK=1 mmd -i "$img" ::/gone
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/complete.txt" \
        "::/complete long name.txt"
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/keep.txt"  ::/KEEP.TXT
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/plain.txt" ::/PLAIN.TXT

    # EXP-0003: deletion writes 0xE5 into the first byte of every entry in the
    # set and zeroes the cluster chain in both FATs. Nothing else changes, and
    # no data cluster is touched.
    MTOOLS_SKIP_CHECK=1 mdel -i "$img" ::/REUSED.TXT
    MTOOLS_SKIP_CHECK=1 mdel -i "$img" "::/partial long name.txt"
    MTOOLS_SKIP_CHECK=1 mrd  -i "$img" ::/gone
    MTOOLS_SKIP_CHECK=1 mdel -i "$img" "::/complete long name.txt"

    # mtools fills the earliest deleted slot first. These two consume the
    # entry REUSED.TXT vacated and then one component of the first long-name
    # set, leaving that set incomplete with nothing in the evidence to say so.
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/reuse1.txt" ::/REUSE1.TXT
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/reuse2.txt" ::/REUSE2.TXT

    # Deleted last, so no later write reuses its slot. This is the entry whose
    # first byte no method recovers: ADR-0009 section 6.4.
    MTOOLS_SKIP_CHECK=1 mdel -i "$img" ::/PLAIN.TXT

    rm -rf "$work"
}

fixture_fat32_deleted_entries() {
    local path="$OUT_DIR/fat32-deleted-entries.img"
    printf 'fat32-deleted-entries.img\n'

    build_deleted_volume "$path" 0xfa730007

    note "deleted 8.3, complete and partial long-name sets, deleted directory"
}

fixture_fat32_deleted_residue() {
    local path="$OUT_DIR/fat32-deleted-residue.img"
    printf 'fat32-deleted-residue.img\n'

    build_deleted_volume "$path" 0xfa730008

    # One byte, into a slot that already held a deletion marker. Slot 5
    # becomes the terminator, and slots 6 to 10 become residue: a complete
    # deleted long-name set, a live entry, and a deleted 8.3 entry.
    #
    # ADR-0006 section 5.1 permits deliberate corruption of one field in an
    # otherwise valid image. Every other byte was written by mkfs.vfat, mcopy,
    # mmd, mdel and mrd. EXP-0003 established that mtools cannot produce this
    # shape by any ordinary sequence: FAT directories do not shrink, and
    # deleted slots are reused before the directory grows.
    local root offset
    root="$(root_dir_offset "$path" "$VBR_OFFSET")"
    offset=$(( root + 5 * 32 ))
    poke_expecting "$path" "$offset" e5 0

    note "a terminator before live and deleted entries, one byte poked"
}

# ---------------------------------------------------------------------------
# 17 and 18. FAT32 volumes for recovering a deleted file's data.
#
# ADR-0010 Decision H. The deleted fixtures of M6 cannot exercise M7: every
# deleted file in them occupies exactly one cluster, so there is no run to
# walk, and nothing in them reuses a deleted file's clusters, so the FAT
# check has nothing to refuse.
#
# BIG.TXT is 1600 bytes, four clusters at this volume's 512-byte cluster
# size, written before anything else so it takes the first four data
# clusters. LIVE1.TXT and LIVE2.TXT follow it and are not deleted, so the
# clusters immediately after the run stay allocated.
#
# The content is forty lines of forty bytes. A test asserting a digest must
# be able to rebuild exactly what was written, and a fixed line width makes
# the total a property of the loop rather than of the strings.
# ---------------------------------------------------------------------------
build_recovery_volume() {
    local path="$1" label_id="$2"

    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: ${label_id}
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c, bootable
EOF

    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    local work
    work="$(mktemp -d)"

    local i
    : > "$work/big.txt"
    for i in $(seq 1 40); do
        printf 'taphonomy multicluster fixture line %03d\n' "$i" >> "$work/big.txt"
    done

    printf 'taphonomy first live fixture\n'  > "$work/live1.txt"
    printf 'taphonomy second live fixture\n' > "$work/live2.txt"
    printf 'taphonomy single cluster fix\n'  > "$work/small.txt"

    local img="${path}@@${VBR_OFFSET}"

    # Order fixes the layout. mtools allocates forward from the first free
    # cluster, so these take clusters 3 to 6, then 7, then 8, then 9, then
    # 10 for the directory.
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/big.txt"   ::/BIG.TXT
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/live1.txt" ::/LIVE1.TXT
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/live2.txt" ::/LIVE2.TXT
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/small.txt" ::/SMALL.TXT
    MTOOLS_SKIP_CHECK=1 mmd   -i "$img" ::/gone

    # Nothing is written after these, so no slot and no cluster is reused.
    # The three deleted entries keep the slots they were given.
    MTOOLS_SKIP_CHECK=1 mdel -i "$img" ::/BIG.TXT
    MTOOLS_SKIP_CHECK=1 mdel -i "$img" ::/SMALL.TXT
    MTOOLS_SKIP_CHECK=1 mrd  -i "$img" ::/gone

    rm -rf "$work"
}

fixture_fat32_recover_run() {
    local path="$OUT_DIR/fat32-recover-run.img"
    printf 'fat32-recover-run.img\n'

    build_recovery_volume "$path" 0xfa730009

    note "a four-cluster deleted file with its run intact, and a live tail"
}

fixture_fat32_recover_collision() {
    local path="$OUT_DIR/fat32-recover-collision.img"
    printf 'fat32-recover-collision.img\n'

    build_recovery_volume "$path" 0xfa73000a

    # One field. BIG.TXT's DIR_FileSize goes from 1600 to 2560, so the run it
    # implies grows from four clusters to five and reaches cluster 7, which
    # LIVE1.TXT holds. That is the shape a volume takes when a deleted file's
    # clusters have been reused.
    #
    # mtools can produce that shape unaided, once the volume is full enough
    # that its free-cluster search wraps: measured in EXP-0004. The poke is
    # kept because it produces the shape while leaving every other byte
    # identical to fat32-recover-run.img, which no unpoked route gives, and
    # that control is what makes the refusal attributable to the declared
    # size alone. ADR-0010 Appendix D.
    #
    # ADR-0006 section 5.1 permits deliberate corruption of one field in an
    # otherwise valid image. Every other byte here was written by mkfs.vfat,
    # mcopy, mmd, mdel and mrd.
    #
    # Slot 0 is the volume label and slot 1 is BIG.TXT, which needs no
    # long-name set. DIR_FileSize is the last four bytes of the entry.
    local root offset
    root="$(root_dir_offset "$path" "$VBR_OFFSET")"
    offset=$(( root + 1 * 32 + 28 ))
    poke_expecting "$path" "$offset"         40 0x00
    poke_expecting "$path" $(( offset + 1 )) 06 0x0a

    note "the same volume, with one deleted entry's size poked to overlap"
}

# ---------------------------------------------------------------------------
# A FAT32 volume that declares no backup boot sector.
#
#     mkfs.vfat -R 2 reserves two sectors and writes BPB_BkBootSec = 0, with
#     BPB_FSInfo = 1. Nobody asked for that: it is what mkfs.fat 4.2 does
#     when the reserved area has no room for a backup (measured, and so is
#     -b 0 at the default 32 reserved sectors). Reserving two sectors also
#     moves the first data sector, so cluster 2 is not where it is on every
#     other fixture's volume.
#
#     No byte is poked. ADR-0006 section 5.1 rejects hand-built structure,
#     and the field is the one mkfs.vfat itself wrote.
#
#     GONE.TXT is written first, so it takes the first clusters after the
#     root directory's, and KEEP.TXT follows and stays live. Nothing is
#     written after the deletion, so no slot and no cluster is reused.
# ---------------------------------------------------------------------------
fixture_fat32_no_backup_boot_sector() {
    local path="$OUT_DIR/fat32-no-backup-boot-sector.img"
    printf 'fat32-no-backup-boot-sector.img\n'

    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0xfa730017
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c, bootable
EOF

    mkfs.vfat --invariant --mbr=n -F 32 -R 2 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    local work
    work="$(mktemp -d)"

    local i
    : > "$work/gone.txt"
    for i in $(seq 1 40); do
        printf 'taphonomy no backup boot sector line %03d\n' "$i" >> "$work/gone.txt"
    done
    printf 'taphonomy no backup keep fixture\n' > "$work/keep.txt"

    local img="${path}@@${VBR_OFFSET}"

    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/gone.txt" ::/GONE.TXT
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/keep.txt" ::/KEEP.TXT
    MTOOLS_SKIP_CHECK=1 mdel  -i "$img" ::/GONE.TXT

    rm -rf "$work"

    note "two reserved sectors, no backup boot sector, one deleted file"
}

# ---------------------------------------------------------------------------
# The volume both fragmentation fixtures share.
#
#     FAT32 deletion zeroes the cluster chain (EXP-0003), so a deleted entry
#     states only a first cluster and a size, and the run it implies is a
#     contiguous assumption. Both fixtures below turn on that assumption
#     being wrong; they differ only in what holds the clusters between the
#     fragments when the volume is handed to the tool.
#
#     EXP-0004 measured how to produce the arrangement with mtools alone.
#     mtools starts each free-cluster search from the FAT32 FSINFO next-free
#     hint, so a cluster freed behind that hint is not reissued until the
#     search wraps. Freeing a gap therefore achieves nothing; the volume must
#     be filled so that scattered single clusters are the only space left, at
#     which point a three-cluster file has no contiguous option. EXP-0004
#     Appendix A.3 records that this is NIST CFTT's published Forced Overwrite
#     technique rather than a local invention.
#
#     No byte of either image is written by this project. ADR-0006 section 5.1
#     rejects hand-built structure, and no poke is needed here.
#
#     This builder stops with FRAG.BIN written and its fragmented chain
#     asserted. Each fixture then performs its own deletions, which is where
#     the two diverge and is the same division build_recovery_volume uses.
# ---------------------------------------------------------------------------
build_fragmented_volume() {
    local path="$1" signature="$2"
    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: ${signature}
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c, bootable
EOF

    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    local work img i j free_clusters chain
    work="$(mktemp -d)"
    img="${path}@@${VBR_OFFSET}"

    # Seven single-cluster files take clusters 3 to 9 in order. Every line
    # names the file it belongs to, so a recovered cluster can be traced to
    # its source by reading it.
    for i in 0 1 2 3 4 5 6; do
        : > "$work/s$i.bin"
        for j in $(seq 0 19); do
            printf 'taphonomy spacer%d block %06d\n' "$i" "$j" >> "$work/s$i.bin"
        done
        truncate -s 512 "$work/s$i.bin"
        MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/s$i.bin" "::/S$i.BIN"
    done

    # Fill every remaining cluster so the next allocation search must wrap.
    # minfo reports the count as a plain integer; mdir reports free space in
    # locale-formatted digit groups, which would make generation depend on
    # the generating machine's locale.
    free_clusters="$(MTOOLS_SKIP_CHECK=1 minfo -i "$img" |
        sed -n 's/^free clusters=//p')"
    head -c $(( free_clusters * SECTOR )) /dev/zero |
        tr '\0' 'F' > "$work/filler.bin"
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/filler.bin" ::/FILLER.BIN

    # Free clusters 4, 6 and 8, leaving no two of them adjacent.
    for i in 1 3 5; do
        MTOOLS_SKIP_CHECK=1 mdel -i "$img" "::/S$i.BIN"
    done

    : > "$work/frag.bin"
    for j in $(seq 0 55); do
        printf 'taphonomy fragment block %06d\n' "$j" >> "$work/frag.bin"
    done
    truncate -s 1536 "$work/frag.bin"
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/frag.bin" ::/FRAG.BIN

    # The arrangement is the fixture. A geometry or mkfs.vfat change that
    # allocated these three clusters contiguously would leave an image that
    # still passes every structural check and tests nothing, which is
    # ADR-0008 section 8.1's "proves versus illustrates" problem. Assert it.
    chain="$(MTOOLS_SKIP_CHECK=1 mshowfat -i "$img" ::/FRAG.BIN)"
    if [ "$chain" != "::/FRAG.BIN <4> <6> <8>" ]; then
        printf 'error: FRAG.BIN is not fragmented as expected: %s\n' \
            "$chain" >&2
        rm -rf "$work"
        exit 1
    fi

    rm -rf "$work"
}

# ---------------------------------------------------------------------------
# 19. FAT32 volume holding a deleted file whose clusters were not adjacent,
#     with the clusters between its fragments freed as well.
#
#     The tool's characteristic false positive. The run FRAG.BIN's entry
#     implies reads as entirely free and extraction yields a plausible wrong
#     digest. Nothing in the evidence records that the file was ever
#     fragmented. EXP-0004 Appendix A.2 measures the whole volume: three of
#     its five deleted entries recover content that is not their own file's,
#     and the tool reports all five identically.
# ---------------------------------------------------------------------------
fixture_fat32_fragmented_deleted() {
    local path="$OUT_DIR/fat32-fragmented-deleted.img"
    printf 'fat32-fragmented-deleted.img\n'

    build_fragmented_volume "$path" 0xfa73000b

    # Free the clusters between the fragments, then the fragmented file. The
    # run its entry implies now reads as entirely free.
    local img="${path}@@${VBR_OFFSET}"
    MTOOLS_SKIP_CHECK=1 mdel -i "$img" ::/S2.BIN
    MTOOLS_SKIP_CHECK=1 mdel -i "$img" ::/S4.BIN
    MTOOLS_SKIP_CHECK=1 mdel -i "$img" ::/FRAG.BIN

    note "a deleted file whose clusters were not adjacent, with the"
    note "clusters between its fragments freed as well"
}

# ---------------------------------------------------------------------------
# 20. The same arrangement with the clusters between the fragments still
#     allocated to live files.
#
#     The case above measures what the tool gets wrong. This one measures
#     what it gets right. FRAG.BIN's implied run is 4 to 6 and cluster 5 is
#     held by the live S2.BIN, so assess reaches an allocated cluster and
#     ADR-0010 Decision C refuses the entry instead of extracting it.
#
#     This is Case 2 of the canonical list in Meyer and Roy, which EXP-0004's
#     external practice section records: where an active file lies between the
#     fragments, Recuva and Magnet AXIOM recover it anyway because they do not
#     refuse on an allocated cluster. It is the one arrangement in that list
#     where this tool's behaviour is correct, and until now nothing in the
#     tree measured it.
#
#     The volume also carries two entries whose single cluster was reallocated
#     to FRAG.BIN and never freed again: S3.BIN at cluster 6 and S5.BIN at
#     cluster 8. Both runs read as free and both recover FRAG.BIN's content,
#     which is the overwritten-deleted-file case. EXP-0004 Appendix A.2
#     recorded the same two digests on the volume above.
#
#     Only the two mdel calls of fixture 19 are omitted, so the two images
#     differ in twenty bytes: the disk signature, two directory entries' first
#     byte, two cluster chains in each of the two FATs, and the FSINFO
#     free-cluster count.
# ---------------------------------------------------------------------------
fixture_fat32_fragmented_live_gap() {
    local path="$OUT_DIR/fat32-fragmented-live-gap.img"
    printf 'fat32-fragmented-live-gap.img\n'

    build_fragmented_volume "$path" 0xfa73000c

    # Only the fragmented file. S2.BIN and S4.BIN stay live, so clusters 5
    # and 7 stay allocated and the implied run 4 to 6 reaches one of them.
    local img="${path}@@${VBR_OFFSET}"
    MTOOLS_SKIP_CHECK=1 mdel -i "$img" ::/FRAG.BIN

    note "a deleted file whose clusters were not adjacent, with a live"
    note "file still holding a cluster inside the run it implies"
}

# Fails the run unless mshowfat reports the chain this fixture exists for.
#
# A geometry or allocation change that moved these directories would leave
# images that pass every structural check and test nothing, which is
# ADR-0008 section 8.1's "proves versus illustrates" problem.
expect_chain() {
    local img="$1" path="$2" expected="$3" chain
    chain="$(MTOOLS_SKIP_CHECK=1 mshowfat -i "$img" "$path")"
    if [ "$chain" != "$expected" ]; then
        printf 'error: unexpected chain: %s, expected %s\n' \
            "$chain" "$expected" >&2
        exit 1
    fi
}

# ---------------------------------------------------------------------------
# 21. FAT32 volume holding a deleted directory with a deleted subtree inside.
#
#     EXP-0005's subtree construction. /GONE holds ALPHA.TXT, BETA.TXT and
#     the directory DEEP, which holds GAMMA.TXT, and mdeltree removes the
#     lot. Every entry inside survives with only its first byte changed, so
#     three recoverable files sit at depths one and two. Until subdirectories
#     are read the tool reports one unread directory and none of the three.
#
#     Every file's content is its own name, so a recovered digest can be
#     checked against the content without trusting the tool.
#
#     ADR-0016 section 11, conditions 1 and 2.
# ---------------------------------------------------------------------------
fixture_fat32_deleted_subtree() {
    local path="$OUT_DIR/fat32-deleted-subtree.img"
    printf 'fat32-deleted-subtree.img\n'

    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0xfa73000d
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c, bootable
EOF

    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    local work img name
    work="$(mktemp -d)"
    img="${path}@@${VBR_OFFSET}"

    for name in ALPHA BETA GAMMA; do
        printf '%s.TXT\n' "$name" > "$work/$name"
    done

    MTOOLS_SKIP_CHECK=1 mmd -i "$img" ::/GONE
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/ALPHA" ::/GONE/ALPHA.TXT
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/BETA" ::/GONE/BETA.TXT
    MTOOLS_SKIP_CHECK=1 mmd -i "$img" ::/GONE/DEEP
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/GAMMA" ::/GONE/DEEP/GAMMA.TXT

    expect_chain "$img" ::/GONE "::/GONE <3>"
    expect_chain "$img" ::/GONE/DEEP "::/GONE/DEEP <6>"

    MTOOLS_SKIP_CHECK=1 mdeltree -i "$img" ::/GONE

    rm -rf "$work"

    note "a deleted directory holding two deleted files and a deleted"
    note "subdirectory holding a third"
}

# ---------------------------------------------------------------------------
# 22. FAT32 volume holding a deleted directory whose two clusters are not
#     adjacent.
#
#     EXP-0005's split construction. /BIG takes cluster 3, PAYLOAD.BIN takes
#     4 to 18, fourteen empty files fill the sixteen slots of BIG's first
#     cluster with . and .., and BIG/TAIL then takes 19 before BIG extends
#     to 20. mdeltree removes BIG.
#
#     BIG's first cluster holds no terminator, and its second is reachable
#     from nothing: its chain is zeroed and it carries no . or .. entry. It
#     held the only entry leading to TAIL and OMEGA.TXT. On this volume a
#     listing that may continue is a coverage gap, and the cluster adjacent
#     to BIG's first is PAYLOAD.BIN's data, not BIG's continuation.
#
#     ADR-0016 section 11, conditions 1 and 3.
# ---------------------------------------------------------------------------
fixture_fat32_deleted_split_directory() {
    local path="$OUT_DIR/fat32-deleted-split-directory.img"
    printf 'fat32-deleted-split-directory.img\n'

    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0xfa73000e
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c, bootable
EOF

    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    local work img i
    work="$(mktemp -d)"
    img="${path}@@${VBR_OFFSET}"

    # 640 lines of 12 bytes: 7,680 bytes, fifteen 512-byte clusters. One
    # printf rather than a pipe, because under pipefail a pipe from yes
    # fails when head closes it.
    printf 'PAYLOAD.BIN\n%.0s' $(seq 640) > "$work/payload"
    : > "$work/empty"
    printf 'OMEGA.TXT\n' > "$work/omega"

    MTOOLS_SKIP_CHECK=1 mmd -i "$img" ::/BIG
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/payload" ::/PAYLOAD.BIN
    for i in $(seq -w 1 14); do
        MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/empty" "::/BIG/E$i.TXT"
    done
    MTOOLS_SKIP_CHECK=1 mmd -i "$img" ::/BIG/TAIL
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/omega" ::/BIG/TAIL/OMEGA.TXT

    expect_chain "$img" ::/BIG "::/BIG <3> <20>"
    expect_chain "$img" ::/PAYLOAD.BIN "::/PAYLOAD.BIN <4-18>"
    expect_chain "$img" ::/BIG/TAIL "::/BIG/TAIL <19>"

    MTOOLS_SKIP_CHECK=1 mdeltree -i "$img" ::/BIG

    rm -rf "$work"

    note "a deleted directory whose two clusters are not adjacent, the"
    note "second reachable from nothing"
}

# Byte offset of a data cluster, computed from the volume's own BIOS
# parameter block, as root_dir_offset is and for the same reason.
cluster_offset() {
    local path="$1" vbr="$2" cluster="$3"
    local base bps spc
    base=$(root_dir_offset "$path" "$vbr")
    bps=$(od -An -tu2 -j $((vbr + 11)) -N 2 "$path" | tr -d ' ')
    spc=$(od -An -tu1 -j $((vbr + 13)) -N 1 "$path" | tr -d ' ')
    printf '%d\n' $(( base + (cluster - 2) * spc * bps ))
}

# Byte offset of a directory entry's first-cluster low word.
entry_first_cluster_lo() {
    local path="$1" vbr="$2" cluster="$3" slot="$4"
    printf '%d\n' $(( $(cluster_offset "$path" "$vbr" "$cluster") + slot * 32 + 0x1A ))
}

# ---------------------------------------------------------------------------
# 23. FAT32 volume nested 130 directories deep.
#
#     ADR-0016 Decision E reads 128 levels below the root and reports the
#     rest as unread, so a volume must nest deeper than the bound for the
#     bound to be measurable. EXP-0006 measured that mmd alone builds this:
#     the deepest path is 262 characters and every level is one cluster.
#
#     ADR-0016 section 11, condition 6.
# ---------------------------------------------------------------------------
fixture_fat32_nested_directories() {
    local path="$OUT_DIR/fat32-nested-directories.img"
    printf 'fat32-nested-directories.img\n'

    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0xfa73000f
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c, bootable
EOF

    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    local img nested i
    img="${path}@@${VBR_OFFSET}"
    nested="::"

    for i in $(seq 1 130); do
        nested="$nested/D"
        MTOOLS_SKIP_CHECK=1 mmd -i "$img" "$nested"
    done

    # The first level takes cluster 3 and each level the next, so the
    # deepest is cluster 132. A geometry change that moved them would leave
    # a volume that still parses and tests a different depth.
    expect_chain "$img" ::/D "::/D <3>"
    expect_chain "$img" "$nested" "$nested <132>"

    note "130 nested directories, deeper than the 128 levels read"
}

# ---------------------------------------------------------------------------
# 24. FAT32 volume whose directory tree loops back on itself.
#
#     /A holds /A/B, and B's entry is then poked to name A's own first
#     cluster. Walking it without a record of what has been read would
#     re-enter A for ever. No mtools command produces this, which EXP-0006
#     measured, so the entry is poked.
#
#     ADR-0016 Decision E, section 11, condition 6.
# ---------------------------------------------------------------------------
fixture_fat32_directory_loop() {
    local path="$OUT_DIR/fat32-directory-loop.img"
    printf 'fat32-directory-loop.img\n'

    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0xfa730010
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c, bootable
EOF

    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    local img offset
    img="${path}@@${VBR_OFFSET}"

    MTOOLS_SKIP_CHECK=1 mmd -i "$img" ::/A
    MTOOLS_SKIP_CHECK=1 mmd -i "$img" ::/A/B

    expect_chain "$img" ::/A "::/A <3>"
    expect_chain "$img" ::/A/B "::/A/B <4>"

    # B's entry sits at slot 2 of A's own cluster, 3: . and .. take slots 0
    # and 1. The byte poked holds 4, B's first cluster, and becomes 3, A's.
    offset=$(entry_first_cluster_lo "$path" "$VBR_OFFSET" 3 2)
    poke_expecting "$path" "$offset" 04 3

    note "a live directory entry naming its own parent's first cluster"
}

# ---------------------------------------------------------------------------
# 25. FAT32 volume where two deleted entries share a slot and a cluster.
#
#     /A/X.TXT takes cluster 5 and is deleted; /B/Y.TXT then takes cluster 6,
#     because EXP-0006 measured that mtools does not reuse a freed cluster,
#     and is deleted too. Y's entry is poked to name cluster 5. Both entries
#     are then slot 2 of their own directory and name the same first cluster,
#     which is the collision ADR-0016 Decision F's name must survive: under
#     ADR-0015 Decision D both were slot-2-cluster-5.bin.
#
#     ADR-0016 section 11, condition 7.
# ---------------------------------------------------------------------------
fixture_fat32_slot_collision() {
    local path="$OUT_DIR/fat32-slot-collision.img"
    printf 'fat32-slot-collision.img\n'

    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0xfa730011
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c, bootable
EOF

    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    local work img offset
    work="$(mktemp -d)"
    img="${path}@@${VBR_OFFSET}"

    printf 'X.TXT\n' > "$work/x"
    printf 'Y.TXT\n' > "$work/y"

    MTOOLS_SKIP_CHECK=1 mmd -i "$img" ::/A
    MTOOLS_SKIP_CHECK=1 mmd -i "$img" ::/B
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/x" ::/A/X.TXT
    expect_chain "$img" ::/A/X.TXT "::/A/X.TXT <5>"
    MTOOLS_SKIP_CHECK=1 mdel -i "$img" ::/A/X.TXT

    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/y" ::/B/Y.TXT
    expect_chain "$img" ::/B/Y.TXT "::/B/Y.TXT <6>"
    MTOOLS_SKIP_CHECK=1 mdel -i "$img" ::/B/Y.TXT

    # Y's entry is slot 2 of B's cluster, 4. The byte poked holds 6, Y's own
    # first cluster, and becomes 5, the cluster X.TXT's content still fills.
    offset=$(entry_first_cluster_lo "$path" "$VBR_OFFSET" 4 2)
    poke_expecting "$path" "$offset" 06 5

    rm -rf "$work"

    note "two deleted entries at the same slot of different directories,"
    note "naming the same first cluster"
}

# ---------------------------------------------------------------------------
# 26 and 27. FAT32 volumes whose DCF tree was lost to a quick format.
#
#     EXP-0007's construction. DCIM takes cluster 3 and DCIM/100TAPH 4, and
#     the three files 5, 6 and 7 to 9. The volume is then formatted again by
#     the same mkfs.vfat invocation, which rewrites the boot sector, the FATs
#     and the root and nothing else. No byte is poked.
#
#     IMG_0001.JPG and IMG_0002.JPG hold their own names. IMG_0003.JPG holds
#     its own name a hundred times, 1,300 bytes, so its run spans three
#     clusters. EXP-0007 recorded each content's digest.
#
#     ADR-0017 section 10, conditions 1 to 3.
# ---------------------------------------------------------------------------
build_formatted_tree() {
    local path="$1" label_id="$2"

    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: ${label_id}
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c, bootable
EOF

    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    local work img i
    work="$(mktemp -d)"
    img="${path}@@${VBR_OFFSET}"

    printf 'IMG_0001.JPG\n' > "$work/1"
    printf 'IMG_0002.JPG\n' > "$work/2"
    for i in $(seq 100); do
        printf 'IMG_0003.JPG\n'
    done > "$work/3"

    MTOOLS_SKIP_CHECK=1 mmd -i "$img" ::/DCIM
    MTOOLS_SKIP_CHECK=1 mmd -i "$img" ::/DCIM/100TAPH
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/1" ::/DCIM/100TAPH/IMG_0001.JPG
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/2" ::/DCIM/100TAPH/IMG_0002.JPG
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/3" ::/DCIM/100TAPH/IMG_0003.JPG

    expect_chain "$img" ::/DCIM "::/DCIM <3>"
    expect_chain "$img" ::/DCIM/100TAPH "::/DCIM/100TAPH <4>"
    expect_chain "$img" ::/DCIM/100TAPH/IMG_0001.JPG \
        "::/DCIM/100TAPH/IMG_0001.JPG <5>"
    expect_chain "$img" ::/DCIM/100TAPH/IMG_0002.JPG \
        "::/DCIM/100TAPH/IMG_0002.JPG <6>"
    expect_chain "$img" ::/DCIM/100TAPH/IMG_0003.JPG \
        "::/DCIM/100TAPH/IMG_0003.JPG <7-9>"

    # The quick format: the same invocation, over the populated volume.
    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    rm -rf "$work"
}

fixture_fat32_formatted_tree() {
    local path="$OUT_DIR/fat32-formatted-tree.img"
    printf 'fat32-formatted-tree.img\n'

    build_formatted_tree "$path" 0xfa730012

    note "a DCF tree and three files that no surviving entry names"
}

# The next shot after the format: the camera recreates the tree on the
# lowest free clusters, 3 and 4, and writes one new file to 5. EXP-0007
# measured that the recreated directories are cleared, so nothing names the
# old IMG_0002.JPG or IMG_0003.JPG, whose bytes survive at 6 and 7 to 9.
fixture_fat32_formatted_reused() {
    local path="$OUT_DIR/fat32-formatted-reused.img"
    printf 'fat32-formatted-reused.img\n'

    build_formatted_tree "$path" 0xfa730013

    local work img
    work="$(mktemp -d)"
    img="${path}@@${VBR_OFFSET}"

    printf 'NEW_0001.JPG\n' > "$work/new"

    MTOOLS_SKIP_CHECK=1 mmd -i "$img" ::/DCIM
    MTOOLS_SKIP_CHECK=1 mmd -i "$img" ::/DCIM/100TAPH
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/new" ::/DCIM/100TAPH/IMG_0001.JPG

    expect_chain "$img" ::/DCIM "::/DCIM <3>"
    expect_chain "$img" ::/DCIM/100TAPH "::/DCIM/100TAPH <4>"
    expect_chain "$img" ::/DCIM/100TAPH/IMG_0001.JPG \
        "::/DCIM/100TAPH/IMG_0001.JPG <5>"

    rm -rf "$work"

    note "the same tree recreated over the old one, which nothing now names"
}

# ---------------------------------------------------------------------------
# 28. FAT32 volume holding a deleted tree 131 directories deep, one of whose
#     dot entries is broken.
#
#     Built as fixture 23 is, one level deeper, then removed with mdeltree,
#     which EXP-0005 measured marks every entry deleted and leaves . and ..
#     intact. Level n takes cluster n + 2, so the deepest is cluster 133.
#
#     The walk reads levels 1 to 128 and declines 129, cluster 131, at the
#     depth bound, though its first cluster still identifies itself. Level
#     130, cluster 132, is named only inside 131, so nothing read names it
#     and the orphan search finds it. It lists 131, cluster 133, whose .
#     entry is then poked to name cluster 0, so the search cannot find it.
#     ADR-0006 section 5.1 permits one field corrupted in a valid image.
#
#     ADR-0017 section 10, condition 6: the search does not read 131, which
#     the walk declined, and reports 133 as a directory it did not find.
# ---------------------------------------------------------------------------
fixture_fat32_deleted_nested() {
    local path="$OUT_DIR/fat32-deleted-nested.img"
    printf 'fat32-deleted-nested.img\n'

    blank_image "$path"

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0xfa730014
unit: sectors
${path}1 : start=${PART_START}, size=$((IMAGE_SECTORS - PART_START)), type=c, bootable
EOF

    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
        --offset="$PART_START" "$path" \
        $(( (IMAGE_SECTORS - PART_START) / 2 )) >/dev/null

    local img nested i offset
    img="${path}@@${VBR_OFFSET}"
    nested="::"

    for i in $(seq 1 131); do
        nested="$nested/D"
        MTOOLS_SKIP_CHECK=1 mmd -i "$img" "$nested"
    done

    expect_chain "$img" ::/D "::/D <3>"
    expect_chain "$img" "$nested" "$nested <133>"

    MTOOLS_SKIP_CHECK=1 mdeltree -i "$img" ::/D

    # Slot 0 of cluster 133 is its . entry, whose low cluster byte holds
    # 133. It becomes 0, so the entry no longer names the cluster it is in.
    offset=$(entry_first_cluster_lo "$path" "$VBR_OFFSET" 133 0)
    poke_expecting "$path" "$offset" 85 0

    note "a deleted tree deeper than 128 levels, its deepest . entry broken"
}

# ---------------------------------------------------------------------------
# 29. Two FAT32 partitions, each holding a deleted file at the same location.
#
#     Cluster numbers restart in every volume, so the directory cluster and
#     slot ADR-0016 Decision F names a written artifact from repeat across
#     partitions. Each partition holds one deleted file at root slot 1 with
#     first cluster 3, and the two files' contents differ, so a second
#     artifact refused or overwritten in place of the first is visible by
#     digest.
#
#     Each partition is the size of every single-partition fixture's volume,
#     so the image is twice as large rather than each volume half as large:
#     no volume of half that size has been measured here.
# ---------------------------------------------------------------------------
fixture_fat32_two_partitions() {
    local path="$OUT_DIR/fat32-two-partitions.img"
    printf 'fat32-two-partitions.img\n'

    local span=$(( IMAGE_SECTORS - PART_START ))
    local second=$(( PART_START + span ))

    blank_image "$path" $(( PART_START + span * 2 ))

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0xfa730015
unit: sectors
${path}1 : start=${PART_START}, size=${span}, type=c
${path}2 : start=${second}, size=${span}, type=c
EOF

    # The first volume prints "block count mismatch": mkfs.vfat sees the
    # second partition's sectors after it and is told to leave them alone.
    local start
    for start in "$PART_START" "$second"; do
        mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
            --offset="$start" "$path" $(( span / 2 )) >/dev/null
    done

    local work img
    work="$(mktemp -d)"
    printf 'FIRST.TXT\n'  > "$work/first"
    printf 'SECOND.TXT\n' > "$work/second"

    img="${path}@@${VBR_OFFSET}"
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/first" ::/FIRST.TXT
    expect_chain "$img" ::/FIRST.TXT "::/FIRST.TXT <3>"
    MTOOLS_SKIP_CHECK=1 mdel -i "$img" ::/FIRST.TXT

    img="${path}@@$(( second * SECTOR ))"
    MTOOLS_SKIP_CHECK=1 mcopy -i "$img" "$work/second" ::/SECOND.TXT
    expect_chain "$img" ::/SECOND.TXT "::/SECOND.TXT <3>"
    MTOOLS_SKIP_CHECK=1 mdel -i "$img" ::/SECOND.TXT

    rm -rf "$work"

    note "two FAT32 partitions, each with a deleted file at root slot 1,"
    note "first cluster 3"
}

# ---------------------------------------------------------------------------
# 30. Truncated image: two whole FAT32 partitions and a third cut short.
#
#     The shape an image of a failing disk takes when acquisition stops
#     early: the table is intact and its last partition runs past the end of
#     what was read. The table is written for the whole disk and the image
#     is then truncated half way through partition 3, so no byte of the
#     table is poked. Partition 3 is never formatted, because nothing may
#     read it.
#
#     Partitions 1 and 2 must still be listed and analysed, and partition 3
#     reported as an anomaly and a coverage gap rather than the whole table
#     being refused. ADR-0005 section 3.3.
# ---------------------------------------------------------------------------
fixture_mbr_truncated_last_partition() {
    local path="$OUT_DIR/mbr-truncated-last-partition.img"
    printf 'mbr-truncated-last-partition.img\n'

    local span=$(( IMAGE_SECTORS - PART_START ))
    local second=$(( PART_START + span ))
    local third=$(( PART_START + span * 2 ))
    local third_size=2048

    blank_image "$path" $(( third + third_size ))

    sfdisk --quiet --no-tell-kernel "$path" >/dev/null <<EOF
label: dos
label-id: 0xfa730016
unit: sectors
${path}1 : start=${PART_START}, size=${span}, type=c
${path}2 : start=${second}, size=${span}, type=c
${path}3 : start=${third}, size=${third_size}, type=c
EOF

    # Both volumes print "block count mismatch", for the reason fixture 29
    # gives.
    local start
    for start in "$PART_START" "$second"; do
        mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" \
            --offset="$start" "$path" $(( span / 2 )) >/dev/null
    done

    truncate -s $(( (third + third_size / 2) * SECTOR )) "$path"

    note "partitions 1 and 2 whole FAT32, partition 3 cut off half way"
    note "by the end of the image"
}

# ---------------------------------------------------------------------------
# A FAT32 volume written over the whole image, with no partition table.
#
#     mkfs.vfat leaves bytes 446 to 509 of sector 0 zero, so sector 0 is a
#     volume boot record and also parses as an empty partition table. That
#     is what ADR-0024's classification exists to tell apart. --mbr=n is
#     given because mkfs.vfat's default can write a table over that area,
#     which would make this image ambiguous.
#
#     GONE.TXT is the only file: forty lines of forty bytes (four clusters),
#     written and deleted, so nothing is reused. A test asserting its
#     recovered bytes rebuilds them from the same loop.
# ---------------------------------------------------------------------------
fixture_fat32_no_partition_table() {
    local path="$OUT_DIR/fat32-no-partition-table.img"
    printf 'fat32-no-partition-table.img\n'
    blank_image "$path"

    mkfs.vfat --invariant --mbr=n -F 32 -n "$FAT32_LABEL" "$path" >/dev/null

    local work i
    work="$(mktemp -d)"
    : > "$work/gone.txt"
    for i in $(seq 1 40); do
        printf 'taphonomy no-partition-table line %03d\n' "$i" >> "$work/gone.txt"
    done

    MTOOLS_SKIP_CHECK=1 mcopy -i "$path" "$work/gone.txt" ::/GONE.TXT
    MTOOLS_SKIP_CHECK=1 mdel -i "$path" ::/GONE.TXT

    rm -rf "$work"

    note "a whole-image FAT32 volume, one four-cluster file written and deleted"
}

# ---------------------------------------------------------------------------

printf 'Generating fixtures in %s\n\n' "$OUT_DIR"

fixture_single_fat32
fixture_four_partitions
fixture_empty_table
fixture_no_signature
fixture_bad_signature
fixture_partition_beyond_end
fixture_gpt_protective
fixture_type_mismatch
fixture_fat32_oversized_volume
fixture_fat32_hidden_mismatch
fixture_fat32_bad_root_cluster
fixture_fat32_undersized_fat
fixture_fat32_root_entries
fixture_fat32_root_multicluster
fixture_fat32_deleted_entries
fixture_fat32_deleted_residue
fixture_fat32_recover_run
fixture_fat32_recover_collision
fixture_fat32_no_backup_boot_sector
fixture_fat32_fragmented_deleted
fixture_fat32_fragmented_live_gap
fixture_fat32_deleted_subtree
fixture_fat32_deleted_split_directory
fixture_fat32_nested_directories
fixture_fat32_directory_loop
fixture_fat32_slot_collision
fixture_fat32_formatted_tree
fixture_fat32_formatted_reused
fixture_fat32_deleted_nested
fixture_fat32_two_partitions
fixture_mbr_truncated_last_partition
fixture_fat32_no_partition_table

# ---------------------------------------------------------------------------
# The two exFAT images EXP-0009 measured.
#
# Windows made them, and nothing in this repository can regenerate them, so
# unlike every fixture above they are committed, compressed, under
# fixtures/exfat. They are expanded here byte for byte; no byte is poked.
# Their digests are in the manifest below, which is what ties the expanded
# image to the one the experiment measured.
# ---------------------------------------------------------------------------
EXFAT_DIR="$(dirname "${BASH_SOURCE[0]}")/../fixtures/exfat"
readonly EXFAT_DIR

for name in exp9-A exp9-B; do
    printf '%s.vhd\n' "$name"
    xz -dc "$EXFAT_DIR/$name.vhd.xz" > "$OUT_DIR/$name.vhd"
    note "expanded from fixtures/exfat/$name.vhd.xz"
done

# ---------------------------------------------------------------------------
# Manifest
#
# Records the SHA-256 of every fixture. A fixture whose digest changes has
# changed shape, and any test asserting against it is no longer testing what
# it was written to test.
# ---------------------------------------------------------------------------

MANIFEST="$OUT_DIR/MANIFEST.sha256"
( cd "$OUT_DIR" && sha256sum ./*.img ./*.vhd > "$(basename "$MANIFEST")" )

printf '\nManifest written to %s\n' "$MANIFEST"
printf '\n'
cat "$MANIFEST"
