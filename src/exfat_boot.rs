//! The exFAT boot region: choosing a region, checking it, and reading the
//! volume's geometry from it.
//!
//! ADR-0019 section 3. The main boot region is used if its checksum is valid
//! and every field is within the range the specification gives it; otherwise
//! the backup region, if the same holds; otherwise the volume is not
//! analysed. A volume with two FATs is TexFAT (specification section 3.1.16),
//! which is identified and not analysed.
//!
//! Every field here is untrusted evidence. The sector size, which decides how
//! many bytes the region occupies, is read from sector 0 before anything else
//! and is range checked before it is used to size a read. Nothing in this
//! module reads a FAT, a directory or the allocation bitmap.
//!
//! The specification's section numbers are cited beside each rule. Where the
//! specification states one bound twice, in two fields' ranges, a single
//! check enforces it and says so.
//!
//! `PartitionOffset` (section 3.1.4) is not read: every value is valid and 0
//! means "ignore", so a disagreement with the partition table is not a
//! finding.

use std::fmt;

use crate::error::Error;
use crate::evidence::EvidenceReader;
use crate::filesystem::{OFF_SIGNATURE, SIGNATURE, VolumeExtent, le_u16, le_u32};
use crate::partition::SECTOR_SIZE;

/// Sectors in one boot region, main or backup. Section 3: the boot sector,
/// eight extended boot sectors, the OEM parameters, a reserved sector and the
/// checksum sector.
const REGION_SECTORS: u64 = 12;

/// Sectors the boot checksum covers, which are the region without its
/// checksum sector. Section 3.4.
const CHECKSUM_SECTORS: usize = 11;

/// Smallest and largest `BytesPerSectorShift`. Section 3.1.14.
const MIN_BYTES_PER_SECTOR_SHIFT: u8 = 9;
const MAX_BYTES_PER_SECTOR_SHIFT: u8 = 12;

/// A cluster is at most 32 MiB, so the two shifts sum to at most 25.
/// Section 3.1.15.
const MAX_CLUSTER_SHIFT_SUM: u8 = 25;

/// Most clusters a FAT can describe. Section 3.1.9.
const MAX_CLUSTER_COUNT: u32 = u32::MAX - 10;

/// The first sector a FAT may start at, after both boot regions.
/// Section 3.1.6.
const MIN_FAT_OFFSET: u32 = 24;

/// The smallest volume, 1 MiB. Sections 3.1.5 and 9.4.
const MIN_VOLUME_BYTES: u64 = 1 << 20;

/// Bytes of a FAT entry. Section 4.
const FAT_ENTRY_BYTES: u64 = 4;

/// Largest value of either revision byte. Section 3.1.12.
const MAX_REVISION_BYTE: u8 = 99;

/// The only major revision this module analyses. Section 3.1.12.
const SUPPORTED_MAJOR_REVISION: u8 = 1;

/// A volume holds at least the root directory, the up-case table and one
/// allocation bitmap per FAT. Section 9.3.
const BASE_CLUSTERS: u32 = 2;

const OFF_JUMP: usize = 0;
const OFF_FILE_SYSTEM_NAME: usize = 3;
const OFF_MUST_BE_ZERO: usize = 11;
const MUST_BE_ZERO_LEN: usize = 53;
const OFF_VOLUME_LENGTH: usize = 72;
const OFF_FAT_OFFSET: usize = 80;
const OFF_FAT_LENGTH: usize = 84;
const OFF_CLUSTER_HEAP_OFFSET: usize = 88;
const OFF_CLUSTER_COUNT: usize = 92;
const OFF_ROOT_CLUSTER: usize = 96;
const OFF_VOLUME_SERIAL: usize = 100;
const OFF_REVISION_MINOR: usize = 104;
const OFF_REVISION_MAJOR: usize = 105;
const OFF_VOLUME_FLAGS: usize = 106;
const OFF_BYTES_PER_SECTOR_SHIFT: usize = 108;
const OFF_SECTORS_PER_CLUSTER_SHIFT: usize = 109;
const OFF_NUMBER_OF_FATS: usize = 110;
const OFF_PERCENT_IN_USE: usize = 112;

/// Bytes the checksum skips: VolumeFlags and PercentInUse. Section 3.4.
const CHECKSUM_SKIPPED: [usize; 3] = [OFF_VOLUME_FLAGS, OFF_VOLUME_FLAGS + 1, OFF_PERCENT_IN_USE];

const JUMP_BOOT: [u8; 3] = [0xEB, 0x76, 0x90];
const FILE_SYSTEM_NAME: [u8; 8] = *b"EXFAT   ";

/// `PercentInUse` value meaning the percentage is not available.
/// Section 3.1.18.
const PERCENT_NOT_AVAILABLE: u8 = 0xFF;

/// Which boot region a volume's geometry was read from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BootRegion {
    /// The main boot region.
    Main,

    /// The backup boot region, because the main one was refused.
    Backup,
}

/// Something unusual about a volume that does not stop it being analysed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExFatObservation {
    /// The volume is shorter than the partition holding it. Sectors past the
    /// volume's end are not part of it. A volume longer than its partition is
    /// refused instead.
    VolumeShorterThanPartition {
        /// Bytes the boot sector declares.
        volume_bytes: u64,
        /// Bytes the partition table declares.
        partition_bytes: u64,
    },
}

impl fmt::Display for ExFatObservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VolumeShorterThanPartition {
                volume_bytes,
                partition_bytes,
            } => write!(
                f,
                "volume is {volume_bytes} bytes, shorter than its partition's {partition_bytes}"
            ),
        }
    }
}

/// A validated exFAT boot sector.
///
/// Every field has passed the range the specification gives it, and the
/// geometry is consistent: the FAT lies before the cluster heap, and the
/// cluster count is exactly the one the volume's length implies.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ExFatBootSector {
    /// The region these fields were read from.
    pub region: BootRegion,

    /// `BytesPerSectorShift`: a sector is `1 << this` bytes, 512 to 4096.
    pub bytes_per_sector_shift: u8,

    /// `SectorsPerClusterShift`: a cluster is `1 << this` sectors.
    pub sectors_per_cluster_shift: u8,

    /// Volume length in sectors of the volume's own size.
    pub volume_length: u64,

    /// First sector of the FAT, relative to the volume.
    pub fat_offset: u32,

    /// Sectors in each FAT.
    pub fat_length: u32,

    /// First sector of the cluster heap, relative to the volume.
    pub cluster_heap_offset: u32,

    /// Clusters in the heap. They are numbered 2 to `cluster_count + 1`.
    pub cluster_count: u32,

    /// First cluster of the root directory.
    pub root_cluster: u32,

    /// The serial number, for telling volumes apart.
    pub volume_serial: u32,

    /// Minor revision. The major revision is 1 or the volume was refused.
    pub revision_minor: u8,

    /// `VolumeFlags`, from the main region only.
    ///
    /// `None` when the backup region was used, because the specification
    /// says its copy is stale (section 3.1). The checksum excludes it, so a
    /// valid checksum says nothing about it.
    pub volume_flags: Option<u16>,

    /// `PercentInUse`, from the main region only, for the reason above.
    pub percent_in_use: Option<u8>,

    /// What was noticed and did not stop the volume being analysed.
    pub observations: Vec<ExFatObservation>,

    /// The backup region was found by trying each sector size, because the
    /// main region's sector size was unusable. Always `false` for the main
    /// region, and for a backup located by the main region's sector size.
    pub located_by_search: bool,
}

impl ExFatBootSector {
    /// Bytes in a sector.
    pub const fn bytes_per_sector(&self) -> u32 {
        1 << self.bytes_per_sector_shift
    }

    /// Bytes in a cluster, at most 32 MiB.
    pub const fn cluster_bytes(&self) -> u32 {
        1 << (self.bytes_per_sector_shift + self.sectors_per_cluster_shift)
    }
}

