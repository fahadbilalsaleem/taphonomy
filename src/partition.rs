//! MBR partition table parsing.
//!
//! Scope is MBR only, per ADR-0005. GPT is detected and reported as
//! unsupported rather than misparsed.
//!
//! # Parsing discipline
//!
//! `DEVELOPMENT_ENVIRONMENT.md` section 18 requires that code parsing
//! external data assume the input may be truncated, corrupted, internally
//! contradictory, or deliberately hostile. Every declared offset and length
//! here is validated against the actual size of the evidence. No declared
//! value is trusted.
//!
//! # Invalid versus anomalous
//!
//! The parser distinguishes two things:
//!
//! * **Invalid** — the structure cannot be interpreted. Parsing fails.
//! * **Anomalous** — the structure is interpretable but unusual. Parsing
//!   succeeds and the observation is reported.
//!
//! Damaged evidence is frequently anomalous. Refusing to parse it would make
//! the tool useless for its purpose. Silently normalising it would hide
//! findings. Both are recorded, and the caller decides.

use std::fmt;

use crate::filesystem::{Filesystem, identify};
use crate::gpt::HeaderFailure;

/// Bytes per sector. Fixed at 512 for now; 4Kn media requires a decision
/// that has not been made.
pub const SECTOR_SIZE: usize = 512;

/// Offset of the first partition entry within the first sector.
pub const TABLE_OFFSET: usize = 0x1BE;

/// Offset of the two-byte boot signature.
pub const SIGNATURE_OFFSET: usize = 0x1FE;

/// Bytes per partition entry.
pub const ENTRY_SIZE: usize = 16;

/// Number of primary partition entries.
pub const ENTRY_COUNT: usize = 4;

/// Required boot signature value.
pub const SIGNATURE: [u8; 2] = [0x55, 0xAA];

/// Partition type marking a GPT protective MBR.
pub const TYPE_GPT_PROTECTIVE: u8 = 0xEE;

/// Partition type marking an unused entry.
pub const TYPE_UNUSED: u8 = 0x00;

/// Offset of the disk signature within the first sector.
const DISK_SIGNATURE_OFFSET: usize = 0x1B8;

/// A single primary partition entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MbrPartition {
    /// Entry position, 1 to 4.
    pub index: u8,
    /// Boot indicator byte, as read.
    pub boot_indicator: u8,
    /// Partition type byte.
    pub partition_type: u8,
    /// First sector of the partition, LBA.
    pub start_lba: u32,
    /// Length of the partition in sectors.
    pub sector_count: u32,
}

impl MbrPartition {
    /// Whether the boot indicator marks this partition active.
    pub const fn is_bootable(&self) -> bool {
        self.boot_indicator == 0x80
    }

    /// Byte offset of the partition's first sector.
    pub const fn start_byte(&self) -> u64 {
        self.start_lba as u64 * SECTOR_SIZE as u64
    }

    /// Length of the partition in bytes.
    pub const fn length_bytes(&self) -> u64 {
        self.sector_count as u64 * SECTOR_SIZE as u64
    }

    /// One past the partition's last sector, LBA.
    ///
    /// Returns `None` on arithmetic overflow, which itself indicates a
    /// malformed entry.
    pub const fn end_lba(&self) -> Option<u64> {
        (self.start_lba as u64).checked_add(self.sector_count as u64)
    }
}

/// What the first sector was found to contain.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PartitionTable {
    /// A conventional MBR partition table.
    Mbr {
        /// Disk signature at offset 0x1B8.
        disk_signature: u32,
        /// Entries with a non-zero partition type whose extent the evidence
        /// holds, in table order. An entry with no sectors or reaching past
        /// the evidence is left out and reported as an [`Anomaly`].
        partitions: Vec<MbrPartition>,
    },
    /// A GPT protective MBR.
    ///
    /// The disk uses GPT. Its real partition table is at LBA 1 and is read by
    /// [`crate::gpt`]. Reporting the protective entry as a real partition
    /// would be a false positive; see ADR-0005 section 4.
    GptProtective,
}

