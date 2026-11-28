//! GUID Partition Table headers, entries and the checks that decide which of
//! them is believed. `ADR-0025`.
//!
//! Nothing here writes: a damaged header is reported and never restored.
//! Logical blocks are 512 bytes; a disk with 4096-byte blocks is reported and
//! not read (Decision I).
//!
//! Every length and offset comes from the disk, so each is checked before it
//! sizes a buffer or addresses a read. The entry array is the only buffer the
//! disk sizes, and it is bounded by [`ARRAY_BOUND`].

use std::fmt;

use crate::error::Error;
use crate::evidence::EvidenceReader;
use crate::partition::{Anomaly, SECTOR_SIZE};

/// The project's bound on an entry array, in bytes. The specification sets
/// none; an unbounded count from a damaged header would size a buffer of
/// gigabytes. 1 MiB holds 8192 entries of the usual 128 bytes, and keeps a
/// partition number inside `u16`. `ADR-0025` Decision C.
pub const ARRAY_BOUND: u64 = 1 << 20;

/// The signature of a header, "EFI PART".
const SIGNATURE: [u8; 8] = *b"EFI PART";

/// The only revision the specification defines (5.3.2, table 5.5).
const REVISION: u32 = 0x0001_0000;

/// Smallest and largest header size accepted. The smallest is the
/// specification's; the largest is one 512-byte block, as the specification
/// bounds it by the block size.
const MIN_HEADER_SIZE: u32 = 92;
const MAX_HEADER_SIZE: u32 = SECTOR_SIZE as u32;

/// Largest entry size accepted. The specification allows 128 x 2^n; the
/// limit is the project's, as no tool measured accepts a size beyond 128 and
/// a larger one only enlarges the array.
const MAX_ENTRY_SIZE: u32 = 512;
const MIN_ENTRY_SIZE: u32 = 128;

/// Bytes at the start of the name field and the name's length in UTF-16
/// units: an entry's name is bytes 56 to 127.
const NAME_START: usize = 56;
const NAME_END: usize = 128;

/// The 16 bytes of the partition type GUID that means an unused entry.
const UNUSED: [u8; 16] = [0; 16];

/// Microsoft basic data, EBD0A0A2-B9E5-4433-87C0-68B6B72699C7, as stored.
pub const BASIC_DATA: [u8; 16] = [
    0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7,
];

/// EFI System Partition, C12A7328-F81F-11D2-BA4B-00A0C93EC93B, as stored.
const EFI_SYSTEM: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];

/// Partition containing a legacy MBR, 024DEE41-33E7-11D3-9D69-0008C781F39F,
/// as stored.
const LEGACY_MBR: [u8; 16] = [
    0x41, 0xEE, 0x4D, 0x02, 0xE7, 0x33, 0xD3, 0x11, 0x9D, 0x69, 0x00, 0x08, 0xC7, 0x81, 0xF3, 0x9F,
];

/// A GUID as the specification writes it: the first three fields are stored
/// little-endian, the last two as they are.
pub fn guid_text(guid: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        guid[3],
        guid[2],
        guid[1],
        guid[0],
        guid[5],
        guid[4],
        guid[7],
        guid[6],
        guid[8],
        guid[9],
        guid[10],
        guid[11],
        guid[12],
        guid[13],
        guid[14],
        guid[15]
    )
}

/// The name this project gives a partition type GUID: the types of the
/// specification's table 5.7 and Microsoft basic data.
pub fn type_name(guid: &[u8; 16]) -> &'static str {
    if *guid == BASIC_DATA {
        "Microsoft basic data"
    } else if *guid == EFI_SYSTEM {
        "EFI System Partition"
    } else if *guid == LEGACY_MBR {
        "partition containing a legacy MBR"
    } else {
        "unknown"
    }
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut bit = 0;
        while bit < 8 {
            // 0xEDB88320 is the polynomial 0x04C11DB7 reflected, as the
            // reflected algorithm needs. `ADR-0025` Decision H.
            c = if c & 1 == 1 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            bit += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

/// CRC-32 as Linux's `efi_crc32` and libblkid compute it: initial value
/// `0xFFFFFFFF`, reflected input and output, final XOR `0xFFFFFFFF`.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc = CRC_TABLE[usize::from((crc as u8) ^ byte)] ^ (crc >> 8);
    }
    !crc
}

/// Which of the two headers a header is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    Primary,
    Backup,
}

/// Why a header is not valid: each check of `ADR-0025` Decision C has its
/// own reason.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HeaderFailure {
    /// The header's block lies outside the image.
    OutsideImage,
    /// Reading the header or its array failed.
    Unreadable(String),
    /// The signature is not "EFI PART".
    Signature,
    /// HeaderSize is outside 92 to 512.
    HeaderSize { size: u32 },
    /// The CRC over HeaderSize bytes does not match.
    HeaderCrc,
    /// MyLBA is not the LBA the header was read from.
    MyLba { found: u64, expected: u64 },
    /// FirstUsableLBA is after LastUsableLBA.
    UsableRange,
    /// SizeOfPartitionEntry is not 128 x 2^n up to 512.
    EntrySize { size: u32 },
    /// The array is larger than the project's bound.
    ArrayBound { bytes: u64 },
    /// The array is not where the specification places it, or is not inside
    /// the image.
    ArrayPlace,
    /// The array's CRC does not match.
    ArrayCrc,
}

impl fmt::Display for HeaderFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutsideImage => f.write_str("the header's block is outside the image"),
            Self::Unreadable(error) => write!(f, "unreadable: {error}"),
            Self::Signature => f.write_str("signature is not \"EFI PART\""),
            Self::HeaderSize { size } => {
                write!(
                    f,
                    "HeaderSize {size} is outside {MIN_HEADER_SIZE} to {MAX_HEADER_SIZE}"
                )
            }
            Self::HeaderCrc => f.write_str("header CRC does not match"),
            Self::MyLba { found, expected } => {
                write!(f, "MyLBA is {found}, the header was read from {expected}")
            }
            Self::UsableRange => f.write_str("FirstUsableLBA is after LastUsableLBA"),
            Self::EntrySize { size } => write!(
                f,
                "SizeOfPartitionEntry {size} is not 128 x 2^n up to {MAX_ENTRY_SIZE}"
            ),
            Self::ArrayBound { bytes } => {
                write!(f, "entry array of {bytes} bytes exceeds the bound")
            }
            Self::ArrayPlace => {
                f.write_str("entry array is not where the specification places it in the image")
            }
            Self::ArrayCrc => f.write_str("entry array CRC does not match"),
        }
    }
}

/// A header whose own fields passed `ADR-0025` Decision C.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Header {
    revision: u32,
    reserved_non_zero: bool,
    my_lba: u64,
    alternate_lba: u64,
    first_usable: u64,
    last_usable: u64,
    disk_guid: [u8; 16],
    entries_lba: u64,
    entry_count: u32,
    entry_size: u32,
    array_crc: u32,
}

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    let mut word = [0u8; 4];
    word.copy_from_slice(&bytes[at..at + 4]);
    u32::from_le_bytes(word)
}

fn le_u64(bytes: &[u8], at: usize) -> u64 {
    let mut word = [0u8; 8];
    word.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(word)
}