/// Why one boot region was refused.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ExFatError {
    /// The region does not lie inside the partition.
    RegionOutsidePartition {
        /// Bytes the region needs, counting from the volume's start.
        required: u64,
        /// Bytes the partition holds.
        available: u64,
    },

    /// The backup region cannot be located, because where it starts depends
    /// on the main region's sector size and that field is out of range.
    BackupNotLocated,

    /// Section 3.1.14.
    BytesPerSectorShift { value: u8 },

    /// Section 3.4. The stored checksum is not the one the region computes.
    ChecksumMismatch { computed: u32, stored: u32 },

    /// Section 3.4. The checksum sector repeats one value from its start to
    /// its end, and a word differs from the first.
    ChecksumNotRepeated { word: usize },

    /// Section 3.1.1.
    JumpBoot { found: [u8; 3] },

    /// Section 3.1.2.
    FileSystemName { found: [u8; 8] },

    /// Section 3.1.3. The first nonzero byte's offset in the sector.
    MustBeZero { offset: usize },

    /// Section 3.1.20.
    BootSignature { found: [u8; 2] },

    /// Section 3.1.15.
    SectorsPerClusterShift { value: u8, maximum: u8 },

    /// Section 3.1.16. Neither 1 nor 2.
    NumberOfFats { value: u8 },

    /// Sections 3.1.5 and 9.4.
    VolumeLengthTooSmall { sectors: u64, minimum: u64 },

    /// Section 3.1.6.
    FatOffsetTooSmall { value: u32, minimum: u32 },

    /// Section 3.1.7. The FAT cannot describe every cluster.
    FatLengthTooSmall { value: u32, minimum: u64 },

    /// Sections 3.1.6, 3.1.7 and 3.1.8, which together say the cluster heap
    /// begins at or after the end of the FATs. One check enforces all three.
    ClusterHeapBeforeFatEnd {
        cluster_heap_offset: u32,
        fat_end: u64,
    },

    /// Section 3.1.8. The heap would extend past the end of the volume.
    ClusterHeapOffsetTooLarge {
        cluster_heap_offset: u32,
        maximum: u64,
    },

    /// Section 3.1.9.
    ClusterCountTooLarge { value: u32 },

    /// Section 9.3.
    ClusterCountTooSmall { value: u32, minimum: u32 },

    /// Section 3.1.9. The valid value is exactly the lesser of the clusters
    /// the volume's length holds and the most a FAT can describe.
    ClusterCountMismatch { value: u32, expected: u32 },

    /// Section 3.1.10.
    RootClusterOutOfRange { value: u32, last: u64 },

    /// Section 3.1.12. A revision byte above 99.
    RevisionOutOfRange { major: u8, minor: u8 },

    /// Section 3.1.12. A major revision other than 1, which the
    /// specification says shall not be mounted.
    RevisionUnsupported { major: u8, minor: u8 },

    /// Section 3.1.13.1. The second FAT is active on a volume with one.
    ActiveFat { flags: u16 },

    /// Section 3.1.18.
    PercentInUse { value: u8 },

    /// The volume is longer than the partition holding it, so its last
    /// sectors would be read from outside it. Compared in bytes.
    VolumeLongerThanPartition {
        volume_bytes: u128,
        partition_bytes: u64,
    },

    /// The region is valid and declares two FATs. TexFAT is identified and
    /// not analysed.
    TexFat,
}

impl fmt::Display for ExFatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RegionOutsidePartition {
                required,
                available,
            } => write!(
                f,
                "boot region needs {required} bytes, the partition holds {available}"
            ),
            Self::BackupNotLocated => f.write_str(
                "backup boot region not located: it starts where the main region's sector size says",
            ),
            Self::BytesPerSectorShift { value } => write!(
                f,
                "bytes per sector shift {value} is outside {MIN_BYTES_PER_SECTOR_SHIFT} to {MAX_BYTES_PER_SECTOR_SHIFT}"
            ),
            Self::ChecksumMismatch { computed, stored } => write!(
                f,
                "boot checksum {computed:#010x} does not match the stored {stored:#010x}"
            ),
            Self::ChecksumNotRepeated { word } => {
                write!(f, "boot checksum sector differs from its first word at word {word}")
            }
            Self::JumpBoot { found } => write!(
                f,
                "jump instruction {:02x} {:02x} {:02x} is not eb 76 90",
                found[0], found[1], found[2]
            ),
            Self::FileSystemName { found } => {
                write!(f, "file system name {found:02x?} is not \"EXFAT   \"")
            }
            Self::MustBeZero { offset } => {
                write!(f, "byte {offset} of the must-be-zero field is not zero")
            }
            Self::BootSignature { found } => {
                write!(f, "boot signature {:02x} {:02x} is not 55 aa", found[0], found[1])
            }
            Self::SectorsPerClusterShift { value, maximum } => write!(
                f,
                "sectors per cluster shift {value} exceeds {maximum}, the largest that keeps a cluster within 32 MiB"
            ),
            Self::NumberOfFats { value } => write!(f, "number of FATs {value} is neither 1 nor 2"),
            Self::VolumeLengthTooSmall { sectors, minimum } => {
                write!(f, "volume length {sectors} sectors is below the minimum {minimum}")
            }
            Self::FatOffsetTooSmall { value, minimum } => {
                write!(f, "FAT offset {value} is below {minimum}")
            }
            Self::FatLengthTooSmall { value, minimum } => write!(
                f,
                "FAT length {value} sectors cannot describe every cluster, which needs {minimum}"
            ),
            Self::ClusterHeapBeforeFatEnd {
                cluster_heap_offset,
                fat_end,
            } => write!(
                f,
                "cluster heap offset {cluster_heap_offset} is before the end of the FATs at {fat_end}"
            ),
            Self::ClusterHeapOffsetTooLarge {
                cluster_heap_offset,
                maximum,
            } => write!(
                f,
                "cluster heap offset {cluster_heap_offset} is past {maximum}, where the heap no longer fits the volume"
            ),
            Self::ClusterCountTooLarge { value } => {
                write!(f, "cluster count {value} exceeds {MAX_CLUSTER_COUNT}")
            }
            Self::ClusterCountTooSmall { value, minimum } => {
                write!(f, "cluster count {value} is below {minimum}")
            }
            Self::ClusterCountMismatch { value, expected } => write!(
                f,
                "cluster count {value} is not the {expected} the volume's length implies"
            ),
            Self::RootClusterOutOfRange { value, last } => {
                write!(f, "root directory cluster {value} is outside 2 to {last}")
            }
            Self::RevisionOutOfRange { major, minor } => {
                write!(f, "revision {major}.{minor:02} has a byte above {MAX_REVISION_BYTE}")
            }
            Self::RevisionUnsupported { major, minor } => {
                write!(f, "revision {major}.{minor:02} is not major revision {SUPPORTED_MAJOR_REVISION}")
            }
            Self::ActiveFat { flags } => write!(
                f,
                "volume flags {flags:#06x} make the second FAT active on a volume with one"
            ),
            Self::PercentInUse { value } => {
                write!(f, "percent in use {value} is neither 0 to 100 nor {PERCENT_NOT_AVAILABLE}")
            }
            Self::VolumeLongerThanPartition {
                volume_bytes,
                partition_bytes,
            } => write!(
                f,
                "volume is {volume_bytes} bytes, longer than its partition's {partition_bytes}"
            ),
            Self::TexFat => f.write_str("volume has two FATs: TexFAT is not analysed"),
        }
    }
}

impl std::error::Error for ExFatError {}

/// Why a volume's boot region gave no geometry.
#[derive(Debug)]
pub enum ExFatBootFailure {
    /// The evidence could not be read.
    Unreadable(Error),

    /// A region is valid and the volume has two FATs.
    TexFat(BootRegion),

    /// Neither region was usable. Both reasons are kept: the operator needs
    /// to know the backup was tried, and why it failed.
    Rejected {
        /// Why the main region was refused.
        main: ExFatError,
        /// Why the backup region was refused.
        backup: ExFatError,
    },

    /// The main region's sector size was unusable, and a valid backup
    /// region was found at more than one sector size. Which is the volume's
    /// is not something the evidence says, so none is used.
    AmbiguousBackup {
        /// The sector size shifts at which a valid backup region was found.
        shifts: Vec<u8>,
    },
}

impl fmt::Display for ExFatBootFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(e) => write!(f, "{e}"),
            Self::TexFat(_) => write!(f, "{}", ExFatError::TexFat),
            Self::AmbiguousBackup { shifts } => {
                write!(
                    f,
                    "main boot region unusable, and a valid backup boot region was found at more than one sector size (shifts {shifts:?})"
                )
            }
            Self::Rejected { main, backup } => {
                write!(f, "main boot region: {main}; backup boot region: {backup}")
            }
        }
    }
}

impl std::error::Error for ExFatBootFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unreadable(e) => Some(e),
            Self::TexFat(_) | Self::Rejected { .. } | Self::AmbiguousBackup { .. } => None,
        }
    }
}