/// An interpretable but unusual observation.
///
/// Media-specific conformance checks (for example, SD Association layout
/// requirements) are deferred until Taphonomy can identify the device an
/// image came from. See ADR-0005 section 3.3.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Anomaly {
    /// Boot indicator is neither 0x00 nor 0x80.
    InvalidBootIndicator {
        index: u8,
        value: u8,
    },
    /// More than one partition is marked bootable.
    MultipleBootablePartitions {
        count: usize,
    },
    /// Two partitions occupy overlapping sectors.
    OverlappingPartitions {
        first: u8,
        second: u8,
    },
    /// An entry is marked unused but is not zero-filled.
    NonZeroUnusedEntry {
        index: u8,
    },
    /// Entries are not in ascending start order.
    EntriesOutOfOrder,
    /// A partition claims sectors beyond the end of the evidence. The entry
    /// is left out of the table.
    PartitionBeyondEnd {
        index: u8,
        start_lba: u32,
        sector_count: u32,
        image_sectors: u64,
    },
    /// A partition has a type but occupies no sectors. The entry is left
    /// out of the table.
    ZeroLengthPartition {
        index: u8,
        partition_type: u8,
    },
    /// A protective record whose StartingLBA is not 1. `ADR-0025` Decision A.
    ProtectiveStartNotOne {
        start_lba: u32,
    },
    /// A protective record whose size is neither the image's sectors minus
    /// one nor `0xFFFFFFFF`.
    ProtectiveSizeUnexpected {
        size: u32,
        expected: u32,
    },
    /// A record other than the protective one is in use beside it, so the
    /// MBR is a hybrid; it is not analysed as a partition.
    HybridMbr {
        index: u8,
        partition_type: u8,
    },
    /// A valid GPT header lies at LBA 1 behind a legacy MBR and is not
    /// honoured (UEFI 5.3.2). `ADR-0025` Decision B.
    GptBehindLegacyMbr,
    /// Sector 1, or the entry array a header there names, could not be read,
    /// so whether a GPT lies behind the legacy MBR is not known.
    GptCheckNotMade {
        reason: String,
    },
    /// `ADR-0025` Decisions C to E, reported by [`crate::gpt`].
    GptRevision {
        revision: u32,
    },
    GptReservedNonZero,
    GptUsableBeyondEnd {
        last_usable: u64,
        image_sectors: u64,
    },
    GptBackupMissing {
        alternate_lba: u64,
    },
    GptBackupInvalid {
        lba: u64,
        reason: HeaderFailure,
    },
    GptBackupNotAtLastLba {
        alternate_lba: u64,
        last_lba: u64,
    },
    GptHeadersDisagree {
        field: &'static str,
    },
    GptUnusedEntryNotZero {
        number: u16,
    },
    /// Left out of the table, like the three after it.
    GptEntryStartsAfterEnd {
        number: u16,
        start: u64,
        end: u64,
    },
    GptEntryBeyondEnd {
        number: u16,
        start: u64,
        end: u64,
        image_sectors: u64,
    },
    GptEntryTooLarge {
        number: u16,
        start: u64,
        end: u64,
    },
    GptEntryOutsideUsable {
        number: u16,
        start: u64,
        end: u64,
        first_usable: u64,
        last_usable: u64,
    },
    GptOverlappingEntries {
        first: u16,
        second: u16,
    },
}

impl Anomaly {
    /// Whether the anomaly is a partition declared and not analysed: a gap
    /// in the run, not only a remark on the table.
    pub const fn leaves_partition_out(&self) -> bool {
        matches!(
            self,
            Anomaly::PartitionBeyondEnd { .. }
                | Anomaly::ZeroLengthPartition { .. }
                | Anomaly::GptEntryStartsAfterEnd { .. }
                | Anomaly::GptEntryBeyondEnd { .. }
                | Anomaly::GptEntryTooLarge { .. }
        )
    }
}

impl fmt::Display for Anomaly {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Anomaly::ProtectiveStartNotOne { start_lba } => write!(
                f,
                "protective MBR record starts at sector {start_lba}, expected 1"
            ),
            Anomaly::ProtectiveSizeUnexpected { size, expected } => write!(
                f,
                "protective MBR record covers {size} sectors, expected {expected} or 4294967295"
            ),
            Anomaly::HybridMbr {
                index,
                partition_type,
            } => write!(
                f,
                "hybrid MBR: record {index} of type {partition_type:#04x} beside the protective record, not analysed as a partition"
            ),
            Anomaly::GptBehindLegacyMbr => {
                f.write_str("a GPT header lies at sector 1 behind a legacy MBR and is not honoured")
            }
            Anomaly::GptCheckNotMade { reason } => write!(
                f,
                "sector 1 or the GPT entry array it names could not be read: the check for a GPT behind a legacy MBR was not made ({reason})"
            ),
            Anomaly::GptRevision { revision } => {
                write!(f, "GPT revision is {revision:#010x}, expected 0x00010000")
            }
            Anomaly::GptReservedNonZero => f.write_str("GPT header reserved bytes are not zero"),
            Anomaly::GptUsableBeyondEnd {
                last_usable,
                image_sectors,
            } => write!(
                f,
                "GPT usable range ends at sector {last_usable} but evidence has {image_sectors} sectors"
            ),
            Anomaly::GptBackupMissing { alternate_lba } => write!(
                f,
                "GPT primary header names its backup at sector {alternate_lba}, which is not a sector of the evidence that can hold one"
            ),
            Anomaly::GptBackupInvalid { lba, reason } => {
                write!(f, "GPT backup header at sector {lba} is invalid: {reason}")
            }
            Anomaly::GptBackupNotAtLastLba {
                alternate_lba,
                last_lba,
            } => write!(
                f,
                "GPT primary header names its backup at sector {alternate_lba}, not the last sector {last_lba}"
            ),
            Anomaly::GptHeadersDisagree { field } => {
                write!(f, "GPT primary and backup headers disagree in {field}")
            }
            Anomaly::GptUnusedEntryNotZero { number } => {
                write!(f, "GPT entry {number} is unused but is not zero-filled")
            }
            Anomaly::GptEntryStartsAfterEnd { number, start, end } => write!(
                f,
                "GPT entry {number} starts at sector {start} after it ends at {end}; left out"
            ),
            Anomaly::GptEntryBeyondEnd {
                number,
                start,
                end,
                image_sectors,
            } => write!(
                f,
                "GPT entry {number} claims sectors {start}..={end} but evidence has {image_sectors} sectors; left out"
            ),
            Anomaly::GptEntryTooLarge { number, start, end } => write!(
                f,
                "GPT entry {number} (sectors {start}..={end}) is beyond what the analysis counts; left out"
            ),
            Anomaly::GptEntryOutsideUsable {
                number,
                start,
                end,
                first_usable,
                last_usable,
            } => write!(
                f,
                "GPT entry {number} (sectors {start}..={end}) is outside the usable range {first_usable}..={last_usable}"
            ),
            Anomaly::GptOverlappingEntries { first, second } => {
                write!(
                    f,
                    "GPT entries {first} and {second} occupy overlapping sectors"
                )
            }
            Anomaly::InvalidBootIndicator { index, value } => write!(
                f,
                "partition {index} boot indicator is {value:#04x}, expected 0x00 or 0x80"
            ),
            Anomaly::MultipleBootablePartitions { count } => write!(
                f,
                "{count} partitions are marked bootable, expected at most 1"
            ),
            Anomaly::OverlappingPartitions { first, second } => write!(
                f,
                "partitions {first} and {second} occupy overlapping sectors"
            ),
            Anomaly::NonZeroUnusedEntry { index } => write!(
                f,
                "partition entry {index} is marked unused but is not zero-filled"
            ),
            Anomaly::EntriesOutOfOrder => {
                f.write_str("partition entries are not in ascending start order")
            }
            Anomaly::PartitionBeyondEnd {
                index,
                start_lba,
                sector_count,
                image_sectors,
            } => write!(
                f,
                "partition {index} claims sectors {start_lba}..{} but evidence has {image_sectors} sectors",
                *start_lba as u64 + *sector_count as u64
            ),
            Anomaly::ZeroLengthPartition {
                index,
                partition_type,
            } => write!(
                f,
                "partition {index} has type {partition_type:#04x} but zero sectors"
            ),
        }
    }
}