/// Checks a header block against everything `ADR-0025` Decision C asks of a
/// header except its array's contents. `bound` is the largest array accepted.
fn parse_header(
    block: &[u8; SECTOR_SIZE],
    read_from: u64,
    role: Role,
    image_sectors: u64,
    bound: u64,
) -> Result<Header, HeaderFailure> {
    if block[..8] != SIGNATURE {
        return Err(HeaderFailure::Signature);
    }

    let size = le_u32(block, 12);
    if !(MIN_HEADER_SIZE..=MAX_HEADER_SIZE).contains(&size) {
        return Err(HeaderFailure::HeaderSize { size });
    }
    let size = size as usize;

    let mut covered = block[..size].to_vec();
    covered[16..20].fill(0);
    if crc32(&covered) != le_u32(block, 16) {
        return Err(HeaderFailure::HeaderCrc);
    }

    let my_lba = le_u64(block, 24);
    if my_lba != read_from {
        return Err(HeaderFailure::MyLba {
            found: my_lba,
            expected: read_from,
        });
    }

    let first_usable = le_u64(block, 40);
    let last_usable = le_u64(block, 48);
    if first_usable > last_usable {
        return Err(HeaderFailure::UsableRange);
    }

    let entry_size = le_u32(block, 84);
    let entry_count = le_u32(block, 80);
    if !(MIN_ENTRY_SIZE..=MAX_ENTRY_SIZE).contains(&entry_size) || !entry_size.is_power_of_two() {
        return Err(HeaderFailure::EntrySize { size: entry_size });
    }

    let array_bytes = u64::from(entry_count) * u64::from(entry_size);
    if array_bytes > bound {
        return Err(HeaderFailure::ArrayBound { bytes: array_bytes });
    }

    // The primary's array follows its header and ends before the usable
    // range; the backup's follows the usable range and ends before its
    // header (UEFI 5.3.2). Both must lie in the image.
    let entries_lba = le_u64(block, 72);
    let array_sectors = array_bytes.div_ceil(SECTOR_SIZE as u64);
    let placed = entries_lba.checked_add(array_sectors).is_some_and(|end| {
        end <= image_sectors
            && match role {
                Role::Primary => entries_lba > my_lba && end <= first_usable,
                Role::Backup => entries_lba > last_usable && end <= my_lba,
            }
    });
    if !placed {
        return Err(HeaderFailure::ArrayPlace);
    }

    Ok(Header {
        revision: le_u32(block, 8),
        reserved_non_zero: block[20..24].iter().any(|&b| b != 0)
            || block[size..].iter().any(|&b| b != 0),
        my_lba,
        alternate_lba: le_u64(block, 32),
        first_usable,
        last_usable,
        disk_guid: {
            let mut guid = [0u8; 16];
            guid.copy_from_slice(&block[56..72]);
            guid
        },
        entries_lba,
        entry_count,
        entry_size,
        array_crc: le_u32(block, 88),
    })
}

fn unreadable(error: &Error) -> HeaderFailure {
    HeaderFailure::Unreadable(error.to_string())
}

/// Reads the header at `lba` and its entry array, and checks both.
fn read_header<R: EvidenceReader>(
    evidence: &mut R,
    lba: u64,
    role: Role,
    image_sectors: u64,
    bound: u64,
) -> Result<(Header, Vec<u8>), HeaderFailure> {
    if lba >= image_sectors {
        return Err(HeaderFailure::OutsideImage);
    }

    let mut block = [0u8; SECTOR_SIZE];
    evidence
        .read_exact_at(lba * SECTOR_SIZE as u64, &mut block)
        .map_err(|e| unreadable(&e))?;
    let header = parse_header(&block, lba, role, image_sectors, bound)?;

    // `parse_header` bounded the array by `bound`, so it fits a buffer.
    let bytes = usize::try_from(u64::from(header.entry_count) * u64::from(header.entry_size))
        .map_err(|_| HeaderFailure::ArrayPlace)?;
    let mut array = vec![0u8; bytes];
    evidence
        .read_exact_at(header.entries_lba * SECTOR_SIZE as u64, &mut array)
        .map_err(|e| unreadable(&e))?;
    if crc32(&array) != header.array_crc {
        return Err(HeaderFailure::ArrayCrc);
    }

    Ok((header, array))
}

/// Which header was believed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HeaderUsed {
    Primary,
    /// The backup, because the primary failed this check.
    Backup {
        primary: HeaderFailure,
    },
}

/// A partition entry that is used and was kept.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    /// The entry's index plus one, as Linux and libblkid number it.
    /// `ADR-0025` Decision G.
    pub number: u16,
    pub type_guid: [u8; 16],
    pub start_lba: u64,
    /// The last sector of the partition, inclusive.
    pub end_lba: u64,
    /// The name's UTF-16 units up to the first zero unit.
    pub name: Vec<u16>,
}

impl Entry {
    pub const fn is_basic_data(&self) -> bool {
        matches!(self.type_guid, BASIC_DATA)
    }

    /// The sectors the partition holds. Never overflows: the entry was kept
    /// only inside the image.
    pub const fn sector_count(&self) -> u64 {
        self.end_lba - self.start_lba + 1
    }
}

/// A GPT that was read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Table {
    pub disk_guid: [u8; 16],
    pub used: HeaderUsed,
    /// Used entries that describe sectors of the image, in index order.
    pub entries: Vec<Entry>,
    pub anomalies: Vec<Anomaly>,
}

/// Why no GPT was read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NotAnalysed {
    /// `ADR-0025` Decision I.
    FourKnBlocks,
    /// Neither header is valid (Decision D).
    NoValidHeader {
        primary: HeaderFailure,
        backup: HeaderFailure,
    },
}

impl fmt::Display for NotAnalysed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FourKnBlocks => f.write_str("logical blocks of 4096 bytes are not read"),
            Self::NoValidHeader { primary, backup } => {
                write!(f, "primary header: {primary}; backup header: {backup}")
            }
        }
    }
}

/// Whether LBA 1 holds no header but byte 4096 does: a GPT whose logical
/// blocks are 4096 bytes. `ADR-0025` Decision I.
fn has_four_kn_header<R: EvidenceReader>(evidence: &mut R, image_sectors: u64) -> bool {
    let mut signature = [0u8; 8];
    // 4104 bytes: the signature at byte 4096 must lie in the image.
    image_sectors * SECTOR_SIZE as u64 >= 4104
        && evidence.read_exact_at(4096, &mut signature).is_ok()
        && signature == SIGNATURE
}

/// What LBA 1 says about a GPT left behind a legacy MBR (`ADR-0025`
/// Decision B): [`Anomaly::GptBehindLegacyMbr`] where it holds a header that
/// passes Decision C, [`Anomaly::GptCheckNotMade`] where it, or the array it
/// names, could not be read, and nothing where there is no valid header: a
/// missing or invalid header is the usual case and is silent.
pub fn stale_header_finding<R: EvidenceReader>(
    evidence: &mut R,
    image_sectors: u64,
) -> Option<Anomaly> {
    match read_header(evidence, 1, Role::Primary, image_sectors, ARRAY_BOUND) {
        Ok(_) => Some(Anomaly::GptBehindLegacyMbr),
        Err(HeaderFailure::Unreadable(reason)) => Some(Anomaly::GptCheckNotMade { reason }),
        Err(_) => None,
    }
}

/// Reads the GPT of an image of `image_sectors` sectors.
///
/// `bound` is the largest entry array accepted, in bytes; the project's
/// [`ARRAY_BOUND`] is the most it can be, and a test can make it smaller.
pub fn read<R: EvidenceReader>(
    evidence: &mut R,
    image_sectors: u64,
    bound: u64,
) -> Result<Table, NotAnalysed> {
    let bound = bound.min(ARRAY_BOUND);
    let mut anomalies = Vec::new();

    let (header, array, used) = match read_header(evidence, 1, Role::Primary, image_sectors, bound)
    {
        Ok((header, array)) => {
            compare_backups(evidence, &header, image_sectors, bound, &mut anomalies);
            (header, array, HeaderUsed::Primary)
        }
        Err(primary) => {
            if primary == HeaderFailure::Signature && has_four_kn_header(evidence, image_sectors) {
                return Err(NotAnalysed::FourKnBlocks);
            }
            // LBA 0 is the MBR and LBA 1 the primary, so the last LBA is a
            // backup only on an image of three sectors or more.
            let backup = match image_sectors.checked_sub(1) {
                Some(last) if last > 1 => {
                    read_header(evidence, last, Role::Backup, image_sectors, bound)
                }
                _ => Err(HeaderFailure::OutsideImage),
            };
            match backup {
                Ok((header, array)) => (header, array, HeaderUsed::Backup { primary }),
                Err(backup) => return Err(NotAnalysed::NoValidHeader { primary, backup }),
            }
        }
    };

    if header.revision != REVISION {
        anomalies.push(Anomaly::GptRevision {
            revision: header.revision,
        });
    }
    if header.reserved_non_zero {
        anomalies.push(Anomaly::GptReservedNonZero);
    }
    if header.last_usable >= image_sectors {
        anomalies.push(Anomaly::GptUsableBeyondEnd {
            last_usable: header.last_usable,
            image_sectors,
        });
    }

    let entries = read_entries(&header, &array, image_sectors, &mut anomalies);
    Ok(Table {
        disk_guid: header.disk_guid,
        used,
        entries,
        anomalies,
    })
}