/// Reads and validates a volume's boot region.
///
/// Reads sector 0 for the sector size, then the main region, and the backup
/// region only if the main one is refused. If the backup at the offset the
/// main region's sector size gives is refused too, or that sector size is
/// unusable, the other sector sizes are tried for it: `search_backup`.
/// `extent` is the partition as the partition table declares it, in 512-byte
/// sectors. Each region must lie inside it.
///
/// A read failure is returned as it is. It says nothing about the volume's
/// structure, and a backup region is not tried in its place.
pub fn parse_boot_region<R: EvidenceReader>(
    reader: &mut R,
    extent: VolumeExtent,
) -> Result<ExFatBootSector, ExFatBootFailure> {
    let start = extent.start_lba as u64 * SECTOR_SIZE as u64;
    let available = extent.sector_count as u64 * SECTOR_SIZE as u64;

    let mut head = [0u8; SECTOR_SIZE];
    if available < SECTOR_SIZE as u64 {
        return Err(refused_everywhere(ExFatError::RegionOutsidePartition {
            required: SECTOR_SIZE as u64,
            available,
        }));
    }
    reader
        .read_exact_at(start, &mut head)
        .map_err(ExFatBootFailure::Unreadable)?;

    let shift = head[OFF_BYTES_PER_SECTOR_SHIFT];
    if !(MIN_BYTES_PER_SECTOR_SHIFT..=MAX_BYTES_PER_SECTOR_SHIFT).contains(&shift) {
        return search_backup(
            reader,
            start,
            extent,
            None,
            ExFatError::BytesPerSectorShift { value: shift },
            ExFatError::BackupNotLocated,
        );
    }

    let region_bytes = REGION_SECTORS << shift;

    let main = read_and_validate(reader, start, 0, region_bytes, BootRegion::Main, extent)?;
    let main_error = match main {
        Ok(boot) => return Ok(boot),
        Err(ExFatError::TexFat) => return Err(ExFatBootFailure::TexFat(BootRegion::Main)),
        Err(e) => e,
    };

    let backup = read_and_validate(
        reader,
        start,
        region_bytes,
        region_bytes,
        BootRegion::Backup,
        extent,
    )?;
    match backup {
        Ok(boot) => Ok(boot),
        Err(ExFatError::TexFat) => Err(ExFatBootFailure::TexFat(BootRegion::Backup)),
        // The main region's own sector size may be the damaged field: a
        // flipped bit can leave it in range and wrong, which sends the read
        // above to the wrong offset. The other sizes are tried.
        Err(backup) => search_backup(reader, start, extent, Some(shift), main_error, backup),
    }
}

/// Finds the backup region by trying sector sizes.
///
/// Used when the main region is refused and the backup at the offset its own
/// sector size gives is refused too, or when that sector size is unusable.
/// The backup starts at sector 12, and how long a sector is, is exactly what
/// the damaged main region may have lost. Each sector size from 512 to 4096
/// except `skip`, the one already tried, is tried: twelve sectors are read
/// at `12 << shift` bytes and validated by every rule, and the candidate is
/// accepted only if its own `BytesPerSectorShift` is the one it was tried at
/// and it lies inside the partition. A region that merely validates is not
/// enough: a 512-byte region found at the 4096-byte candidate's offset is a
/// valid region, and not a backup that starts there.
///
/// Exactly one candidate is used. Two valid candidates are two different
/// volumes' worth of structure in one place, and choosing between them would
/// be a guess, so the volume is refused and the ambiguity reported. The
/// result of a successful search says it was found by one. Where none is
/// valid, the failures the caller already had are reported unchanged.
fn search_backup<R: EvidenceReader>(
    reader: &mut R,
    start: u64,
    extent: VolumeExtent,
    skip: Option<u8>,
    main_error: ExFatError,
    backup_error: ExFatError,
) -> Result<ExFatBootSector, ExFatBootFailure> {
    let available = extent.sector_count as u64 * SECTOR_SIZE as u64;

    // `None` is a valid candidate that declares two FATs.
    let mut passed: Vec<(u8, Option<ExFatBootSector>)> = Vec::new();

    for shift in MIN_BYTES_PER_SECTOR_SHIFT..=MAX_BYTES_PER_SECTOR_SHIFT {
        if Some(shift) == skip {
            continue;
        }

        let region_bytes = REGION_SECTORS << shift;

        // The candidate must lie inside the partition: the backup starts
        // twelve sectors in and is twelve sectors long.
        if 2 * region_bytes > available {
            continue;
        }

        // Bounded: twelve sectors of at most 4096 bytes.
        let mut region = vec![0u8; region_bytes as usize];
        reader
            .read_exact_at(start + region_bytes, &mut region)
            .map_err(ExFatBootFailure::Unreadable)?;

        if region[OFF_BYTES_PER_SECTOR_SHIFT] != shift {
            continue;
        }

        match validate_region(&region, BootRegion::Backup, extent) {
            Ok(mut boot) => {
                boot.located_by_search = true;
                passed.push((shift, Some(boot)));
            }
            Err(ExFatError::TexFat) => passed.push((shift, None)),
            Err(_) => {}
        }
    }

    match passed.len() {
        0 => Err(ExFatBootFailure::Rejected {
            main: main_error,
            backup: backup_error,
        }),
        1 => match passed.remove(0).1 {
            Some(boot) => Ok(boot),
            None => Err(ExFatBootFailure::TexFat(BootRegion::Backup)),
        },
        _ => Err(ExFatBootFailure::AmbiguousBackup {
            shifts: passed.into_iter().map(|(shift, _)| shift).collect(),
        }),
    }
}

/// A failure that stops both regions at once.
fn refused_everywhere(error: ExFatError) -> ExFatBootFailure {
    ExFatBootFailure::Rejected {
        main: error.clone(),
        backup: error,
    }
}

/// Reads one region and validates it. The outer error is the evidence's, the
/// inner is the region's.
fn read_and_validate<R: EvidenceReader>(
    reader: &mut R,
    start: u64,
    region_offset: u64,
    region_bytes: u64,
    which: BootRegion,
    extent: VolumeExtent,
) -> Result<Result<ExFatBootSector, ExFatError>, ExFatBootFailure> {
    let available = extent.sector_count as u64 * SECTOR_SIZE as u64;
    let required = region_offset + region_bytes;

    if required > available {
        return Ok(Err(ExFatError::RegionOutsidePartition {
            required,
            available,
        }));
    }

    // Bounded: `region_bytes` is twelve sectors of at most 4096 bytes.
    let mut region = vec![0u8; region_bytes as usize];
    reader
        .read_exact_at(start + region_offset, &mut region)
        .map_err(ExFatBootFailure::Unreadable)?;

    Ok(validate_region(&region, which, extent))
}

/// The checksum over the first eleven sectors of a boot region.
///
/// Section 3.4. `region` holds at least eleven sectors of
/// `1 << bytes_per_sector_shift` bytes. VolumeFlags and PercentInUse are
/// skipped, because an implementation may change them without updating the
/// checksum.
pub fn boot_checksum(region: &[u8], bytes_per_sector_shift: u8) -> u32 {
    let covered = CHECKSUM_SECTORS << bytes_per_sector_shift;
    let mut checksum: u32 = 0;

    for (index, &byte) in region.iter().take(covered).enumerate() {
        if CHECKSUM_SKIPPED.contains(&index) {
            continue;
        }
        let rotated_in: u32 = if checksum & 1 == 1 { 0x8000_0000 } else { 0 };
        checksum = rotated_in
            .wrapping_add(checksum >> 1)
            .wrapping_add(byte as u32);
    }

    checksum
}