/// A reason the first sector could not be interpreted.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ParseError {
    /// Fewer bytes were supplied than a sector contains.
    ShortSector { supplied: usize },
    /// The boot signature is absent or wrong.
    MissingSignature { found: [u8; 2] },
    /// A partition's start and length overflow when added.
    LengthOverflow {
        index: u8,
        start_lba: u32,
        sector_count: u32,
    },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::ShortSector { supplied } => write!(
                f,
                "first sector truncated: {supplied} bytes supplied, {SECTOR_SIZE} required"
            ),
            ParseError::MissingSignature { found } => write!(
                f,
                "no MBR signature at offset 0x1FE: found {:02x}{:02x}, expected 55aa",
                found[0], found[1]
            ),
            ParseError::LengthOverflow {
                index,
                start_lba,
                sector_count,
            } => write!(
                f,
                "partition {index} length overflows: start {start_lba} plus {sector_count} sectors"
            ),
        }
    }
}

impl std::error::Error for ParseError {}

/// The outcome of parsing a first sector.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParseOutcome {
    /// What the sector was found to contain.
    pub table: PartitionTable,
    /// Interpretable but unusual observations.
    pub anomalies: Vec<Anomaly>,
}

/// What the first sector of an image is, before it is parsed as a table.
///
/// A volume written at sector 0 with no partition table leaves the bytes
/// where a table's entries would be zero (`mkfs.vfat`, `mkfs.exfat`) or
/// holding boot code (Windows), so reading it as a table either reports an
/// empty table or fails. This follows util-linux `libblkid`'s order, which
/// classified every image measured for `ADR-0024` correctly; the kernel's
/// boot-indicator test alone and The Sleuth Kit's OEM-name test each call
/// every partitionless volume a table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SectorZero {
    /// A partition table, or a sector with no 0x55AA signature: parsed by
    /// [`parse_mbr`], which reports the second as an error.
    Table,
    /// A volume boot record of this filesystem, with no partition table.
    VolumeBootRecord(Filesystem),
    /// Not a table, and not a volume boot record either.
    Neither,
    /// A volume boot record whose table area also lists used entries. Which
    /// it is cannot be told, so neither is read.
    Ambiguous {
        filesystem: Filesystem,
        used_entries: usize,
    },
}

/// Classifies the first sector of an image.
///
/// Performs no I/O. In order: no signature, a table (the existing path); a
/// boot indicator other than 0x00 or 0x80, not a table; a sector that
/// identifies as a volume boot record is a volume when no entry is used and
/// ambiguous otherwise; anything else is a table. An entry is used when its
/// type and its sector count are both non-zero.
pub fn classify_sector_zero(sector: &[u8]) -> SectorZero {
    if sector.len() < SECTOR_SIZE || sector[SIGNATURE_OFFSET..SIGNATURE_OFFSET + 2] != SIGNATURE {
        return SectorZero::Table;
    }

    let entries =
        || (0..ENTRY_COUNT).map(|slot| &sector[TABLE_OFFSET + slot * ENTRY_SIZE..][..ENTRY_SIZE]);

    let boot_record = identify(sector).filesystem();

    // `ADR-0025` Decision A: UEFI 5.2.3 says a protective record's boot
    // indicator "Must be ignored", so a 0xEE record makes this a table
    // whatever the other records' indicators say. Only where the sector is
    // no boot record: the type offsets lie inside a volume's boot code, so a
    // 0xEE there on a FAT32 or exFAT boot record leaves it ambiguous, as a
    // boot record listing used entries is (`ADR-0024`), the 0xEE record
    // counted as used.
    if entries().any(|e| e[4] == TYPE_GPT_PROTECTIVE) {
        return match boot_record {
            None => SectorZero::Table,
            Some(filesystem) => SectorZero::Ambiguous {
                filesystem,
                used_entries: entries()
                    .filter(|e| {
                        e[4] == TYPE_GPT_PROTECTIVE || (e[4] != TYPE_UNUSED && e[12..16] != [0; 4])
                    })
                    .count(),
            },
        };
    }

    let indicators_valid = entries().all(|e| e[0] == 0x00 || e[0] == 0x80);

    if !indicators_valid {
        return boot_record.map_or(SectorZero::Neither, SectorZero::VolumeBootRecord);
    }

    let Some(filesystem) = boot_record else {
        return SectorZero::Table;
    };
    let used_entries = entries()
        .filter(|e| e[4] != TYPE_UNUSED && e[12..16] != [0; 4])
        .count();
    if used_entries == 0 {
        SectorZero::VolumeBootRecord(filesystem)
    } else {
        SectorZero::Ambiguous {
            filesystem,
            used_entries,
        }
    }
}