/// `ADR-0025` Decision D: the backup is read at the primary's AlternateLBA
/// and, where that is not the image's last LBA, at the last LBA too. A backup
/// that is missing, invalid or disagrees with the primary is an anomaly.
fn compare_backups<R: EvidenceReader>(
    evidence: &mut R,
    primary: &Header,
    image_sectors: u64,
    bound: u64,
    anomalies: &mut Vec<Anomaly>,
) {
    let last = image_sectors - 1;
    let alternate = primary.alternate_lba;

    if alternate <= 1 || alternate >= image_sectors {
        anomalies.push(Anomaly::GptBackupMissing {
            alternate_lba: alternate,
        });
    } else {
        match read_header(evidence, alternate, Role::Backup, image_sectors, bound) {
            Ok((backup, _)) => compare(primary, &backup, true, anomalies),
            Err(reason) => anomalies.push(Anomaly::GptBackupInvalid {
                lba: alternate,
                reason,
            }),
        }
    }

    if alternate != last {
        anomalies.push(Anomaly::GptBackupNotAtLastLba {
            alternate_lba: alternate,
            last_lba: last,
        });
        // A header at the last LBA that is not the one the primary names is
        // compared, but not on MyLBA and AlternateLBA, which differ there by
        // construction and are what the finding above already says; one
        // that is not a header adds nothing to it.
        if last > 1
            && let Ok((backup, _)) = read_header(evidence, last, Role::Backup, image_sectors, bound)
        {
            compare(primary, &backup, false, anomalies);
        }
    }
}

/// Compares a backup with the primary. `named` says the backup is the
/// header the primary's AlternateLBA names; only then do MyLBA and
/// AlternateLBA have to name each other.
fn compare(primary: &Header, backup: &Header, named: bool, anomalies: &mut Vec<Anomaly>) {
    let checks = [
        (
            "MyLBA and AlternateLBA",
            named
                && (backup.my_lba != primary.alternate_lba
                    || backup.alternate_lba != primary.my_lba),
        ),
        (
            "usable range",
            (backup.first_usable, backup.last_usable)
                != (primary.first_usable, primary.last_usable),
        ),
        ("DiskGUID", backup.disk_guid != primary.disk_guid),
        ("entry count", backup.entry_count != primary.entry_count),
        ("entry size", backup.entry_size != primary.entry_size),
        ("entry array CRC", backup.array_crc != primary.array_crc),
    ];
    for (field, differs) in checks {
        if differs {
            anomalies.push(Anomaly::GptHeadersDisagree { field });
        }
    }
}

/// `ADR-0025` Decision E.
fn read_entries(
    header: &Header,
    array: &[u8],
    image_sectors: u64,
    anomalies: &mut Vec<Anomaly>,
) -> Vec<Entry> {
    let mut kept = Vec::new();
    let size = header.entry_size as usize;

    for (index, raw) in array.chunks_exact(size).enumerate() {
        // The array holds at most `ARRAY_BOUND / 128` entries, so the number
        // fits; an array that somehow held more stops here.
        let Ok(number) = u16::try_from(index + 1) else {
            break;
        };

        let mut type_guid = [0u8; 16];
        type_guid.copy_from_slice(&raw[..16]);
        if type_guid == UNUSED {
            if raw.iter().any(|&b| b != 0) {
                anomalies.push(Anomaly::GptUnusedEntryNotZero { number });
            }
            continue;
        }

        let start = le_u64(raw, 32);
        let end = le_u64(raw, 40);
        if start > end {
            anomalies.push(Anomaly::GptEntryStartsAfterEnd { number, start, end });
            continue;
        }
        if end >= image_sectors {
            anomalies.push(Anomaly::GptEntryBeyondEnd {
                number,
                start,
                end,
                image_sectors,
            });
            continue;
        }
        // shortcut: the analysis counts a volume's sectors in `u32`, so an
        // entry reaching past 2 TiB is left out and reported, not analysed;
        // upgrade when `VolumeExtent` widens.
        if u32::try_from(start).is_err() || u32::try_from(end - start + 1).is_err() {
            anomalies.push(Anomaly::GptEntryTooLarge { number, start, end });
            continue;
        }
        if start < header.first_usable || end > header.last_usable {
            anomalies.push(Anomaly::GptEntryOutsideUsable {
                number,
                start,
                end,
                first_usable: header.first_usable,
                last_usable: header.last_usable,
            });
        }

        let name_end = NAME_END.min(raw.len());
        let units: Vec<u16> = raw[NAME_START..name_end]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .take_while(|&unit| unit != 0)
            .collect();
        kept.push(Entry {
            number,
            type_guid,
            start_lba: start,
            end_lba: end,
            name: units,
        });
    }

    report_overlaps(&kept, anomalies);
    kept
}