/// Validates one boot region.
///
/// `region` is the twelve sectors of the region. `extent` is the partition,
/// and the volume length is compared with it in bytes, because the two count
/// in different units. Performs no I/O.
///
/// The checksum is checked before any other field is used, as section 3.4
/// requires, and the sector size before the checksum, because it says how
/// many bytes the checksum covers.
pub fn validate_region(
    region: &[u8],
    which: BootRegion,
    extent: VolumeExtent,
) -> Result<ExFatBootSector, ExFatError> {
    if region.len() < SECTOR_SIZE {
        return Err(ExFatError::RegionOutsidePartition {
            required: SECTOR_SIZE as u64,
            available: region.len() as u64,
        });
    }

    let shift = region[OFF_BYTES_PER_SECTOR_SHIFT];
    if !(MIN_BYTES_PER_SECTOR_SHIFT..=MAX_BYTES_PER_SECTOR_SHIFT).contains(&shift) {
        return Err(ExFatError::BytesPerSectorShift { value: shift });
    }

    let sector_bytes = 1usize << shift;
    let region_bytes = (REGION_SECTORS as usize) << shift;
    if region.len() < region_bytes {
        return Err(ExFatError::RegionOutsidePartition {
            required: region_bytes as u64,
            available: region.len() as u64,
        });
    }

    check_checksum(region, shift, sector_bytes)?;

    let jump = [region[OFF_JUMP], region[OFF_JUMP + 1], region[OFF_JUMP + 2]];
    if jump != JUMP_BOOT {
        return Err(ExFatError::JumpBoot { found: jump });
    }

    let mut name = [0u8; 8];
    name.copy_from_slice(&region[OFF_FILE_SYSTEM_NAME..OFF_FILE_SYSTEM_NAME + 8]);
    if name != FILE_SYSTEM_NAME {
        return Err(ExFatError::FileSystemName { found: name });
    }

    if let Some(offset) = region[OFF_MUST_BE_ZERO..OFF_MUST_BE_ZERO + MUST_BE_ZERO_LEN]
        .iter()
        .position(|&b| b != 0)
    {
        return Err(ExFatError::MustBeZero {
            offset: OFF_MUST_BE_ZERO + offset,
        });
    }

    let signature = [region[OFF_SIGNATURE], region[OFF_SIGNATURE + 1]];
    if signature != SIGNATURE {
        return Err(ExFatError::BootSignature { found: signature });
    }

    let cluster_shift = region[OFF_SECTORS_PER_CLUSTER_SHIFT];
    let maximum_shift = MAX_CLUSTER_SHIFT_SUM - shift;
    if cluster_shift > maximum_shift {
        return Err(ExFatError::SectorsPerClusterShift {
            value: cluster_shift,
            maximum: maximum_shift,
        });
    }

    let fats = region[OFF_NUMBER_OF_FATS];
    if fats != 1 && fats != 2 {
        return Err(ExFatError::NumberOfFats { value: fats });
    }

    let geometry = Geometry {
        volume_length: le_u64(region, OFF_VOLUME_LENGTH),
        fat_offset: le_u32(region, OFF_FAT_OFFSET),
        fat_length: le_u32(region, OFF_FAT_LENGTH),
        cluster_heap_offset: le_u32(region, OFF_CLUSTER_HEAP_OFFSET),
        cluster_count: le_u32(region, OFF_CLUSTER_COUNT),
        root_cluster: le_u32(region, OFF_ROOT_CLUSTER),
        shift,
        cluster_shift,
        fats,
    };
    check_geometry(&geometry)?;

    let major = region[OFF_REVISION_MAJOR];
    let minor = region[OFF_REVISION_MINOR];
    if major > MAX_REVISION_BYTE || minor > MAX_REVISION_BYTE {
        return Err(ExFatError::RevisionOutOfRange { major, minor });
    }
    if major != SUPPORTED_MAJOR_REVISION {
        return Err(ExFatError::RevisionUnsupported { major, minor });
    }

    // Both fields are outside the checksum, and the backup's copies are
    // stale: only the main region's are checked, and only they are kept.
    let (volume_flags, percent_in_use) = match which {
        BootRegion::Main => {
            let flags = le_u16(region, OFF_VOLUME_FLAGS);
            let percent = region[OFF_PERCENT_IN_USE];
            check_main_only_fields(flags, percent, fats)?;
            (Some(flags), Some(percent))
        }
        BootRegion::Backup => (None, None),
    };

    // Compared in bytes: the volume counts sectors of its own size and the
    // partition table counts 512-byte sectors. In u128, because a declared
    // length of nearly 2^64 sectors of 4096 bytes does not fit in u64.
    let volume_bytes = (geometry.volume_length as u128) << shift;
    let partition_bytes = extent.sector_count as u64 * SECTOR_SIZE as u64;
    let mut observations = Vec::new();
    if volume_bytes > partition_bytes as u128 {
        return Err(ExFatError::VolumeLongerThanPartition {
            volume_bytes,
            partition_bytes,
        });
    }
    if volume_bytes < partition_bytes as u128 {
        observations.push(ExFatObservation::VolumeShorterThanPartition {
            // Less than `partition_bytes`, which is a u64.
            volume_bytes: volume_bytes as u64,
            partition_bytes,
        });
    }

    // Last, so a TexFAT volume that is otherwise malformed is reported as
    // malformed, not as unsupported.
    if fats == 2 {
        return Err(ExFatError::TexFat);
    }

    Ok(ExFatBootSector {
        region: which,
        bytes_per_sector_shift: shift,
        sectors_per_cluster_shift: cluster_shift,
        volume_length: geometry.volume_length,
        fat_offset: geometry.fat_offset,
        fat_length: geometry.fat_length,
        cluster_heap_offset: geometry.cluster_heap_offset,
        cluster_count: geometry.cluster_count,
        root_cluster: geometry.root_cluster,
        volume_serial: le_u32(region, OFF_VOLUME_SERIAL),
        revision_minor: minor,
        volume_flags,
        percent_in_use,
        observations,
        located_by_search: false,
    })
}

/// Section 3.4. The checksum the region computes, the value in its checksum
/// sector, and that sector repeating the value from end to end.
fn check_checksum(region: &[u8], shift: u8, sector_bytes: usize) -> Result<(), ExFatError> {
    let computed = boot_checksum(region, shift);
    let sector = &region[CHECKSUM_SECTORS * sector_bytes..(CHECKSUM_SECTORS + 1) * sector_bytes];

    let stored = le_u32(sector, 0);
    if computed != stored {
        return Err(ExFatError::ChecksumMismatch { computed, stored });
    }

    for word in 1..sector_bytes / 4 {
        if le_u32(sector, word * 4) != stored {
            return Err(ExFatError::ChecksumNotRepeated { word });
        }
    }

    Ok(())
}

/// The geometry fields, with the two shifts and the FAT count.
struct Geometry {
    volume_length: u64,
    fat_offset: u32,
    fat_length: u32,
    cluster_heap_offset: u32,
    cluster_count: u32,
    root_cluster: u32,
    shift: u8,
    cluster_shift: u8,
    fats: u8,
}

/// Sections 3.1.5 to 3.1.10 and 9.3. In 64-bit arithmetic throughout, so no
/// field can overflow a calculation made from it.
fn check_geometry(g: &Geometry) -> Result<(), ExFatError> {
    let minimum_sectors = MIN_VOLUME_BYTES >> g.shift;
    if g.volume_length < minimum_sectors {
        return Err(ExFatError::VolumeLengthTooSmall {
            sectors: g.volume_length,
            minimum: minimum_sectors,
        });
    }

    if g.fat_offset < MIN_FAT_OFFSET {
        return Err(ExFatError::FatOffsetTooSmall {
            value: g.fat_offset,
            minimum: MIN_FAT_OFFSET,
        });
    }

    // Each FAT holds an entry for the two reserved ones and for every cluster.
    let fat_bytes = (g.cluster_count as u64 + 2) * FAT_ENTRY_BYTES;
    let minimum_fat_length = fat_bytes.div_ceil(1 << g.shift);
    if (g.fat_length as u64) < minimum_fat_length {
        return Err(ExFatError::FatLengthTooSmall {
            value: g.fat_length,
            minimum: minimum_fat_length,
        });
    }

    // Sections 3.1.6 and 3.1.7 bound the FAT offset and length from above by
    // the cluster heap offset, and 3.1.8 bounds the heap offset from below by
    // the FATs' end. All three are this one inequality.
    let fat_end = g.fat_offset as u64 + g.fat_length as u64 * g.fats as u64;
    if (g.cluster_heap_offset as u64) < fat_end {
        return Err(ExFatError::ClusterHeapBeforeFatEnd {
            cluster_heap_offset: g.cluster_heap_offset,
            fat_end,
        });
    }

    // Section 3.1.8's other bound. The 2^32 - 1 half cannot be exceeded by a
    // u32 field.
    let heap_sectors = (g.cluster_count as u64) << g.cluster_shift;
    let heap_limit = g.volume_length.saturating_sub(heap_sectors);
    if g.cluster_heap_offset as u64 > heap_limit.min(u32::MAX as u64) {
        return Err(ExFatError::ClusterHeapOffsetTooLarge {
            cluster_heap_offset: g.cluster_heap_offset,
            maximum: heap_limit.min(u32::MAX as u64),
        });
    }

    if g.cluster_count > MAX_CLUSTER_COUNT {
        return Err(ExFatError::ClusterCountTooLarge {
            value: g.cluster_count,
        });
    }
    let minimum_clusters = BASE_CLUSTERS + g.fats as u32;
    if g.cluster_count < minimum_clusters {
        return Err(ExFatError::ClusterCountTooSmall {
            value: g.cluster_count,
            minimum: minimum_clusters,
        });
    }

    // The heap offset is at most the volume length here, so this cannot
    // underflow. At most u32::MAX clusters fit the u32 result.
    let fits = (g.volume_length - g.cluster_heap_offset as u64) >> g.cluster_shift;
    let expected = fits.min(MAX_CLUSTER_COUNT as u64) as u32;
    if g.cluster_count != expected {
        return Err(ExFatError::ClusterCountMismatch {
            value: g.cluster_count,
            expected,
        });
    }

    let last = g.cluster_count as u64 + 1;
    if g.root_cluster < 2 || g.root_cluster as u64 > last {
        return Err(ExFatError::RootClusterOutOfRange {
            value: g.root_cluster,
            last,
        });
    }

    Ok(())
}