/// A GPT protective MBR: the real table is at LBA 1, so no record is reported
/// as a partition, which would be a false positive (`ADR-0005` section 4).
/// `ADR-0025` Decision A: the protective record is checked against what UEFI
/// 5.2.3 requires of it, and any other record in use is a hybrid-MBR anomaly.
fn protective_outcome(sector: &[u8], protective: usize, image_sectors: u64) -> ParseOutcome {
    let mut anomalies = Vec::new();

    for slot in 0..ENTRY_COUNT {
        let base = TABLE_OFFSET + slot * ENTRY_SIZE;
        let entry = &sector[base..base + ENTRY_SIZE];
        let index = (slot + 1) as u8;
        let start_lba = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]);
        let size = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]);

        if slot == protective {
            if start_lba != 1 {
                anomalies.push(Anomaly::ProtectiveStartNotOne { start_lba });
            }
            let expected = u32::try_from(image_sectors.saturating_sub(1)).unwrap_or(u32::MAX);
            if size != expected && size != u32::MAX {
                anomalies.push(Anomaly::ProtectiveSizeUnexpected { size, expected });
            }
        } else if entry[4] != TYPE_UNUSED {
            anomalies.push(Anomaly::HybridMbr {
                index,
                partition_type: entry[4],
            });
        } else if entry.iter().any(|&b| b != 0) {
            anomalies.push(Anomaly::NonZeroUnusedEntry { index });
        }
    }

    ParseOutcome {
        table: PartitionTable::GptProtective,
        anomalies,
    }
}