/// Reports entries that share a sector. Sorted by start, each entry is
/// compared with the entry reaching furthest before it, so a table of
/// thousands of entries yields at most one finding per entry and costs no
/// more than a sort.
fn report_overlaps(entries: &[Entry], anomalies: &mut Vec<Anomaly>) {
    let mut order: Vec<&Entry> = entries.iter().collect();
    order.sort_by_key(|e| (e.start_lba, e.number));

    let mut reaching: Option<&Entry> = None;
    for entry in order {
        if let Some(previous) = reaching
            && entry.start_lba <= previous.end_lba
        {
            anomalies.push(Anomaly::GptOverlappingEntries {
                first: previous.number.min(entry.number),
                second: previous.number.max(entry.number),
            });
        }
        if reaching.is_none_or(|previous| entry.end_lba > previous.end_lba) {
            reaching = Some(entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::tests::MemoryImage;

    const SECTORS: u64 = 2048;
    const COUNT: usize = 128;
    const FIRST_USABLE: u64 = 34;
    const LAST_USABLE: u64 = SECTORS - 34;
    const BACKUP_LBA: u64 = SECTORS - 1;
    const BACKUP_ARRAY_LBA: u64 = SECTORS - 33;

    fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u64(bytes: &mut [u8], at: usize, value: u64) {
        bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }

    /// A disk of `SECTORS` sectors with both headers valid and one basic-data
    /// entry at sectors 100 to 199.
    struct Disk(Vec<u8>);

    impl Disk {
        fn new() -> Self {
            let mut disk = Self(vec![0u8; SECTORS as usize * SECTOR_SIZE]);
            disk.entry(0, BASIC_DATA, 100, 199);
            disk.reseal();
            disk
        }

        fn at(&self, lba: u64) -> usize {
            lba as usize * SECTOR_SIZE
        }

        fn entry(&mut self, index: usize, type_guid: [u8; 16], start: u64, end: u64) {
            for array_lba in [2, BACKUP_ARRAY_LBA] {
                let base = self.at(array_lba) + index * 128;
                self.0[base..base + 16].copy_from_slice(&type_guid);
                self.0[base + 16] = 0x11;
                put_u64(&mut self.0, base + 32, start);
                put_u64(&mut self.0, base + 40, end);
                for (i, unit) in "Data".encode_utf16().enumerate() {
                    self.0[base + 56 + i * 2..][..2].copy_from_slice(&unit.to_le_bytes());
                }
            }
        }

        /// Writes both headers from the fields and the arrays, with every CRC
        /// recomputed. Damage is applied after this, or before it where the
        /// damage is to a field the CRC must cover.
        fn reseal(&mut self) {
            for (lba, alternate, entries, first, last) in [
                (1, BACKUP_LBA, 2, FIRST_USABLE, LAST_USABLE),
                (BACKUP_LBA, 1, BACKUP_ARRAY_LBA, FIRST_USABLE, LAST_USABLE),
            ] {
                let base = self.at(lba);
                let array = self.at(entries);
                let array_crc = crc32(&self.0[array..array + COUNT * 128]);
                let header = &mut self.0[base..base + SECTOR_SIZE];
                header[..8].copy_from_slice(&SIGNATURE);
                put_u32(header, 8, REVISION);
                put_u32(header, 12, 92);
                put_u64(header, 24, lba);
                put_u64(header, 32, alternate);
                put_u64(header, 40, first);
                put_u64(header, 48, last);
                header[56..72].copy_from_slice(&[0x5A; 16]);
                put_u64(header, 72, entries);
                put_u32(header, 80, COUNT as u32);
                put_u32(header, 84, 128);
                put_u32(header, 88, array_crc);
                self.crc(lba);
            }
        }

        fn crc(&mut self, lba: u64) {
            let base = self.at(lba);
            let size = le_u32(&self.0[base..], 12) as usize;
            self.0[base + 16..base + 20].fill(0);
            let crc = crc32(&self.0[base..base + size]);
            put_u32(&mut self.0, base + 16, crc);
        }

        fn image(&self) -> MemoryImage {
            let mut image = MemoryImage::new(self.0.len() as u64);
            image.write(0, &self.0);
            image
        }

        fn read(&self) -> Result<Table, NotAnalysed> {
            read(&mut self.image(), SECTORS, ARRAY_BOUND)
        }

        fn table(&self) -> Table {
            self.read().expect("a GPT the test built should be read")
        }
    }

    fn primary_failure(disk: &Disk) -> HeaderFailure {
        match disk.table().used {
            HeaderUsed::Backup { primary } => primary,
            HeaderUsed::Primary => panic!("the primary should have been refused"),
        }
    }

    #[test]
    fn crc32_matches_the_check_value_of_the_algorithm() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    /// The fixture's own stored values: the header over 92 bytes with its CRC
    /// field zeroed, and the array of 128 entries.
    #[test]
    fn crc32_matches_a_real_disks_header_and_array() {
        let image = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures/partition/gpt-protective.img"),
        )
        .expect("the fixture is in the repository");
        let mut header = image[512..512 + 92].to_vec();
        assert_eq!(le_u32(&header, 16), 0xad42_145d);
        header[16..20].fill(0);
        assert_eq!(crc32(&header), 0xad42_145d);
        assert_eq!(crc32(&image[1024..1024 + 128 * 128]), 0x676f_a7f8);
    }

    #[test]
    fn guid_text_swaps_the_first_three_fields() {
        assert_eq!(
            guid_text(&BASIC_DATA),
            "ebd0a0a2-b9e5-4433-87c0-68b6b72699c7"
        );
        assert_eq!(
            guid_text(&EFI_SYSTEM),
            "c12a7328-f81f-11d2-ba4b-00a0c93ec93b"
        );
        assert_eq!(type_name(&BASIC_DATA), "Microsoft basic data");
        assert_eq!(type_name(&[9; 16]), "unknown");
    }

    #[test]
    fn a_healthy_disk_uses_the_primary_and_reports_nothing() {
        let table = Disk::new().table();
        assert_eq!(table.used, HeaderUsed::Primary);
        assert_eq!(table.anomalies, []);
        assert_eq!(table.disk_guid, [0x5A; 16]);
        let [entry] = table.entries.as_slice() else {
            panic!("one entry expected");
        };
        assert_eq!(
            (entry.number, entry.start_lba, entry.end_lba),
            (1, 100, 199)
        );
        assert_eq!(entry.sector_count(), 100);
        assert_eq!(entry.name, "Data".encode_utf16().collect::<Vec<_>>());
        assert!(entry.is_basic_data());
    }

    // Decision C: one test per check, each damaging only that.

    #[test]
    fn a_wrong_signature_fails_the_primary() {
        let mut disk = Disk::new();
        disk.0[512] = b'X';
        assert_eq!(primary_failure(&disk), HeaderFailure::Signature);
    }

    #[test]
    fn a_header_size_outside_92_to_512_fails() {
        for size in [91u32, 513] {
            let mut disk = Disk::new();
            put_u32(&mut disk.0, 512 + 12, size);
            assert_eq!(primary_failure(&disk), HeaderFailure::HeaderSize { size });
        }
    }

    #[test]
    fn a_header_size_of_512_is_accepted_when_its_crc_covers_512_bytes() {
        let mut disk = Disk::new();
        put_u32(&mut disk.0, 512 + 12, 512);
        disk.crc(1);
        assert_eq!(disk.table().used, HeaderUsed::Primary);
    }

    #[test]
    fn a_header_crc_mismatch_fails() {
        let mut disk = Disk::new();
        disk.0[512 + 56] ^= 0xFF;
        assert_eq!(primary_failure(&disk), HeaderFailure::HeaderCrc);
    }

    #[test]
    fn the_crc_covers_header_size_bytes_not_92() {
        let mut disk = Disk::new();
        put_u32(&mut disk.0, 512 + 12, 128);
        disk.crc(1);
        disk.0[512 + 100] = 1;
        assert_eq!(primary_failure(&disk), HeaderFailure::HeaderCrc);
    }

    #[test]
    fn my_lba_must_be_where_the_header_was_read() {
        let mut disk = Disk::new();
        put_u64(&mut disk.0, 512 + 24, 2);
        disk.crc(1);
        assert_eq!(
            primary_failure(&disk),
            HeaderFailure::MyLba {
                found: 2,
                expected: 1
            }
        );
    }

    #[test]
    fn a_usable_range_that_ends_before_it_starts_fails() {
        let mut disk = Disk::new();
        put_u64(&mut disk.0, 512 + 40, LAST_USABLE + 1);
        disk.crc(1);
        assert_eq!(primary_failure(&disk), HeaderFailure::UsableRange);
    }

    #[test]
    fn an_entry_size_that_is_not_128_times_a_power_of_two_up_to_512_fails() {
        for size in [0u32, 64, 127, 192, 1024] {
            let mut disk = Disk::new();
            put_u32(&mut disk.0, 512 + 84, size);
            disk.crc(1);
            assert_eq!(primary_failure(&disk), HeaderFailure::EntrySize { size });
        }
    }

    #[test]
    fn entry_sizes_of_256_and_512_are_accepted() {
        for size in [256u32, 512] {
            let mut disk = Disk::new();
            // 16 entries of the larger size fill the same array bytes.
            let count = (COUNT as u32 * 128) / size;
            for lba in [1, BACKUP_LBA] {
                let base = disk.at(lba);
                put_u32(&mut disk.0, base + 80, count);
                put_u32(&mut disk.0, base + 84, size);
            }
            // Re-lay the arrays: each 128-byte entry now sits at a multiple
            // of the new size. Entry 0 is unchanged; the rest are zero.
            for lba in [1, BACKUP_LBA] {
                let base = disk.at(lba);
                let entries = le_u64(&disk.0[base..], 72);
                let array = disk.at(entries);
                let crc = crc32(&disk.0[array..array + (count * size) as usize]);
                put_u32(&mut disk.0, base + 88, crc);
                disk.crc(lba);
            }
            assert_eq!(disk.table().used, HeaderUsed::Primary, "size {size}");
        }
    }

    #[test]
    fn an_array_larger_than_the_bound_fails_and_the_bound_can_be_made_small() {
        let disk = Disk::new();
        let mut image = disk.image();
        // 128 entries of 128 bytes is 16384 bytes: refused under 16383.
        let both = read(&mut image, SECTORS, 16_383).expect_err("the bound is smaller");
        assert!(matches!(
            both,
            NotAnalysed::NoValidHeader {
                primary: HeaderFailure::ArrayBound { bytes: 16384 },
                backup: HeaderFailure::ArrayBound { bytes: 16384 }
            }
        ));
        read(&mut image, SECTORS, 16_384).expect("exactly the bound is accepted");
    }

    #[test]
    fn the_bound_cannot_be_raised_above_the_projects() {
        let mut disk = Disk::new();
        // An array of 2 MiB: 16384 entries of 128 bytes.
        for lba in [1, BACKUP_LBA] {
            let base = disk.at(lba);
            put_u32(&mut disk.0, base + 80, 16_384);
            disk.crc(lba);
        }
        let err = read(&mut disk.image(), SECTORS, u64::MAX).expect_err("over the bound");
        assert!(matches!(
            err,
            NotAnalysed::NoValidHeader {
                primary: HeaderFailure::ArrayBound { .. },
                ..
            }
        ));
    }

    #[test]
    fn the_primarys_array_must_end_before_the_usable_range() {
        let mut disk = Disk::new();
        // The array occupies LBA 2 to 33; a first usable LBA of 33 overlaps
        // it. Only the primary is changed, so the backup is the one used.
        put_u64(&mut disk.0, 512 + 40, 33);
        disk.crc(1);
        assert_eq!(primary_failure(&disk), HeaderFailure::ArrayPlace);
    }

    #[test]
    fn the_primarys_array_must_follow_its_header() {
        let mut disk = Disk::new();
        put_u64(&mut disk.0, 512 + 72, 1);
        disk.crc(1);
        assert_eq!(primary_failure(&disk), HeaderFailure::ArrayPlace);
    }

    #[test]
    fn an_array_that_runs_past_the_image_fails() {
        let mut disk = Disk::new();
        put_u64(&mut disk.0, 512 + 72, SECTORS - 10);
        disk.crc(1);
        assert_eq!(primary_failure(&disk), HeaderFailure::ArrayPlace);
    }

    #[test]
    fn the_backups_array_must_follow_the_usable_range_and_end_before_its_header() {
        let mut disk = Disk::new();
        // Break the primary so the backup is the one read, then misplace its array.
        disk.0[512] = b'X';
        let base = disk.at(BACKUP_LBA);
        put_u64(&mut disk.0, base + 72, 2);
        disk.crc(BACKUP_LBA);
        assert!(matches!(
            disk.read(),
            Err(NotAnalysed::NoValidHeader {
                backup: HeaderFailure::ArrayPlace,
                ..
            })
        ));
    }

    #[test]
    fn an_array_crc_mismatch_fails() {
        let mut disk = Disk::new();
        let at = disk.at(2) + 56;
        disk.0[at] ^= 0xFF;
        assert_eq!(primary_failure(&disk), HeaderFailure::ArrayCrc);
    }

    #[test]
    fn a_revision_and_reserved_bytes_are_reported_not_refused() {
        let mut disk = Disk::new();
        for lba in [1, BACKUP_LBA] {
            let base = disk.at(lba);
            put_u32(&mut disk.0, base + 8, 0x0001_0001);
            disk.0[base + 20] = 1;
            disk.0[base + 200] = 1;
            disk.crc(lba);
        }
        let table = disk.table();
        assert_eq!(table.used, HeaderUsed::Primary);
        assert_eq!(
            table.anomalies,
            [
                Anomaly::GptRevision {
                    revision: 0x0001_0001
                },
                Anomaly::GptReservedNonZero
            ]
        );
    }

    #[test]
    fn bytes_after_header_size_are_reserved_too() {
        let mut disk = Disk::new();
        disk.0[512 + 200] = 1;
        assert_eq!(disk.table().anomalies, [Anomaly::GptReservedNonZero]);
    }

    #[test]
    fn a_usable_range_past_the_image_is_reported_not_refused() {
        let mut disk = Disk::new();
        let base = disk.at(1);
        put_u64(&mut disk.0, base + 48, SECTORS + 5);
        disk.crc(1);
        let table = disk.table();
        assert_eq!(table.used, HeaderUsed::Primary);
        assert!(table.anomalies.contains(&Anomaly::GptUsableBeyondEnd {
            last_usable: SECTORS + 5,
            image_sectors: SECTORS
        }));
    }

    // Decision D.

    #[test]
    fn an_invalid_primary_gives_the_backup_and_the_reason() {
        let mut disk = Disk::new();
        disk.0[512 + 56] ^= 0xFF;
        let table = disk.table();
        assert_eq!(
            table.used,
            HeaderUsed::Backup {
                primary: HeaderFailure::HeaderCrc
            }
        );
        assert_eq!(table.entries.len(), 1);
    }

    #[test]
    fn both_headers_invalid_is_no_gpt_with_each_reason() {
        let mut disk = Disk::new();
        disk.0[512 + 56] ^= 0xFF;
        let backup = disk.at(BACKUP_LBA);
        disk.0[backup] = b'X';
        assert_eq!(
            disk.read(),
            Err(NotAnalysed::NoValidHeader {
                primary: HeaderFailure::HeaderCrc,
                backup: HeaderFailure::Signature
            })
        );
    }

    #[test]
    fn an_image_too_short_for_a_backup_has_no_gpt_when_the_primary_fails() {
        let mut image = MemoryImage::new(2 * SECTOR_SIZE as u64);
        image.write(0, &[0u8; 1024]);
        assert_eq!(
            read(&mut image, 2, ARRAY_BOUND),
            Err(NotAnalysed::NoValidHeader {
                primary: HeaderFailure::Signature,
                backup: HeaderFailure::OutsideImage
            })
        );
    }

    #[test]
    fn a_valid_primary_with_a_valid_agreeing_backup_reports_nothing() {
        assert_eq!(Disk::new().table().anomalies, []);
    }

    #[test]
    fn an_invalid_backup_is_an_anomaly_and_the_primary_is_used() {
        let mut disk = Disk::new();
        let backup = disk.at(BACKUP_LBA);
        disk.0[backup + 56] ^= 0xFF;
        let table = disk.table();
        assert_eq!(table.used, HeaderUsed::Primary);
        assert_eq!(
            table.anomalies,
            [Anomaly::GptBackupInvalid {
                lba: BACKUP_LBA,
                reason: HeaderFailure::HeaderCrc
            }]
        );
    }

    #[test]
    fn a_backup_that_disagrees_is_an_anomaly_per_field() {
        let mut disk = Disk::new();
        let backup = disk.at(BACKUP_LBA);
        disk.0[backup + 56] ^= 0xFF;
        disk.crc(BACKUP_LBA);
        assert_eq!(
            disk.table().anomalies,
            [Anomaly::GptHeadersDisagree { field: "DiskGUID" }]
        );
    }

    #[test]
    fn each_field_the_backup_is_compared_on_is_reported() {
        let edits: [(&str, usize, u64, usize); 5] = [
            ("MyLBA and AlternateLBA", 32, 5, 8),
            ("usable range", 40, FIRST_USABLE + 1, 8),
            ("entry count", 80, 127, 4),
            ("entry size", 84, 128, 4),
            ("entry array CRC", 88, 0, 4),
        ];
        // Entry size and the others are changed on the backup only; each is
        // changed to a value the backup's own checks still pass, except where
        // that cannot be done, which the assertion below states.
        let mut seen = Vec::new();
        for (field, at, value, width) in edits {
            let mut disk = Disk::new();
            let base = disk.at(BACKUP_LBA);
            match (field, width) {
                ("entry size", _) => {
                    // 128 is the current value; a backup of 256-byte entries
                    // over 64 entries has the same array bytes.
                    put_u32(&mut disk.0, base + 80, 64);
                    put_u32(&mut disk.0, base + 84, 256);
                    let array = disk.at(BACKUP_ARRAY_LBA);
                    let crc = crc32(&disk.0[array..array + 64 * 256]);
                    put_u32(&mut disk.0, base + 88, crc);
                }
                (_, 8) => put_u64(&mut disk.0, base + at, value),
                _ => put_u32(&mut disk.0, base + at, value as u32),
            }
            disk.crc(BACKUP_LBA);
            let anomalies = disk.table().anomalies;
            // A backup whose own checks fail is "invalid", not "disagrees".
            seen.push((field, anomalies));
        }
        // The usable range is the one change that keeps every backup check
        // passing and moves only that field.
        let usable = &seen[1].1;
        assert_eq!(
            usable,
            &[Anomaly::GptHeadersDisagree {
                field: "usable range"
            }]
        );
        // An entry count of 127 changes the array's bytes, so the backup is
        // invalid by its array CRC, which is reported as an invalid backup.
        assert!(matches!(
            seen[2].1.as_slice(),
            [Anomaly::GptBackupInvalid {
                reason: HeaderFailure::ArrayCrc,
                ..
            }]
        ));
        // 64 x 256 over the same bytes: count and size both differ.
        assert_eq!(
            seen[3].1,
            [
                Anomaly::GptHeadersDisagree {
                    field: "entry count"
                },
                Anomaly::GptHeadersDisagree {
                    field: "entry size"
                }
            ]
        );
        // An array CRC of zero does not match the array: invalid.
        assert!(matches!(
            seen[4].1.as_slice(),
            [Anomaly::GptBackupInvalid {
                reason: HeaderFailure::ArrayCrc,
                ..
            }]
        ));
        // MyLBA unchanged, AlternateLBA of the backup changed to 5: they no
        // longer name each other.
        assert_eq!(
            seen[0].1,
            [Anomaly::GptHeadersDisagree {
                field: "MyLBA and AlternateLBA"
            }]
        );
    }

    #[test]
    fn the_array_crc_field_alone_differing_is_reported_by_the_comparison() {
        let primary = read_header(
            &mut Disk::new().image(),
            1,
            Role::Primary,
            SECTORS,
            ARRAY_BOUND,
        )
        .expect("valid")
        .0;
        let mut backup = primary.clone();
        backup.array_crc ^= 1;
        let mut anomalies = Vec::new();
        compare(&primary, &backup, true, &mut anomalies);
        assert_eq!(
            anomalies,
            [
                Anomaly::GptHeadersDisagree {
                    field: "MyLBA and AlternateLBA"
                },
                Anomaly::GptHeadersDisagree {
                    field: "entry array CRC"
                }
            ]
        );
        // The same two headers, the backup not being the one the primary
        // names: MyLBA and AlternateLBA are not compared.
        anomalies.clear();
        compare(&primary, &backup, false, &mut anomalies);
        assert_eq!(
            anomalies,
            [Anomaly::GptHeadersDisagree {
                field: "entry array CRC"
            }]
        );
    }

    #[test]
    fn a_backup_missing_from_a_truncated_image_is_reported() {
        let disk = Disk::new();
        // Cut the image so the last 1000 sectors, the backup's place, are gone.
        let mut image = MemoryImage::new(1000 * SECTOR_SIZE as u64);
        image.write(0, &disk.0[..1000 * SECTOR_SIZE]);
        let table = read(&mut image, 1000, ARRAY_BOUND).expect("the primary is intact");
        assert_eq!(table.used, HeaderUsed::Primary);
        assert!(table.anomalies.contains(&Anomaly::GptBackupMissing {
            alternate_lba: BACKUP_LBA
        }));
        assert!(table.anomalies.contains(&Anomaly::GptBackupNotAtLastLba {
            alternate_lba: BACKUP_LBA,
            last_lba: 999
        }));
    }

    #[test]
    fn a_backup_not_at_the_last_lba_is_reported_when_the_image_grew() {
        let disk = Disk::new();
        let mut image = MemoryImage::new((SECTORS + 100) * SECTOR_SIZE as u64);
        image.write(0, &disk.0);
        let table = read(&mut image, SECTORS + 100, ARRAY_BOUND).expect("valid primary");
        assert_eq!(
            table.anomalies,
            [Anomaly::GptBackupNotAtLastLba {
                alternate_lba: BACKUP_LBA,
                last_lba: SECTORS + 99
            }]
        );
    }

    #[test]
    fn a_header_at_the_last_lba_that_the_primary_does_not_name_is_compared() {
        let disk = Disk::new();
        // The image grew, and a copy of the backup header, resealed for its
        // new place, sits at the new last LBA with a different disk GUID.
        let last = SECTORS + 99;
        let mut bytes = disk.0.clone();
        bytes.resize(((SECTORS + 100) as usize) * SECTOR_SIZE, 0);
        let mut moved = Disk(bytes);
        let from = moved.at(BACKUP_LBA);
        let to = moved.at(last);
        let copy = moved.0[from..from + SECTOR_SIZE].to_vec();
        moved.0[to..to + SECTOR_SIZE].copy_from_slice(&copy);
        put_u64(&mut moved.0, to + 24, last);
        moved.0[to + 56] ^= 0xFF;
        moved.crc(last);
        let mut image = MemoryImage::new(moved.0.len() as u64);
        image.write(0, &moved.0);
        let table = read(&mut image, SECTORS + 100, ARRAY_BOUND).expect("valid primary");
        assert!(
            table
                .anomalies
                .contains(&Anomaly::GptHeadersDisagree { field: "DiskGUID" })
        );
        // The primary names its backup at LBA 2047, not at this header, so
        // its MyLBA differs from the primary's AlternateLBA by construction:
        // not reported, the NotAtLastLba finding says it.
        assert!(!table.anomalies.contains(&Anomaly::GptHeadersDisagree {
            field: "MyLBA and AlternateLBA"
        }));
    }

    #[test]
    fn a_backup_header_whose_alternate_is_not_the_primary_disagrees() {
        let mut disk = Disk::new();
        let backup = disk.at(BACKUP_LBA);
        put_u64(&mut disk.0, backup + 32, 5);
        disk.crc(BACKUP_LBA);
        assert_eq!(
            disk.table().anomalies,
            [Anomaly::GptHeadersDisagree {
                field: "MyLBA and AlternateLBA"
            }]
        );
    }

    #[test]
    fn the_usable_range_is_compared_at_both_ends() {
        for (at, value) in [(40, FIRST_USABLE + 1), (48, LAST_USABLE - 1)] {
            let mut disk = Disk::new();
            let backup = disk.at(BACKUP_LBA);
            put_u64(&mut disk.0, backup + at, value);
            disk.crc(BACKUP_LBA);
            assert_eq!(
                disk.table().anomalies,
                [Anomaly::GptHeadersDisagree {
                    field: "usable range"
                }],
                "offset {at}"
            );
        }
    }

    #[test]
    fn an_alternate_lba_that_cannot_hold_a_header_means_no_backup() {
        for alternate in [0u64, 1] {
            let mut disk = Disk::new();
            put_u64(&mut disk.0, 512 + 32, alternate);
            disk.crc(1);
            let anomalies = disk.table().anomalies;
            assert!(
                anomalies.contains(&Anomaly::GptBackupMissing {
                    alternate_lba: alternate
                }),
                "alternate {alternate}: {anomalies:?}"
            );
        }
    }

    #[test]
    fn a_usable_range_ending_at_the_image_size_is_beyond_it_and_one_less_is_not() {
        for (last, beyond) in [(SECTORS, true), (SECTORS - 1, false)] {
            let mut disk = Disk::new();
            put_u64(&mut disk.0, 512 + 48, last);
            disk.crc(1);
            let found = disk
                .table()
                .anomalies
                .iter()
                .any(|a| matches!(a, Anomaly::GptUsableBeyondEnd { .. }));
            assert_eq!(found, beyond, "last usable {last}");
        }
    }

    #[test]
    fn a_primary_array_past_the_image_fails_even_where_the_usable_range_allows_it() {
        // A truncated image: the usable range runs past the image, so the
        // array's place before it does not by itself keep it in the image.
        let mut disk = Disk::new();
        put_u64(&mut disk.0, 512 + 48, SECTORS * 10);
        put_u64(&mut disk.0, 512 + 40, SECTORS * 5);
        put_u64(&mut disk.0, 512 + 72, SECTORS - 10);
        disk.crc(1);
        assert_eq!(primary_failure(&disk), HeaderFailure::ArrayPlace);
    }

    #[test]
    fn a_backup_array_must_end_before_its_header_even_inside_a_larger_image() {
        // The image grew past the backup header, so an array that ends after
        // the header still lies inside the image.
        let mut disk = Disk::new();
        let mut bytes = disk.0.clone();
        bytes.resize(((SECTORS + 100) as usize) * SECTOR_SIZE, 0);
        disk = Disk(bytes);
        let base = disk.at(BACKUP_LBA);
        put_u64(&mut disk.0, base + 72, BACKUP_LBA - 5);
        put_u64(&mut disk.0, base + 48, BACKUP_LBA - 100);
        disk.crc(BACKUP_LBA);
        let mut image = disk.image();
        assert_eq!(
            read_header(
                &mut image,
                BACKUP_LBA,
                Role::Backup,
                SECTORS + 100,
                ARRAY_BOUND
            )
            .map(|_| ()),
            Err(HeaderFailure::ArrayPlace)
        );
    }

    #[test]
    fn nothing_is_written_to_the_image_when_a_header_is_repaired_in_the_report() {
        // The backup is used and the primary is left as it was: the reader
        // takes `&mut impl EvidenceReader`, which has no write method, and
        // the buffer the test holds is unchanged.
        let mut disk = Disk::new();
        disk.0[512 + 56] ^= 0xFF;
        let before = disk.0.clone();
        let _ = disk.table();
        assert_eq!(disk.0, before);
    }

    // Decision E.

    #[test]
    fn an_unused_entry_with_a_stray_byte_is_reported() {
        let mut disk = Disk::new();
        let at = disk.at(2) + 128 + 70;
        disk.0[at] = 1;
        let array = disk.at(2);
        let crc = crc32(&disk.0[array..array + COUNT * 128]);
        let base = disk.at(1);
        put_u32(&mut disk.0, base + 88, crc);
        disk.crc(1);
        // The backup's array still holds the old bytes, so its CRC no longer
        // equals the primary's; the finding of interest is the entry's.
        let table = disk.table();
        assert!(
            table
                .anomalies
                .contains(&Anomaly::GptUnusedEntryNotZero { number: 2 })
        );
        assert_eq!(table.entries.len(), 1);
    }

    #[test]
    fn an_entry_that_starts_after_it_ends_is_left_out() {
        let mut disk = Disk::new();
        disk.entry(0, BASIC_DATA, 500, 400);
        disk.reseal();
        let table = disk.table();
        assert!(table.entries.is_empty());
        assert_eq!(
            table.anomalies,
            [Anomaly::GptEntryStartsAfterEnd {
                number: 1,
                start: 500,
                end: 400
            }]
        );
    }

    #[test]
    fn an_entry_that_ends_one_sector_past_the_image_is_left_out() {
        let mut disk = Disk::new();
        disk.entry(0, BASIC_DATA, 100, SECTORS);
        disk.reseal();
        let table = disk.table();
        assert!(table.entries.is_empty());
        assert!(matches!(
            table.anomalies.as_slice(),
            [Anomaly::GptEntryBeyondEnd { number: 1, .. }, ..]
        ));
    }

    #[test]
    fn an_entry_ending_at_the_last_sector_of_the_image_is_kept() {
        let mut disk = Disk::new();
        disk.entry(0, BASIC_DATA, 100, SECTORS - 1);
        disk.reseal();
        let table = disk.table();
        assert_eq!(table.entries.len(), 1);
        assert!(
            table
                .anomalies
                .iter()
                .any(|a| matches!(a, Anomaly::GptEntryOutsideUsable { number: 1, .. }))
        );
    }

    #[test]
    fn an_entry_outside_the_usable_range_but_inside_the_image_is_kept_and_reported() {
        let mut disk = Disk::new();
        disk.entry(0, BASIC_DATA, 20, 199);
        disk.reseal();
        let table = disk.table();
        assert_eq!(table.entries.len(), 1);
        assert_eq!(
            table.anomalies,
            [Anomaly::GptEntryOutsideUsable {
                number: 1,
                start: 20,
                end: 199,
                first_usable: FIRST_USABLE,
                last_usable: LAST_USABLE
            }]
        );
    }

    #[test]
    fn an_entry_exactly_filling_the_usable_range_is_not_reported() {
        let mut disk = Disk::new();
        disk.entry(0, BASIC_DATA, FIRST_USABLE, LAST_USABLE);
        disk.reseal();
        let table = disk.table();
        assert_eq!(table.entries.len(), 1);
        assert_eq!(table.anomalies, []);
    }

    #[test]
    fn an_entry_beyond_32_bits_of_sectors_is_left_out_by_the_analysis_limit() {
        // Needs an image of 2 TiB; the rule is tested through the entry
        // reader with the image size the entry fits.
        let mut disk = Disk::new();
        disk.entry(0, BASIC_DATA, 100, 199);
        disk.reseal();
        let table = disk.table();
        let header = Header {
            revision: REVISION,
            reserved_non_zero: false,
            my_lba: 1,
            alternate_lba: 2,
            first_usable: 0,
            last_usable: u64::MAX,
            disk_guid: [0; 16],
            entries_lba: 2,
            entry_count: 1,
            entry_size: 128,
            array_crc: 0,
        };
        let mut raw = vec![0u8; 128];
        raw[..16].copy_from_slice(&BASIC_DATA);
        put_u64(&mut raw, 32, u64::from(u32::MAX) + 1);
        put_u64(&mut raw, 40, u64::from(u32::MAX) + 10);
        let mut anomalies = Vec::new();
        let kept = read_entries(&header, &raw, u64::MAX, &mut anomalies);
        assert!(kept.is_empty());
        assert_eq!(
            anomalies,
            [Anomaly::GptEntryTooLarge {
                number: 1,
                start: u64::from(u32::MAX) + 1,
                end: u64::from(u32::MAX) + 10
            }]
        );
        // The same entry shifted under 2^32 is kept.
        put_u64(&mut raw, 32, 5);
        put_u64(&mut raw, 40, 9);
        anomalies.clear();
        assert_eq!(
            read_entries(&header, &raw, u64::MAX, &mut anomalies).len(),
            1
        );
        // An entry that starts low but is longer than 2^32 - 1 sectors is
        // left out as well.
        put_u64(&mut raw, 32, 5);
        put_u64(&mut raw, 40, u64::from(u32::MAX) + 10);
        anomalies.clear();
        assert!(read_entries(&header, &raw, u64::MAX, &mut anomalies).is_empty());
        assert!(matches!(
            anomalies.as_slice(),
            [Anomaly::GptEntryTooLarge { number: 1, .. }]
        ));
        let _ = table;
    }

    #[test]
    fn entries_out_of_start_order_are_compared_in_start_order() {
        let mut disk = Disk::new();
        disk.entry(0, BASIC_DATA, 500, 600);
        disk.entry(1, BASIC_DATA, 100, 200);
        disk.entry(2, BASIC_DATA, 150, 250);
        disk.reseal();
        assert_eq!(
            disk.table().anomalies,
            [Anomaly::GptOverlappingEntries {
                first: 2,
                second: 3
            }]
        );
    }

    #[test]
    fn an_overlap_names_the_lower_numbered_entry_first_whatever_the_start_order() {
        let mut disk = Disk::new();
        disk.entry(0, BASIC_DATA, 150, 250);
        disk.entry(1, BASIC_DATA, 100, 200);
        disk.reseal();
        assert_eq!(
            disk.table().anomalies,
            [Anomaly::GptOverlappingEntries {
                first: 1,
                second: 2
            }]
        );
    }

    #[test]
    fn overlapping_entries_are_reported_once_each_and_adjacent_ones_are_not() {
        let mut disk = Disk::new();
        disk.entry(1, BASIC_DATA, 150, 250);
        disk.entry(2, BASIC_DATA, 251, 300);
        disk.reseal();
        let table = disk.table();
        assert_eq!(table.entries.len(), 3);
        assert_eq!(
            table.anomalies,
            [Anomaly::GptOverlappingEntries {
                first: 1,
                second: 2
            }]
        );
    }

    #[test]
    fn entries_sharing_one_sector_overlap() {
        let mut disk = Disk::new();
        disk.entry(1, BASIC_DATA, 199, 250);
        disk.reseal();
        assert_eq!(
            disk.table().anomalies,
            [Anomaly::GptOverlappingEntries {
                first: 1,
                second: 2
            }]
        );
    }

    #[test]
    fn an_entry_inside_another_is_compared_with_the_one_reaching_furthest() {
        let mut disk = Disk::new();
        disk.entry(0, BASIC_DATA, 100, 900);
        disk.entry(1, BASIC_DATA, 200, 300);
        disk.entry(2, BASIC_DATA, 400, 500);
        disk.reseal();
        assert_eq!(
            disk.table().anomalies,
            [
                Anomaly::GptOverlappingEntries {
                    first: 1,
                    second: 2
                },
                Anomaly::GptOverlappingEntries {
                    first: 1,
                    second: 3
                }
            ]
        );
    }

    #[test]
    fn entries_are_numbered_by_index_plus_one_whatever_their_order() {
        let mut disk = Disk::new();
        disk.entry(0, BASIC_DATA, 500, 600);
        disk.entry(6, EFI_SYSTEM, 100, 199);
        disk.reseal();
        let numbers: Vec<u16> = disk.table().entries.iter().map(|e| e.number).collect();
        assert_eq!(numbers, [1, 7]);
    }

    #[test]
    fn a_name_ends_at_the_first_zero_unit_and_keeps_unpaired_surrogates() {
        let mut disk = Disk::new();
        for array_lba in [2, BACKUP_ARRAY_LBA] {
            let base = disk.at(array_lba) + 56;
            disk.0[base..base + 72].fill(0);
            disk.0[base..base + 2].copy_from_slice(&0xD800u16.to_le_bytes());
            disk.0[base + 2..base + 4].copy_from_slice(&0x41u16.to_le_bytes());
            disk.0[base + 6..base + 8].copy_from_slice(&0x42u16.to_le_bytes());
        }
        disk.reseal();
        assert_eq!(disk.table().entries[0].name, [0xD800, 0x41]);
    }

    // Decision I.

    #[test]
    fn a_header_at_byte_4096_and_none_at_lba_1_is_a_4096_byte_block_gpt() {
        let mut bytes = vec![0u8; 8192];
        bytes[4096..4104].copy_from_slice(&SIGNATURE);
        let mut image = MemoryImage::new(8192);
        image.write(0, &bytes);
        assert_eq!(
            read(&mut image, 16, ARRAY_BOUND),
            Err(NotAnalysed::FourKnBlocks)
        );
    }

    #[test]
    fn no_header_at_either_place_is_two_failures_not_a_4096_byte_block_gpt() {
        let mut image = MemoryImage::new(8192);
        image.write(0, &[0u8; 8192]);
        assert!(matches!(
            read(&mut image, 16, ARRAY_BOUND),
            Err(NotAnalysed::NoValidHeader { .. })
        ));
    }

    #[test]
    fn an_invalid_header_at_lba_1_is_not_taken_for_a_4096_byte_block_gpt() {
        // The signature is present at LBA 1 and at byte 4096: the failure is
        // the header's own, not the block size.
        let mut disk = Disk::new();
        disk.0[512 + 56] ^= 0xFF;
        disk.0[4096..4104].copy_from_slice(&SIGNATURE);
        assert_eq!(primary_failure(&disk), HeaderFailure::HeaderCrc);
    }

    // Decision B.

    #[test]
    fn reserved_bytes_in_the_header_alone_are_reported() {
        let mut disk = Disk::new();
        for lba in [1, BACKUP_LBA] {
            let base = disk.at(lba);
            disk.0[base + 20] = 1;
            disk.crc(lba);
        }
        assert_eq!(disk.table().anomalies, [Anomaly::GptReservedNonZero]);
    }

    #[test]
    fn a_header_at_lba_1_is_found_for_the_stale_check_and_a_damaged_one_is_not() {
        let disk = Disk::new();
        assert_eq!(
            stale_header_finding(&mut disk.image(), SECTORS),
            Some(Anomaly::GptBehindLegacyMbr)
        );
        let mut damaged = Disk::new();
        damaged.0[512 + 56] ^= 0xFF;
        assert_eq!(stale_header_finding(&mut damaged.image(), SECTORS), None);
        // An image with no sector 1 has no header there: silent.
        assert_eq!(stale_header_finding(&mut disk.image(), 1), None);
    }

    /// Fails every read at or after `from`.
    struct FailingFrom<'a> {
        inner: &'a mut MemoryImage,
        from: u64,
    }

    impl EvidenceReader for FailingFrom<'_> {
        fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
            if offset >= self.from {
                return Err(Error::from_io(
                    std::path::Path::new("<test>"),
                    std::io::Error::from(std::io::ErrorKind::Other),
                ));
            }
            self.inner.read_exact_at(offset, buf)
        }
    }

    #[test]
    fn a_sector_1_that_cannot_be_read_is_reported_not_taken_for_no_stale_gpt() {
        let disk = Disk::new();
        let mut inner = disk.image();
        let mut failing = FailingFrom {
            inner: &mut inner,
            from: 512,
        };
        assert!(matches!(
            stale_header_finding(&mut failing, SECTORS),
            Some(Anomaly::GptCheckNotMade { .. })
        ));
    }

    #[test]
    fn an_entry_array_that_cannot_be_read_is_reported_the_same_way() {
        // Sector 1 reads; the array at LBA 2 does not.
        let disk = Disk::new();
        let mut inner = disk.image();
        let mut failing = FailingFrom {
            inner: &mut inner,
            from: 1024,
        };
        assert!(matches!(
            stale_header_finding(&mut failing, SECTORS),
            Some(Anomaly::GptCheckNotMade { .. })
        ));
    }

    #[test]
    fn a_read_error_on_the_primary_is_a_reason_not_a_missing_gpt() {
        // The same reader through `read`: the primary fails as unreadable,
        // and the backup, at an offset the reader also refuses, as well.
        let disk = Disk::new();
        let mut inner = disk.image();
        let mut failing = FailingFrom {
            inner: &mut inner,
            from: 512,
        };
        let Err(NotAnalysed::NoValidHeader { primary, .. }) =
            read(&mut failing, SECTORS, ARRAY_BOUND)
        else {
            panic!("a reader that fails must not yield a table");
        };
        assert!(matches!(primary, HeaderFailure::Unreadable(_)));
    }

    #[test]
    fn display_of_each_failure_is_one_line_without_debug_syntax() {
        let all = [
            HeaderFailure::OutsideImage,
            HeaderFailure::Unreadable("x".into()),
            HeaderFailure::Signature,
            HeaderFailure::HeaderSize { size: 91 },
            HeaderFailure::HeaderCrc,
            HeaderFailure::MyLba {
                found: 2,
                expected: 1,
            },
            HeaderFailure::UsableRange,
            HeaderFailure::EntrySize { size: 64 },
            HeaderFailure::ArrayBound { bytes: 9 },
            HeaderFailure::ArrayPlace,
            HeaderFailure::ArrayCrc,
        ];
        for failure in all {
            let text = failure.to_string();
            assert!(!text.is_empty() && !text.contains('{') && !text.contains('\n'));
        }
        assert_eq!(
            NotAnalysed::NoValidHeader {
                primary: HeaderFailure::Signature,
                backup: HeaderFailure::ArrayCrc
            }
            .to_string(),
            "primary header: signature is not \"EFI PART\"; backup header: entry array CRC does not match"
        );
    }
}