/// Sections 3.1.13.1 and 3.1.18, for the fields the checksum does not cover.
fn check_main_only_fields(flags: u16, percent: u8, fats: u8) -> Result<(), ExFatError> {
    // ActiveFat is bit 0, and 1 is possible only with two FATs.
    if flags & 1 == 1 && fats == 1 {
        return Err(ExFatError::ActiveFat { flags });
    }

    if percent > 100 && percent != PERCENT_NOT_AVAILABLE {
        return Err(ExFatError::PercentInUse { value: percent });
    }

    Ok(())
}

fn le_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::evidence::tests::MemoryImage;

    /// The geometry of EXP-0009's volumes: 4 KiB clusters, 3,792 of them.
    const VOLUME_LENGTH: u64 = 30_592;
    const START_LBA: u32 = 128;

    pub(crate) fn extent() -> VolumeExtent {
        VolumeExtent {
            start_lba: START_LBA,
            sector_count: VOLUME_LENGTH as u32,
        }
    }

    /// Sets a checksum sector from the other eleven.
    fn seal(region: &mut [u8]) {
        let shift = region[OFF_BYTES_PER_SECTOR_SHIFT];
        // A region with a sector size out of range has no checksum sector
        // to seal; the tests that build one want exactly that.
        if !(MIN_BYTES_PER_SECTOR_SHIFT..=MAX_BYTES_PER_SECTOR_SHIFT).contains(&shift) {
            return;
        }
        let sector = 1usize << shift;
        let checksum = boot_checksum(region, shift);
        for word in 0..sector / 4 {
            let at = CHECKSUM_SECTORS * sector + word * 4;
            region[at..at + 4].copy_from_slice(&checksum.to_le_bytes());
        }
    }

    fn put_u32(region: &mut [u8], offset: usize, value: u32) {
        region[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u64(region: &mut [u8], offset: usize, value: u64) {
        region[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    /// A valid region for the synthetic volume, sealed.
    pub(crate) fn valid() -> Vec<u8> {
        let mut r = vec![0u8; 12 * 512];
        r[OFF_JUMP..OFF_JUMP + 3].copy_from_slice(&JUMP_BOOT);
        r[OFF_FILE_SYSTEM_NAME..OFF_FILE_SYSTEM_NAME + 8].copy_from_slice(&FILE_SYSTEM_NAME);
        put_u64(&mut r, 64, START_LBA as u64);
        put_u64(&mut r, OFF_VOLUME_LENGTH, VOLUME_LENGTH);
        put_u32(&mut r, OFF_FAT_OFFSET, 128);
        put_u32(&mut r, OFF_FAT_LENGTH, 32);
        put_u32(&mut r, OFF_CLUSTER_HEAP_OFFSET, 256);
        put_u32(&mut r, OFF_CLUSTER_COUNT, 3792);
        put_u32(&mut r, OFF_ROOT_CLUSTER, 5);
        put_u32(&mut r, OFF_VOLUME_SERIAL, 0x4474_5a2f);
        r[OFF_REVISION_MINOR] = 0;
        r[OFF_REVISION_MAJOR] = 1;
        r[OFF_BYTES_PER_SECTOR_SHIFT] = 9;
        r[OFF_SECTORS_PER_CLUSTER_SHIFT] = 3;
        r[OFF_NUMBER_OF_FATS] = 1;
        r[OFF_PERCENT_IN_USE] = 52;
        r[OFF_SIGNATURE] = 0x55;
        r[OFF_SIGNATURE + 1] = 0xAA;
        seal(&mut r);
        r
    }

    /// Changes the region with `edit`, seals it so the checksum is not what
    /// fails, and validates it as the main region.
    fn refused(edit: impl FnOnce(&mut Vec<u8>)) -> ExFatError {
        let mut r = valid();
        edit(&mut r);
        seal(&mut r);
        validate_region(&r, BootRegion::Main, extent()).expect_err("the region should be refused")
    }

    #[test]
    fn a_valid_region_is_accepted() {
        let boot = validate_region(&valid(), BootRegion::Main, extent())
            .expect("the synthetic region is valid");

        assert_eq!(boot.region, BootRegion::Main);
        assert_eq!(boot.bytes_per_sector(), 512);
        assert_eq!(boot.cluster_bytes(), 4096);
        assert_eq!(boot.cluster_count, 3792);
        assert_eq!(boot.root_cluster, 5);
        assert_eq!(boot.volume_flags, Some(0));
        assert_eq!(boot.percent_in_use, Some(52));
        assert!(boot.observations.is_empty());
    }

    /// Known answers computed by an independent implementation, the
    /// checksum routine of `scripts/experiments/exp-0009-analyse.py`,
    /// written from the specification's Figure 1.
    #[test]
    fn the_checksum_matches_values_an_independent_implementation_computed() {
        let mut one = vec![0u8; 11 * 512];
        one[0] = 1;
        assert_eq!(boot_checksum(&one, 9), 0x10);

        let pattern: Vec<u8> = (0..11 * 512).map(|i| (i % 251) as u8).collect();
        assert_eq!(boot_checksum(&pattern, 9), 0x7357_a210);

        let wide: Vec<u8> = (0..11 * 4096).map(|i| ((i * 7 + 3) % 256) as u8).collect();
        assert_eq!(boot_checksum(&wide, 12), 0xdb6b_ac4f);
    }

    #[test]
    fn the_checksum_skips_volume_flags_and_percent_in_use() {
        let pattern: Vec<u8> = (0..11 * 512).map(|i| (i % 251) as u8).collect();
        let mut changed = pattern.clone();
        changed[106] = 0xAA;
        changed[107] = 0xBB;
        changed[112] = 0xCC;

        assert_eq!(boot_checksum(&pattern, 9), boot_checksum(&changed, 9));
    }

    #[test]
    fn a_changed_flags_field_does_not_invalidate_the_checksum() {
        let mut r = valid();
        r[OFF_VOLUME_FLAGS] = 0x02;
        r[OFF_PERCENT_IN_USE] = 99;

        assert!(validate_region(&r, BootRegion::Main, extent()).is_ok());
    }

    #[test]
    fn a_wrong_checksum_is_refused() {
        let mut r = valid();
        r[OFF_VOLUME_SERIAL] ^= 1;

        assert!(matches!(
            validate_region(&r, BootRegion::Main, extent()),
            Err(ExFatError::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn a_checksum_sector_that_does_not_repeat_is_refused() {
        let mut r = valid();
        r[11 * 512 + 8] ^= 1;

        assert_eq!(
            validate_region(&r, BootRegion::Main, extent()),
            Err(ExFatError::ChecksumNotRepeated { word: 2 })
        );
    }

    #[test]
    fn a_bad_jump_instruction_is_refused() {
        assert!(matches!(
            refused(|r| r[OFF_JUMP + 1] = 0x77),
            ExFatError::JumpBoot { .. }
        ));
    }

    /// Identification accepts E9 as a jump byte; the parser must not.
    #[test]
    fn the_near_jump_identification_accepts_is_refused_here() {
        assert!(matches!(
            refused(|r| r[OFF_JUMP] = 0xE9),
            ExFatError::JumpBoot {
                found: [0xE9, 0x76, 0x90]
            }
        ));
    }

    #[test]
    fn a_wrong_file_system_name_is_refused() {
        assert!(matches!(
            refused(|r| r[OFF_FILE_SYSTEM_NAME + 7] = b'!'),
            ExFatError::FileSystemName { .. }
        ));
    }

    #[test]
    fn every_must_be_zero_byte_is_checked() {
        for offset in [OFF_MUST_BE_ZERO, OFF_MUST_BE_ZERO + 52] {
            assert_eq!(
                refused(|r| r[offset] = 1),
                ExFatError::MustBeZero { offset },
                "offset {offset}"
            );
        }
    }

    #[test]
    fn a_wrong_boot_signature_is_refused() {
        assert!(matches!(
            refused(|r| r[OFF_SIGNATURE + 1] = 0),
            ExFatError::BootSignature { .. }
        ));
    }

    #[test]
    fn a_sector_size_outside_512_to_4096_is_refused() {
        for value in [0u8, 8, 13, 255] {
            assert_eq!(
                refused(|r| r[OFF_BYTES_PER_SECTOR_SHIFT] = value),
                ExFatError::BytesPerSectorShift { value }
            );
        }
    }

    #[test]
    fn a_cluster_larger_than_32_mib_is_refused() {
        // 9 + 17 is 26, one past the largest sum, 25.
        assert_eq!(
            refused(|r| r[OFF_SECTORS_PER_CLUSTER_SHIFT] = 17),
            ExFatError::SectorsPerClusterShift {
                value: 17,
                maximum: 16
            }
        );
    }

    #[test]
    fn a_number_of_fats_other_than_one_or_two_is_refused() {
        for value in [0u8, 3] {
            assert_eq!(
                refused(|r| r[OFF_NUMBER_OF_FATS] = value),
                ExFatError::NumberOfFats { value }
            );
        }
    }

    #[test]
    fn a_volume_under_one_mebibyte_is_refused() {
        // 2,047 sectors of 512 bytes is one sector short of 1 MiB. The
        // partition is made the same size so that rule is the one that fails.
        let mut r = valid();
        put_u64(&mut r, OFF_VOLUME_LENGTH, 2047);
        seal(&mut r);
        let small = VolumeExtent {
            start_lba: START_LBA,
            sector_count: 2047,
        };

        assert_eq!(
            validate_region(&r, BootRegion::Main, small),
            Err(ExFatError::VolumeLengthTooSmall {
                sectors: 2047,
                minimum: 2048
            })
        );
    }

    #[test]
    fn a_fat_offset_inside_the_boot_regions_is_refused() {
        assert_eq!(
            refused(|r| put_u32(r, OFF_FAT_OFFSET, 23)),
            ExFatError::FatOffsetTooSmall {
                value: 23,
                minimum: 24
            }
        );
    }

    #[test]
    fn a_fat_too_short_for_its_clusters_is_refused() {
        // 3,794 entries of 4 bytes need 30 sectors of 512.
        assert_eq!(
            refused(|r| put_u32(r, OFF_FAT_LENGTH, 29)),
            ExFatError::FatLengthTooSmall {
                value: 29,
                minimum: 30
            }
        );
    }

    #[test]
    fn a_cluster_heap_that_begins_inside_the_fat_is_refused() {
        assert!(matches!(
            refused(|r| put_u32(r, OFF_CLUSTER_HEAP_OFFSET, 159)),
            ExFatError::ClusterHeapBeforeFatEnd { fat_end: 160, .. }
        ));
    }

    #[test]
    fn a_cluster_heap_that_does_not_fit_the_volume_is_refused() {
        // 3,792 clusters of 8 sectors need 30,336, which leaves 256.
        assert!(matches!(
            refused(|r| put_u32(r, OFF_CLUSTER_HEAP_OFFSET, 257)),
            ExFatError::ClusterHeapOffsetTooLarge { maximum: 256, .. }
        ));
    }

    #[test]
    fn a_cluster_count_above_the_most_a_fat_can_describe_is_refused() {
        let mut r = valid();
        // A volume, FAT and heap offset large enough that no other rule
        // fails first: the FAT needs 33,554,430 sectors for this many
        // clusters.
        put_u32(&mut r, OFF_CLUSTER_COUNT, u32::MAX - 9);
        put_u64(&mut r, OFF_VOLUME_LENGTH, 1 << 40);
        put_u32(&mut r, OFF_FAT_LENGTH, 1 << 25);
        put_u32(&mut r, OFF_CLUSTER_HEAP_OFFSET, 40_000_000);
        seal(&mut r);

        assert_eq!(
            validate_region(&r, BootRegion::Main, extent()),
            Err(ExFatError::ClusterCountTooLarge {
                value: u32::MAX - 9
            })
        );
    }

    #[test]
    fn a_cluster_count_below_the_basic_structures_is_refused() {
        // Two clusters, with one FAT: the root, the bitmap and the up-case
        // table need three.
        assert_eq!(
            refused(|r| put_u32(r, OFF_CLUSTER_COUNT, 2)),
            ExFatError::ClusterCountTooSmall {
                value: 2,
                minimum: 3
            }
        );
    }

    #[test]
    fn a_cluster_count_that_is_not_the_one_the_length_implies_is_refused() {
        assert_eq!(
            refused(|r| put_u32(r, OFF_CLUSTER_COUNT, 3791)),
            ExFatError::ClusterCountMismatch {
                value: 3791,
                expected: 3792
            }
        );
    }

    #[test]
    fn a_root_cluster_outside_the_heap_is_refused() {
        for value in [0u32, 1, 3794] {
            assert_eq!(
                refused(|r| put_u32(r, OFF_ROOT_CLUSTER, value)),
                ExFatError::RootClusterOutOfRange { value, last: 3793 }
            );
        }
    }

    #[test]
    fn the_last_cluster_is_a_valid_root() {
        let mut r = valid();
        put_u32(&mut r, OFF_ROOT_CLUSTER, 3793);
        seal(&mut r);

        assert!(validate_region(&r, BootRegion::Main, extent()).is_ok());
    }

    #[test]
    fn a_revision_byte_above_99_is_refused() {
        assert_eq!(
            refused(|r| r[OFF_REVISION_MINOR] = 100),
            ExFatError::RevisionOutOfRange {
                major: 1,
                minor: 100
            }
        );
    }

    #[test]
    fn a_major_revision_other_than_1_is_refused() {
        assert_eq!(
            refused(|r| r[OFF_REVISION_MAJOR] = 2),
            ExFatError::RevisionUnsupported { major: 2, minor: 0 }
        );
        assert_eq!(
            refused(|r| r[OFF_REVISION_MAJOR] = 0),
            ExFatError::RevisionUnsupported { major: 0, minor: 0 }
        );
    }

    #[test]
    fn a_minor_revision_above_zero_is_analysed() {
        let mut r = valid();
        r[OFF_REVISION_MINOR] = 5;
        seal(&mut r);

        let boot = validate_region(&r, BootRegion::Main, extent()).expect("major 1 is analysed");
        assert_eq!(boot.revision_minor, 5);
    }

    #[test]
    fn the_second_fat_active_on_a_volume_with_one_is_refused() {
        // Outside the checksum, so no sealing is needed or wanted.
        let mut r = valid();
        r[OFF_VOLUME_FLAGS] = 1;

        assert_eq!(
            validate_region(&r, BootRegion::Main, extent()),
            Err(ExFatError::ActiveFat { flags: 1 })
        );
    }

    #[test]
    fn a_percent_in_use_above_100_other_than_ff_is_refused() {
        let mut r = valid();
        r[OFF_PERCENT_IN_USE] = 101;
        assert_eq!(
            validate_region(&r, BootRegion::Main, extent()),
            Err(ExFatError::PercentInUse { value: 101 })
        );

        r[OFF_PERCENT_IN_USE] = PERCENT_NOT_AVAILABLE;
        assert!(validate_region(&r, BootRegion::Main, extent()).is_ok());
    }

    #[test]
    fn the_backup_regions_stale_fields_are_neither_checked_nor_kept() {
        let mut r = valid();
        r[OFF_VOLUME_FLAGS] = 1;
        r[OFF_PERCENT_IN_USE] = 200;

        let boot = validate_region(&r, BootRegion::Backup, extent())
            .expect("stale fields do not refuse the backup");
        assert_eq!(boot.region, BootRegion::Backup);
        assert_eq!(boot.volume_flags, None);
        assert_eq!(boot.percent_in_use, None);
    }

    /// Compared in bytes. The same sector count is a different length at
    /// 4,096 bytes a sector than at 512.
    #[test]
    fn a_volume_longer_than_its_partition_is_refused_in_bytes() {
        let mut r = valid();
        put_u64(&mut r, OFF_VOLUME_LENGTH, VOLUME_LENGTH + 1);
        // Keep the cluster count the length implies: 8 more sectors would be
        // needed for one more cluster, so one sector leaves it unchanged.
        seal(&mut r);

        assert_eq!(
            validate_region(&r, BootRegion::Main, extent()),
            Err(ExFatError::VolumeLongerThanPartition {
                volume_bytes: (VOLUME_LENGTH as u128 + 1) * 512,
                partition_bytes: VOLUME_LENGTH * 512,
            })
        );
    }

    #[test]
    fn a_volume_shorter_than_its_partition_is_observed() {
        let longer = VolumeExtent {
            start_lba: START_LBA,
            sector_count: VOLUME_LENGTH as u32 + 100,
        };

        let boot = validate_region(&valid(), BootRegion::Main, longer)
            .expect("a shorter volume is analysed");
        assert_eq!(
            boot.observations,
            vec![ExFatObservation::VolumeShorterThanPartition {
                volume_bytes: VOLUME_LENGTH * 512,
                partition_bytes: (VOLUME_LENGTH + 100) * 512,
            }]
        );
    }

    /// A valid region with 4,096-byte sectors that fills the same partition:
    /// 3,824 of them are 15,663,104 bytes, the partition's length in bytes,
    /// though the count differs from 30,592.
    pub(crate) fn valid_4096() -> Vec<u8> {
        let mut r = vec![0u8; 12 * 4096];
        r[..512].copy_from_slice(&valid()[..512]);
        r[OFF_BYTES_PER_SECTOR_SHIFT] = 12;
        r[OFF_SECTORS_PER_CLUSTER_SHIFT] = 0;
        put_u64(&mut r, OFF_VOLUME_LENGTH, 3824);
        put_u32(&mut r, OFF_FAT_OFFSET, 24);
        put_u32(&mut r, OFF_FAT_LENGTH, 4);
        put_u32(&mut r, OFF_CLUSTER_HEAP_OFFSET, 32);
        put_u32(&mut r, OFF_CLUSTER_COUNT, 3792);
        seal(&mut r);
        r
    }

    #[test]
    fn a_volume_whose_sectors_are_larger_is_compared_by_its_own_size() {
        let r = valid_4096();

        let boot = validate_region(&r, BootRegion::Main, extent())
            .expect("3,824 sectors of 4,096 bytes fill the partition exactly");
        assert_eq!(boot.bytes_per_sector(), 4096);
        assert!(boot.observations.is_empty());
    }

    #[test]
    fn a_partition_offset_that_disagrees_with_the_table_is_not_a_finding() {
        for value in [0u64, 1, 5000, u64::MAX] {
            let mut r = valid();
            put_u64(&mut r, 64, value);
            seal(&mut r);

            let boot = validate_region(&r, BootRegion::Main, extent())
                .expect("every PartitionOffset is valid");
            assert!(boot.observations.is_empty(), "{value}");
        }
    }

    #[test]
    fn two_fats_are_texfat_and_not_analysed() {
        let mut r = valid();
        r[OFF_NUMBER_OF_FATS] = 2;
        // Two FATs of 32 sectors fit before the heap at 256.
        seal(&mut r);

        assert_eq!(
            validate_region(&r, BootRegion::Main, extent()),
            Err(ExFatError::TexFat)
        );
    }

    #[test]
    fn a_malformed_texfat_volume_is_reported_as_malformed() {
        let mut r = valid();
        r[OFF_NUMBER_OF_FATS] = 2;
        put_u32(&mut r, OFF_CLUSTER_COUNT, 1);
        seal(&mut r);

        assert!(matches!(
            validate_region(&r, BootRegion::Main, extent()),
            Err(ExFatError::ClusterCountTooSmall { .. })
        ));
    }

    /// An image holding `region` as the main and `backup` as the backup
    /// region, at the partition's start.
    fn image_with(main: &[u8], backup: &[u8]) -> MemoryImage {
        let start = START_LBA as u64 * 512;
        let mut image = MemoryImage::new(start + VOLUME_LENGTH * 512);
        image.write(start, main);
        image.write(start + 12 * 512, backup);
        image
    }

    #[test]
    fn the_main_region_is_used_when_it_is_valid() {
        let mut image = image_with(&valid(), &valid());

        let boot = parse_boot_region(&mut image, extent()).expect("both regions are valid");
        assert_eq!(boot.region, BootRegion::Main);
    }

    #[test]
    fn the_backup_region_is_used_when_the_main_one_is_refused() {
        let mut main = valid();
        main[OFF_VOLUME_SERIAL] ^= 1;
        let mut image = image_with(&main, &valid());

        let boot = parse_boot_region(&mut image, extent()).expect("the backup is valid");
        assert_eq!(boot.region, BootRegion::Backup);
        assert_eq!(boot.volume_flags, None);
    }

    #[test]
    fn both_regions_refused_keeps_both_reasons() {
        let mut main = valid();
        main[OFF_VOLUME_SERIAL] ^= 1;
        let mut backup = valid();
        backup[OFF_JUMP] = 0;
        seal(&mut backup);
        let mut image = image_with(&main, &backup);

        match parse_boot_region(&mut image, extent()) {
            Err(ExFatBootFailure::Rejected { main, backup }) => {
                assert!(matches!(main, ExFatError::ChecksumMismatch { .. }));
                assert!(matches!(backup, ExFatError::JumpBoot { .. }));
            }
            other => panic!("expected both regions refused, got {other:?}"),
        }
    }

    /// An image with `backup` at the start of the backup region of sectors
    /// `1 << shift` bytes long, and `main` at the volume's start.
    fn image_with_backup_at(main: &[u8], backup: &[u8], shift: u8) -> MemoryImage {
        let start = START_LBA as u64 * 512;
        let mut image = MemoryImage::new(start + VOLUME_LENGTH * 512);
        image.write(start, main);
        image.write(start + (12u64 << shift), backup);
        image
    }

    #[test]
    fn a_zeroed_main_sector_with_a_valid_512_byte_backup_finds_the_backup_by_search() {
        let mut image = image_with_backup_at(&[0u8; 12 * 512], &valid(), 9);

        let boot = parse_boot_region(&mut image, extent()).expect("the backup is found");
        assert_eq!(boot.region, BootRegion::Backup);
        assert!(boot.located_by_search);
        assert_eq!(boot.bytes_per_sector(), 512);
        assert_eq!(boot.volume_flags, None);
    }

    #[test]
    fn a_zeroed_main_sector_with_a_valid_4096_byte_backup_finds_the_backup_by_search() {
        let mut image = image_with_backup_at(&[0u8; 12 * 512], &valid_4096(), 12);

        let boot = parse_boot_region(&mut image, extent()).expect("the backup is found");
        assert_eq!(boot.region, BootRegion::Backup);
        assert!(boot.located_by_search);
        assert_eq!(boot.bytes_per_sector(), 4096);
    }

    #[test]
    fn a_main_sector_of_garbage_finds_the_backup_by_search() {
        let mut main = vec![0u8; 12 * 512];
        let mut state = 7u64;
        for byte in main.iter_mut() {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *byte = (state >> 56) as u8;
        }
        // Whatever the garbage is, its sector size is unusable.
        main[OFF_BYTES_PER_SECTOR_SHIFT] = 200;
        let mut image = image_with_backup_at(&main, &valid(), 9);

        let boot = parse_boot_region(&mut image, extent()).expect("the backup is found");
        assert!(boot.located_by_search);
    }

    /// A backup located from the main region's own sector size is not found
    /// by search.
    #[test]
    fn a_backup_the_main_regions_sector_size_locates_is_not_marked_as_searched() {
        let mut main = valid();
        main[OFF_VOLUME_SERIAL] ^= 1;
        let mut image = image_with(&main, &valid());

        let boot = parse_boot_region(&mut image, extent()).expect("the backup is valid");
        assert!(!boot.located_by_search);
    }

    #[test]
    fn two_valid_candidates_are_refused_as_ambiguous() {
        let start = START_LBA as u64 * 512;
        let mut image = MemoryImage::new(start + VOLUME_LENGTH * 512);
        image.write(start + (12 << 9), &valid());
        image.write(start + (12 << 12), &valid_4096());

        match parse_boot_region(&mut image, extent()) {
            Err(ExFatBootFailure::AmbiguousBackup { shifts }) => assert_eq!(shifts, vec![9, 12]),
            other => panic!("expected the ambiguity reported, got {other:?}"),
        }
    }

    #[test]
    fn no_valid_candidate_reports_the_main_failure_and_an_unlocated_backup() {
        let mut image = image_with_backup_at(&[0u8; 12 * 512], &[0u8; 12 * 512], 9);

        match parse_boot_region(&mut image, extent()) {
            Err(ExFatBootFailure::Rejected { main, backup }) => {
                assert_eq!(main, ExFatError::BytesPerSectorShift { value: 0 });
                assert_eq!(backup, ExFatError::BackupNotLocated);
            }
            other => panic!("expected both failures, got {other:?}"),
        }
    }

    /// A valid 512-byte region sitting where a 4,096-byte backup would start
    /// validates, but declares a different sector size from the one it was
    /// found at, so it is not that candidate.
    #[test]
    fn a_candidate_that_declares_another_sector_size_is_not_accepted() {
        let mut image = image_with_backup_at(&[0u8; 12 * 512], &valid(), 12);

        assert!(matches!(
            parse_boot_region(&mut image, extent()),
            Err(ExFatBootFailure::Rejected {
                backup: ExFatError::BackupNotLocated,
                ..
            })
        ));
    }

    /// An image holding each `(offset, bytes)` pair, the offset counted from
    /// the volume's start.
    pub(crate) fn image_with_regions(regions: &[(u64, &[u8])]) -> MemoryImage {
        let start = START_LBA as u64 * 512;
        let mut image = MemoryImage::new(start + VOLUME_LENGTH * 512);
        for (offset, bytes) in regions {
            image.write(start + offset, bytes);
        }
        image
    }

    /// The main region with one bit of its sector size flipped, 9 to 11: in
    /// range, wrong, and invalidating the checksum. Its own backup offset,
    /// `12 << 11`, holds nothing.
    #[test]
    fn a_main_sector_size_flipped_to_another_in_range_value_finds_the_backup_by_search() {
        let mut main = valid();
        main[OFF_BYTES_PER_SECTOR_SHIFT] ^= 0b10;
        assert_eq!(main[OFF_BYTES_PER_SECTOR_SHIFT], 11);
        let mut image = image_with_regions(&[(0, &main), (12 << 9, &valid())]);

        let boot = parse_boot_region(&mut image, extent()).expect("the backup is found");
        assert_eq!(boot.region, BootRegion::Backup);
        assert!(boot.located_by_search);
        assert_eq!(boot.bytes_per_sector(), 512);
    }

    #[test]
    fn a_refused_main_and_a_refused_backup_at_its_own_offset_search_the_other_sizes() {
        let mut main = valid();
        main[OFF_VOLUME_SERIAL] ^= 1;
        let mut bad_backup = valid();
        bad_backup[OFF_VOLUME_SERIAL] ^= 1;
        let mut image = image_with_regions(&[
            (0, &main),
            (12 << 9, &bad_backup),
            (12 << 12, &valid_4096()),
        ]);

        let boot = parse_boot_region(&mut image, extent()).expect("another candidate is valid");
        assert_eq!(boot.region, BootRegion::Backup);
        assert!(boot.located_by_search);
        assert_eq!(boot.bytes_per_sector(), 4096);
    }

    /// Another valid candidate is present, and is not looked for: finding
    /// both would be an ambiguity that a normally located backup does not
    /// have.
    #[test]
    fn a_backup_valid_at_its_own_offset_is_used_without_a_search() {
        let mut main = valid();
        main[OFF_VOLUME_SERIAL] ^= 1;
        let mut image =
            image_with_regions(&[(0, &main), (12 << 9, &valid()), (12 << 12, &valid_4096())]);

        let boot = parse_boot_region(&mut image, extent()).expect("the backup is valid");
        assert_eq!(boot.region, BootRegion::Backup);
        assert!(!boot.located_by_search);
        assert_eq!(boot.bytes_per_sector(), 512);
    }

    #[test]
    fn when_no_candidate_is_valid_the_main_and_the_normally_located_backup_errors_are_kept() {
        let mut main = valid();
        main[OFF_VOLUME_SERIAL] ^= 1;
        let mut bad_backup = valid();
        bad_backup[OFF_JUMP] = 0;
        seal(&mut bad_backup);
        let mut image = image_with_regions(&[(0, &main), (12 << 9, &bad_backup)]);

        match parse_boot_region(&mut image, extent()) {
            Err(ExFatBootFailure::Rejected { main, backup }) => {
                assert!(matches!(main, ExFatError::ChecksumMismatch { .. }));
                assert!(matches!(backup, ExFatError::JumpBoot { .. }));
            }
            other => panic!("expected both failures, got {other:?}"),
        }
    }

    /// The main region's own sector size is skipped by the search, so two
    /// others are the two candidates.
    #[test]
    fn two_valid_candidates_are_ambiguous_after_a_refused_in_range_main() {
        let mut main = valid();
        main[OFF_BYTES_PER_SECTOR_SHIFT] ^= 0b10;
        let mut image =
            image_with_regions(&[(0, &main), (12 << 9, &valid()), (12 << 12, &valid_4096())]);

        match parse_boot_region(&mut image, extent()) {
            Err(ExFatBootFailure::AmbiguousBackup { shifts }) => assert_eq!(shifts, vec![9, 12]),
            other => panic!("expected the ambiguity reported, got {other:?}"),
        }
    }

    #[test]
    fn a_candidate_that_does_not_fit_the_partition_is_not_read() {
        // 190 sectors cannot hold two regions of 4,096-byte sectors, which
        // need 192.
        let small = VolumeExtent {
            start_lba: START_LBA,
            sector_count: 190,
        };
        let mut image = image_with_backup_at(&[0u8; 12 * 512], &valid_4096(), 12);

        assert!(matches!(
            parse_boot_region(&mut image, small),
            Err(ExFatBootFailure::Rejected {
                backup: ExFatError::BackupNotLocated,
                ..
            })
        ));
    }

    /// A valid region that declares two FATs: a TexFAT volume, sealed.
    pub(crate) fn texfat() -> Vec<u8> {
        let mut r = valid();
        r[OFF_NUMBER_OF_FATS] = 2;
        seal(&mut r);
        r
    }

    #[test]
    fn a_texfat_candidate_found_by_search_is_texfat() {
        let mut image = image_with_backup_at(&[0u8; 12 * 512], &texfat(), 9);

        assert!(matches!(
            parse_boot_region(&mut image, extent()),
            Err(ExFatBootFailure::TexFat(BootRegion::Backup))
        ));
    }

    #[test]
    fn a_texfat_main_region_does_not_fall_back_to_the_backup() {
        let mut main = valid();
        main[OFF_NUMBER_OF_FATS] = 2;
        seal(&mut main);
        let mut image = image_with(&main, &valid());

        assert!(matches!(
            parse_boot_region(&mut image, extent()),
            Err(ExFatBootFailure::TexFat(BootRegion::Main))
        ));
    }

    #[test]
    fn a_texfat_backup_is_texfat_when_the_main_region_is_refused() {
        let mut main = valid();
        main[OFF_VOLUME_SERIAL] ^= 1;
        let mut backup = valid();
        backup[OFF_NUMBER_OF_FATS] = 2;
        seal(&mut backup);
        let mut image = image_with(&main, &backup);

        assert!(matches!(
            parse_boot_region(&mut image, extent()),
            Err(ExFatBootFailure::TexFat(BootRegion::Backup))
        ));
    }

    #[test]
    fn a_partition_too_small_for_the_regions_is_refused_without_a_read_past_it() {
        // 23 sectors hold the main region and part of the backup.
        let small = VolumeExtent {
            start_lba: START_LBA,
            sector_count: 23,
        };
        let mut image = image_with(&valid(), &valid());

        match parse_boot_region(&mut image, small) {
            Err(ExFatBootFailure::Rejected { backup, .. }) => assert!(matches!(
                backup,
                ExFatError::RegionOutsidePartition {
                    required: 12288,
                    available: 11776
                }
            )),
            Ok(boot) => {
                // The main region is valid on its own, and a volume longer
                // than this partition is refused for that reason first.
                panic!("the volume is longer than the partition: {boot:?}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn a_short_region_buffer_is_refused_not_indexed() {
        for len in [0usize, 100, 511, 12 * 512 - 1] {
            let buffer = vec![0u8; len];
            assert!(
                validate_region(&buffer, BootRegion::Main, extent()).is_err(),
                "{len}"
            );
        }
    }

    #[test]
    fn arbitrary_bytes_never_panic() {
        // Deterministic pseudo-random regions, and the valid one with each
        // single byte of its first sector flipped.
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..200 {
            let mut r = vec![0u8; 12 * 512];
            for byte in r.iter_mut() {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                *byte = (state >> 56) as u8;
            }
            let _ = validate_region(&r, BootRegion::Main, extent());
        }

        for at in 0..512 {
            let mut r = valid();
            r[at] ^= 0xFF;
            let _ = validate_region(&r, BootRegion::Main, extent());
            seal(&mut r);
            let _ = validate_region(&r, BootRegion::Main, extent());
        }
    }
}