/// Parses an MBR from the first sector of evidence.
///
/// `image_sectors` is the true size of the evidence in sectors. Every
/// declared partition extent is checked against it. Passing a value not
/// derived from the evidence defeats the bounds checking.
///
/// This function performs no I/O. It is a pure function of its inputs, which
/// makes every failure path testable without a filesystem.
pub fn parse_mbr(sector: &[u8], image_sectors: u64) -> Result<ParseOutcome, ParseError> {
    if sector.len() < SECTOR_SIZE {
        return Err(ParseError::ShortSector {
            supplied: sector.len(),
        });
    }

    let signature = [sector[SIGNATURE_OFFSET], sector[SIGNATURE_OFFSET + 1]];
    if signature != SIGNATURE {
        return Err(ParseError::MissingSignature { found: signature });
    }

    let disk_signature = u32::from_le_bytes([
        sector[DISK_SIGNATURE_OFFSET],
        sector[DISK_SIGNATURE_OFFSET + 1],
        sector[DISK_SIGNATURE_OFFSET + 2],
        sector[DISK_SIGNATURE_OFFSET + 3],
    ]);

    let mut anomalies = Vec::new();
    let mut partitions = Vec::new();

    if let Some(protective) = (0..ENTRY_COUNT)
        .find(|&slot| sector[TABLE_OFFSET + slot * ENTRY_SIZE + 4] == TYPE_GPT_PROTECTIVE)
    {
        return Ok(protective_outcome(sector, protective, image_sectors));
    }

    for slot in 0..ENTRY_COUNT {
        let base = TABLE_OFFSET + slot * ENTRY_SIZE;
        let entry = &sector[base..base + ENTRY_SIZE];
        let index = (slot + 1) as u8;

        let boot_indicator = entry[0];
        let partition_type = entry[4];
        let start_lba = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]);
        let sector_count = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]);

        if partition_type == TYPE_UNUSED {
            if entry.iter().any(|&b| b != 0) {
                anomalies.push(Anomaly::NonZeroUnusedEntry { index });
            }
            continue;
        }

        // One malformed entry leaves the other three interpretable, and a
        // truncated image of a failing disk cuts off the last partition in
        // exactly this way. ADR-0005 section 3.3: an anomaly is reported,
        // not made an error. The entry is not listed, so nothing analyses
        // an extent the evidence does not hold.
        if sector_count == 0 {
            anomalies.push(Anomaly::ZeroLengthPartition {
                index,
                partition_type,
            });
            continue;
        }

        let end = (start_lba as u64).checked_add(sector_count as u64).ok_or(
            ParseError::LengthOverflow {
                index,
                start_lba,
                sector_count,
            },
        )?;

        if end > image_sectors {
            anomalies.push(Anomaly::PartitionBeyondEnd {
                index,
                start_lba,
                sector_count,
                image_sectors,
            });
            continue;
        }

        if boot_indicator != 0x00 && boot_indicator != 0x80 {
            anomalies.push(Anomaly::InvalidBootIndicator {
                index,
                value: boot_indicator,
            });
        }

        partitions.push(MbrPartition {
            index,
            boot_indicator,
            partition_type,
            start_lba,
            sector_count,
        });
    }

    let bootable = partitions.iter().filter(|p| p.is_bootable()).count();
    if bootable > 1 {
        anomalies.push(Anomaly::MultipleBootablePartitions { count: bootable });
    }

    for i in 0..partitions.len() {
        for j in (i + 1)..partitions.len() {
            let a = &partitions[i];
            let b = &partitions[j];
            let (Some(a_end), Some(b_end)) = (a.end_lba(), b.end_lba()) else {
                continue;
            };
            if (a.start_lba as u64) < b_end && (b.start_lba as u64) < a_end {
                anomalies.push(Anomaly::OverlappingPartitions {
                    first: a.index,
                    second: b.index,
                });
            }
        }
    }

    if partitions
        .windows(2)
        .any(|w| w[0].start_lba > w[1].start_lba)
    {
        anomalies.push(Anomaly::EntriesOutOfOrder);
    }

    Ok(ParseOutcome {
        table: PartitionTable::Mbr {
            disk_signature,
            partitions,
        },
        anomalies,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a sector with a valid signature and no entries.
    fn blank_sector() -> Vec<u8> {
        let mut s = vec![0u8; SECTOR_SIZE];
        s[SIGNATURE_OFFSET] = SIGNATURE[0];
        s[SIGNATURE_OFFSET + 1] = SIGNATURE[1];
        s
    }

    fn write_entry(sector: &mut [u8], slot: usize, boot: u8, ptype: u8, start: u32, count: u32) {
        let base = TABLE_OFFSET + slot * ENTRY_SIZE;
        sector[base] = boot;
        sector[base + 4] = ptype;
        sector[base + 8..base + 12].copy_from_slice(&start.to_le_bytes());
        sector[base + 12..base + 16].copy_from_slice(&count.to_le_bytes());
    }

    /// A FAT32 boot sector as `mkfs.vfat` writes it: bytes 446 to 509 zero.
    fn fat32_boot_record() -> Vec<u8> {
        crate::fat::tests::fat32_sector()
    }

    fn exfat_boot_record() -> Vec<u8> {
        let mut s = fat32_boot_record();
        s[3..11].copy_from_slice(b"EXFAT   ");
        s
    }

    #[test]
    fn zero_entry_area_and_a_fat32_boot_record_is_a_volume() {
        assert_eq!(
            classify_sector_zero(&fat32_boot_record()),
            SectorZero::VolumeBootRecord(Filesystem::Fat32)
        );
    }

    #[test]
    fn zero_entry_area_and_an_exfat_boot_record_is_a_volume() {
        assert_eq!(
            classify_sector_zero(&exfat_boot_record()),
            SectorZero::VolumeBootRecord(Filesystem::ExFat)
        );
    }

    #[test]
    fn boot_code_in_the_entry_area_and_an_exfat_boot_record_is_a_volume() {
        // Windows writes boot code over the table area, so a boot indicator
        // other than 0x00 and 0x80 appears, and its bytes also look like
        // used entries: only the indicator test tells this from ambiguous.
        let mut s = exfat_boot_record();
        s[TABLE_OFFSET..TABLE_OFFSET + ENTRY_COUNT * ENTRY_SIZE].fill(0x33);
        assert_eq!(
            classify_sector_zero(&s),
            SectorZero::VolumeBootRecord(Filesystem::ExFat)
        );
    }

    #[test]
    fn a_bad_boot_indicator_and_no_boot_record_is_neither() {
        let mut s = blank_sector();
        s[TABLE_OFFSET + ENTRY_SIZE] = 0x33;
        assert_eq!(classify_sector_zero(&s), SectorZero::Neither);
    }

    #[test]
    fn a_boot_record_listing_a_used_entry_is_ambiguous() {
        let mut s = fat32_boot_record();
        write_entry(&mut s, 0, 0, 0x0c, 2048, 1000);
        assert_eq!(
            classify_sector_zero(&s),
            SectorZero::Ambiguous {
                filesystem: Filesystem::Fat32,
                used_entries: 1
            }
        );
    }

    #[test]
    fn an_entry_with_a_type_and_no_sectors_or_sectors_and_no_type_is_not_used() {
        let mut s = fat32_boot_record();
        write_entry(&mut s, 0, 0, 0x0c, 2048, 0);
        write_entry(&mut s, 1, 0, 0x00, 2048, 1000);
        assert_eq!(
            classify_sector_zero(&s),
            SectorZero::VolumeBootRecord(Filesystem::Fat32)
        );
    }

    #[test]
    fn an_mbr_with_grub_style_jump_code_and_a_used_entry_is_a_table() {
        let mut s = blank_sector();
        s[..3].copy_from_slice(&[0xEB, 0x63, 0x90]);
        write_entry(&mut s, 0, 0x80, 0x83, 2048, 1000);
        assert_eq!(classify_sector_zero(&s), SectorZero::Table);
    }

    #[test]
    fn all_zero_entries_and_no_boot_record_is_a_table() {
        assert_eq!(classify_sector_zero(&blank_sector()), SectorZero::Table);
    }

    #[test]
    fn a_sector_with_no_signature_is_left_to_the_table_parser() {
        assert_eq!(classify_sector_zero(&[0u8; SECTOR_SIZE]), SectorZero::Table);
    }

    #[test]
    fn short_sector_is_rejected() {
        let err = parse_mbr(&[0u8; 511], 1000).unwrap_err();
        assert_eq!(err, ParseError::ShortSector { supplied: 511 });
    }

    #[test]
    fn missing_signature_is_rejected() {
        let sector = vec![0u8; SECTOR_SIZE];
        let err = parse_mbr(&sector, 1000).unwrap_err();
        assert_eq!(err, ParseError::MissingSignature { found: [0, 0] });
    }

    #[test]
    fn empty_table_is_valid() {
        let outcome = parse_mbr(&blank_sector(), 1000).expect("empty table is valid");
        match outcome.table {
            PartitionTable::Mbr { partitions, .. } => assert!(partitions.is_empty()),
            other => panic!("expected Mbr, got {other:?}"),
        }
    }

    #[test]
    fn single_partition_is_parsed() {
        let mut s = blank_sector();
        write_entry(&mut s, 0, 0x80, 0x0c, 2048, 129_024);

        let outcome = parse_mbr(&s, 131_072).expect("valid single partition");
        let PartitionTable::Mbr { partitions, .. } = outcome.table else {
            panic!("expected Mbr");
        };

        assert_eq!(partitions.len(), 1);
        let p = partitions[0];
        assert_eq!(p.index, 1);
        assert!(p.is_bootable());
        assert_eq!(p.partition_type, 0x0c);
        assert_eq!(p.start_lba, 2048);
        assert_eq!(p.sector_count, 129_024);
        assert_eq!(p.start_byte(), 1_048_576);
    }

    /// The partitions an outcome lists.
    fn listed(outcome: &ParseOutcome) -> &[MbrPartition] {
        match &outcome.table {
            PartitionTable::Mbr { partitions, .. } => partitions,
            other => panic!("expected Mbr, got {other:?}"),
        }
    }

    #[test]
    fn partition_beyond_end_is_dropped_as_an_anomaly() {
        let mut s = blank_sector();
        write_entry(&mut s, 0, 0x00, 0x0c, 2048, u32::MAX);

        let outcome = parse_mbr(&s, 131_072).expect("a bad entry is not a bad table");
        assert!(listed(&outcome).is_empty());
        assert!(matches!(
            outcome.anomalies.as_slice(),
            [Anomaly::PartitionBeyondEnd { index: 1, .. }]
        ));
    }

    #[test]
    fn exact_fit_is_accepted() {
        let mut s = blank_sector();
        write_entry(&mut s, 0, 0x00, 0x0c, 0, 1000);
        parse_mbr(&s, 1000).expect("a partition filling the evidence exactly is valid");
    }

    #[test]
    fn one_sector_over_is_dropped_as_an_anomaly() {
        let mut s = blank_sector();
        write_entry(&mut s, 0, 0x00, 0x0c, 0, 1001);
        let outcome = parse_mbr(&s, 1000).expect("a bad entry is not a bad table");
        assert!(listed(&outcome).is_empty());
        assert!(matches!(
            outcome.anomalies.as_slice(),
            [Anomaly::PartitionBeyondEnd { .. }]
        ));
    }

    #[test]
    fn zero_length_partition_is_dropped_as_an_anomaly() {
        let mut s = blank_sector();
        write_entry(&mut s, 0, 0x00, 0x0c, 2048, 0);
        let outcome = parse_mbr(&s, 131_072).expect("a bad entry is not a bad table");
        assert!(listed(&outcome).is_empty());
        assert!(matches!(
            outcome.anomalies.as_slice(),
            [Anomaly::ZeroLengthPartition { index: 1, .. }]
        ));
    }

    /// The case the change exists for: one bad entry among good ones leaves
    /// the good ones listed, in table order.
    #[test]
    fn a_bad_entry_leaves_the_others_listed() {
        let mut s = blank_sector();
        write_entry(&mut s, 0, 0x00, 0x0c, 2048, 1000);
        write_entry(&mut s, 1, 0x00, 0x0c, 3048, 0);
        write_entry(&mut s, 2, 0x00, 0x0c, 4048, 1000);
        write_entry(&mut s, 3, 0x00, 0x0c, 5048, 200_000);

        let outcome = parse_mbr(&s, 131_072).expect("a bad entry is not a bad table");
        let indices: Vec<u8> = listed(&outcome).iter().map(|p| p.index).collect();
        assert_eq!(indices, [1, 3]);
        assert!(matches!(
            outcome.anomalies.as_slice(),
            [
                Anomaly::ZeroLengthPartition { index: 2, .. },
                Anomaly::PartitionBeyondEnd { index: 4, .. }
            ]
        ));
    }

    #[test]
    fn gpt_protective_is_detected_not_parsed() {
        let mut s = blank_sector();
        write_entry(&mut s, 0, 0x00, TYPE_GPT_PROTECTIVE, 1, 131_071);

        let outcome = parse_mbr(&s, 131_072).expect("protective MBR parses");
        assert_eq!(outcome.table, PartitionTable::GptProtective);
    }

    #[test]
    fn gpt_protective_wins_over_later_entries() {
        let mut s = blank_sector();
        write_entry(&mut s, 0, 0x00, TYPE_GPT_PROTECTIVE, 1, 131_071);
        write_entry(&mut s, 1, 0x00, 0x0c, 2048, 1000);

        let outcome = parse_mbr(&s, 131_072).expect("protective MBR parses");
        assert_eq!(outcome.table, PartitionTable::GptProtective);
    }

    /// A protective MBR as sgdisk writes it: one 0xEE record, boot indicator
    /// zero, from sector 1 to the end of an image of `sectors` sectors.
    fn protective_sector(sectors: u64) -> Vec<u8> {
        let mut s = blank_sector();
        write_entry(
            &mut s,
            0,
            0x00,
            TYPE_GPT_PROTECTIVE,
            1,
            (sectors - 1) as u32,
        );
        s
    }

    fn anomalies_of(sector: &[u8], sectors: u64) -> Vec<Anomaly> {
        parse_mbr(sector, sectors)
            .expect("a protective MBR parses")
            .anomalies
    }

    // `ADR-0025` Decision A.

    #[test]
    fn a_protective_mbr_matching_the_image_has_no_anomaly() {
        assert_eq!(anomalies_of(&protective_sector(131_072), 131_072), []);
    }

    #[test]
    fn a_protective_record_in_any_slot_makes_the_table_a_gpt() {
        let mut s = blank_sector();
        write_entry(&mut s, 2, 0x00, TYPE_GPT_PROTECTIVE, 1, 131_071);
        let outcome = parse_mbr(&s, 131_072).expect("parses");
        assert_eq!(outcome.table, PartitionTable::GptProtective);
        assert_eq!(outcome.anomalies, []);
    }

    #[test]
    fn a_protective_record_not_starting_at_sector_1_is_an_anomaly() {
        let mut s = protective_sector(131_072);
        write_entry(&mut s, 0, 0x00, TYPE_GPT_PROTECTIVE, 2, 131_071);
        assert_eq!(
            anomalies_of(&s, 131_072),
            [Anomaly::ProtectiveStartNotOne { start_lba: 2 }]
        );
    }

    #[test]
    fn a_protective_size_other_than_the_image_or_the_maximum_is_an_anomaly() {
        let mut s = protective_sector(131_072);
        write_entry(&mut s, 0, 0x00, TYPE_GPT_PROTECTIVE, 1, 1000);
        assert_eq!(
            anomalies_of(&s, 131_072),
            [Anomaly::ProtectiveSizeUnexpected {
                size: 1000,
                expected: 131_071
            }]
        );
    }

    #[test]
    fn a_protective_size_of_0xffffffff_is_expected_on_any_image() {
        let mut s = protective_sector(131_072);
        write_entry(&mut s, 0, 0x00, TYPE_GPT_PROTECTIVE, 1, u32::MAX);
        assert_eq!(anomalies_of(&s, 131_072), []);
        assert_eq!(anomalies_of(&s, 1 << 40), []);
    }

    #[test]
    fn another_record_in_use_beside_the_protective_one_is_a_hybrid_mbr() {
        let mut s = protective_sector(131_072);
        write_entry(&mut s, 1, 0x00, 0x0c, 2048, 1000);
        let outcome = parse_mbr(&s, 131_072).expect("parses");
        assert_eq!(outcome.table, PartitionTable::GptProtective);
        assert_eq!(
            outcome.anomalies,
            [Anomaly::HybridMbr {
                index: 2,
                partition_type: 0x0c
            }]
        );
    }

    #[test]
    fn junk_in_an_unused_record_beside_the_protective_one_is_not_a_hybrid() {
        let mut s = protective_sector(131_072);
        s[TABLE_OFFSET + ENTRY_SIZE + 1] = 0xFF;
        assert_eq!(
            anomalies_of(&s, 131_072),
            [Anomaly::NonZeroUnusedEntry { index: 2 }]
        );
    }

    #[test]
    fn a_protective_record_is_a_table_whatever_the_boot_indicators() {
        // A protective record's indicator "Must be ignored" (UEFI 5.2.3):
        // 0x42 would make the sector neither table nor volume otherwise.
        let mut s = protective_sector(131_072);
        s[TABLE_OFFSET] = 0x42;
        assert_eq!(classify_sector_zero(&s), SectorZero::Table);
    }

    #[test]
    fn a_bad_indicator_beside_a_protective_record_is_still_a_table() {
        let mut s = protective_sector(131_072);
        write_entry(&mut s, 1, 0x33, 0x0c, 2048, 1000);
        assert_eq!(classify_sector_zero(&s), SectorZero::Table);
    }

    #[test]
    fn a_protective_record_on_a_boot_record_is_ambiguous_not_a_table() {
        let mut s = fat32_boot_record();
        write_entry(&mut s, 0, 0x00, TYPE_GPT_PROTECTIVE, 1, 131_071);
        assert_eq!(
            classify_sector_zero(&s),
            SectorZero::Ambiguous {
                filesystem: Filesystem::Fat32,
                used_entries: 1
            }
        );
    }

    /// A 0xEE byte at a type offset inside boot code must not make a
    /// partitionless volume a GPT disk (`ADR-0024` regression guard).
    #[test]
    fn a_0xee_byte_at_a_type_offset_of_a_boot_record_is_not_a_table() {
        for (name, mut s) in [
            ("fat32", fat32_boot_record()),
            ("exfat", exfat_boot_record()),
        ] {
            s[TABLE_OFFSET + 4] = TYPE_GPT_PROTECTIVE; // offset 450
            let class = classify_sector_zero(&s);
            assert_ne!(class, SectorZero::Table, "{name}");
            assert!(
                matches!(
                    class,
                    SectorZero::Ambiguous {
                        used_entries: 1,
                        ..
                    }
                ),
                "{name}: {class:?}"
            );
        }
    }

    #[test]
    fn the_protective_record_is_counted_beside_other_used_entries_on_a_boot_record() {
        let mut s = fat32_boot_record();
        write_entry(&mut s, 0, 0x00, TYPE_GPT_PROTECTIVE, 0, 0);
        write_entry(&mut s, 1, 0x00, 0x0c, 2048, 1000);
        assert_eq!(
            classify_sector_zero(&s),
            SectorZero::Ambiguous {
                filesystem: Filesystem::Fat32,
                used_entries: 2
            }
        );
    }

    #[test]
    fn the_new_anomalies_render_without_debug_syntax() {
        let anomalies = [
            Anomaly::ProtectiveStartNotOne { start_lba: 2 },
            Anomaly::ProtectiveSizeUnexpected {
                size: 1,
                expected: 2,
            },
            Anomaly::HybridMbr {
                index: 2,
                partition_type: 0x0c,
            },
            Anomaly::GptBehindLegacyMbr,
            Anomaly::GptCheckNotMade { reason: "x".into() },
            Anomaly::GptRevision { revision: 1 },
            Anomaly::GptReservedNonZero,
            Anomaly::GptUsableBeyondEnd {
                last_usable: 9,
                image_sectors: 5,
            },
            Anomaly::GptBackupMissing { alternate_lba: 9 },
            Anomaly::GptBackupInvalid {
                lba: 9,
                reason: HeaderFailure::ArrayCrc,
            },
            Anomaly::GptBackupNotAtLastLba {
                alternate_lba: 9,
                last_lba: 10,
            },
            Anomaly::GptHeadersDisagree { field: "DiskGUID" },
            Anomaly::GptUnusedEntryNotZero { number: 1 },
            Anomaly::GptEntryStartsAfterEnd {
                number: 1,
                start: 5,
                end: 4,
            },
            Anomaly::GptEntryBeyondEnd {
                number: 1,
                start: 5,
                end: 40,
                image_sectors: 9,
            },
            Anomaly::GptEntryTooLarge {
                number: 1,
                start: 5,
                end: 40,
            },
            Anomaly::GptEntryOutsideUsable {
                number: 1,
                start: 5,
                end: 40,
                first_usable: 34,
                last_usable: 90,
            },
            Anomaly::GptOverlappingEntries {
                first: 1,
                second: 2,
            },
        ];
        for a in &anomalies {
            let text = a.to_string();
            assert!(!text.is_empty() && !text.contains('{') && !text.contains('\n'));
        }
        let left_out = anomalies
            .iter()
            .filter(|a| a.leaves_partition_out())
            .count();
        assert_eq!(left_out, 3, "the three GPT entries left out are gaps");
    }

    #[test]
    fn overlapping_partitions_are_flagged() {
        let mut s = blank_sector();
        write_entry(&mut s, 0, 0x00, 0x0c, 2048, 10_000);
        write_entry(&mut s, 1, 0x00, 0x0c, 5000, 10_000);

        let outcome = parse_mbr(&s, 131_072).expect("overlapping partitions still parse");
        assert!(outcome.anomalies.contains(&Anomaly::OverlappingPartitions {
            first: 1,
            second: 2
        }));
    }

    #[test]
    fn adjacent_partitions_do_not_overlap() {
        let mut s = blank_sector();
        write_entry(&mut s, 0, 0x00, 0x0c, 2048, 1000);
        write_entry(&mut s, 1, 0x00, 0x0c, 3048, 1000);

        let outcome = parse_mbr(&s, 131_072).expect("adjacent partitions parse");
        assert!(
            !outcome
                .anomalies
                .iter()
                .any(|a| matches!(a, Anomaly::OverlappingPartitions { .. })),
            "adjacent partitions must not be reported as overlapping"
        );
    }

    #[test]
    fn invalid_boot_indicator_is_flagged_not_rejected() {
        let mut s = blank_sector();
        write_entry(&mut s, 0, 0x42, 0x0c, 2048, 1000);

        let outcome = parse_mbr(&s, 131_072).expect("odd boot indicator still parses");
        assert!(outcome.anomalies.contains(&Anomaly::InvalidBootIndicator {
            index: 1,
            value: 0x42
        }));
    }

    #[test]
    fn unused_entry_with_junk_is_flagged() {
        let mut s = blank_sector();
        let base = TABLE_OFFSET + ENTRY_SIZE;
        s[base + 1] = 0xff;

        let outcome = parse_mbr(&s, 131_072).expect("junk in unused entry still parses");
        assert!(
            outcome
                .anomalies
                .contains(&Anomaly::NonZeroUnusedEntry { index: 2 })
        );
    }

    #[test]
    fn disk_signature_is_read() {
        let mut s = blank_sector();
        s[DISK_SIGNATURE_OFFSET..DISK_SIGNATURE_OFFSET + 4]
            .copy_from_slice(&0x1a2b_3c4du32.to_le_bytes());

        let outcome = parse_mbr(&s, 1000).expect("valid");
        let PartitionTable::Mbr { disk_signature, .. } = outcome.table else {
            panic!("expected Mbr");
        };
        assert_eq!(disk_signature, 0x1a2b_3c4d);
    }

    #[test]
    fn a_longer_buffer_is_accepted() {
        let mut s = blank_sector();
        s.extend_from_slice(&[0u8; 512]);
        parse_mbr(&s, 1000).expect("extra bytes beyond the sector are ignored");
    }

    #[test]
    fn anomaly_display_is_human_readable() {
        let anomalies = [
            Anomaly::InvalidBootIndicator {
                index: 1,
                value: 0x42,
            },
            Anomaly::MultipleBootablePartitions { count: 2 },
            Anomaly::OverlappingPartitions {
                first: 1,
                second: 2,
            },
            Anomaly::NonZeroUnusedEntry { index: 3 },
            Anomaly::EntriesOutOfOrder,
            Anomaly::PartitionBeyondEnd {
                index: 1,
                start_lba: 2048,
                sector_count: 1000,
                image_sectors: 2000,
            },
            Anomaly::ZeroLengthPartition {
                index: 2,
                partition_type: 0x0c,
            },
        ];

        for a in &anomalies {
            let rendered = a.to_string();
            assert!(!rendered.is_empty());
            assert!(
                !rendered.contains('{') && !rendered.contains('}'),
                "Debug-style struct syntax leaked into Display output: {rendered:?}"
            );
        }

        assert!(anomalies[0].to_string().contains("0x42"));
        // Worded as they were when both were errors rejecting the table.
        assert_eq!(
            anomalies[5].to_string(),
            "partition 1 claims sectors 2048..3048 but evidence has 2000 sectors"
        );
        assert_eq!(
            anomalies[6].to_string(),
            "partition 2 has type 0x0c but zero sectors"
        );
    }
}
