//! FAT directory entries and FAT32 directory enumeration.
//!
//! The root and every live directory are read along their cluster chain.
//! A deleted directory's chain is zeroed, so it is read from its first
//! cluster alone, and only after that cluster identifies itself. ADR-0016.
//!
//! # What is shared and what is not
//!
//! The 32-byte directory entry structure parsed by [`classify`] is identical
//! on FAT12, FAT16 and FAT32. That part of this module would serve all three.
//!
//! [`enumerate_root`] and its siblings are FAT32-only. On FAT12 and FAT16
//! the root directory is a fixed-size region located immediately after the
//! last FAT and sized by `BPB_RootEntCnt`; there is no cluster chain to
//! walk. FAT32 sets that field to zero and stores the root directory as an
//! ordinary cluster chain.
//!
//! The module is named for FAT rather than for directories generally because
//! chain walking is not shared beyond it.
//!
//! # What this does not do
//!
//! Long-filename entries are located, counted and kept in on-disk order.
//! They are not decoded. ADR-0007 section 5 records why, and section 5.3
//! records why the decision matters more for M6 than for M5.
//!
//! Short names are rendered only when every byte is printable ASCII. FAT
//! stores them in the OEM code page current when the entry was created, and
//! no code page information is recorded on the volume. Guessing one would
//! produce a name that looks authoritative and may be wrong.
//!
//! `DIR_NTRes` is recorded and not acted on. The specification instructs
//! readers to set it to zero on creation and never look at it afterwards,
//! while the tools that produced this project's fixtures write case flags
//! into it. Applying those flags would be an interpretation; recording the
//! byte preserves the evidence for a milestone that decides to.

use std::fmt;

use crate::error::Error;
use crate::evidence::EvidenceReader;
use crate::fat::FIRST_DATA_CLUSTER;
use crate::fat32::{FAT32_ENTRY_BYTES, Fat32BootSector};
use crate::filesystem::{VolumeExtent, le_u16, le_u32};

/// Bytes in one directory entry.
pub const ENTRY_BYTES: usize = 32;

/// Bytes in the 8.3 name field.
pub const NAME_LEN: usize = 11;

/// Bytes in the name field's stem. The remainder is the extension.
const NAME_STEM_LEN: usize = 8;

/// Offset of `DIR_Name`. `LDIR_Ord` occupies the same byte in a long-name
/// entry, which is why the first byte can classify an entry before its
/// attribute is read.
const OFF_NAME: usize = 0x00;
const OFF_ATTR: usize = 0x0B;
const OFF_NT_RES: usize = 0x0C;
const OFF_LDIR_CHKSUM: usize = 0x0D;
const OFF_FST_CLUS_HI: usize = 0x14;
const OFF_FST_CLUS_LO: usize = 0x1A;
const OFF_FILE_SIZE: usize = 0x1C;

const ATTR_READ_ONLY: u8 = 0x01;
const ATTR_HIDDEN: u8 = 0x02;
const ATTR_SYSTEM: u8 = 0x04;
const ATTR_VOLUME_ID: u8 = 0x08;
const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_ARCHIVE: u8 = 0x20;

/// The attribute combination marking a long-name entry.
///
/// This contains [`ATTR_VOLUME_ID`], so a bitmask test for the volume label
/// matches every long-name entry. The specification requires equality
/// against [`ATTR_LONG_NAME_MASK`], not a bit test.
const ATTR_LONG_NAME: u8 = ATTR_READ_ONLY | ATTR_HIDDEN | ATTR_SYSTEM | ATTR_VOLUME_ID;

/// The mask applied before comparing against [`ATTR_LONG_NAME`].
const ATTR_LONG_NAME_MASK: u8 = ATTR_LONG_NAME | ATTR_DIRECTORY | ATTR_ARCHIVE;

/// First byte marking an entry free with allocated entries possibly
/// following.
const NAME_DELETED: u8 = 0xE5;

/// First byte marking an entry free with no allocated entries following.
const NAME_TERMINATOR: u8 = 0x00;

/// First byte standing in for a literal `0xE5`.
///
/// `0xE5` is a valid lead byte in the Japanese character set, so a name
/// legitimately beginning with it is stored as `0x05` to keep the entry from
/// reading as deleted.
const NAME_ESCAPED_E5: u8 = 0x05;

/// Bit marking the last entry of a long-name set.
const LDIR_ORD_LAST: u8 = 0x40;

/// Mask applied to a FAT32 table entry. The high four bits are reserved.
const FAT_ENTRY_MASK: u32 = 0x0FFF_FFFF;

/// Lowest value marking the end of a cluster chain.
///
/// This is a range, not a single value. Formatting tools write `0x0FFFFFFF`
/// for files and `0x0FFFFFF8` for the root directory, and both appear in the
/// project's own fixtures.
const FAT_END_OF_CHAIN: u32 = 0x0FFF_FFF8;

/// Value marking a cluster as unusable.
const FAT_BAD_CLUSTER: u32 = 0x0FFF_FFF7;

/// Maximum directory entries.
///
/// The FAT32 specification requires that a directory not exceed
/// 65,536 * 32 bytes. ADR-0007 section 4 adopts it as a hard bound and
/// section 4.1 requires that exceeding it be an error rather than a
/// truncated result.
pub const MAX_ENTRIES: usize = 65_536;

/// What a 32-byte directory entry contains.
///
/// Classification is total: every possible 32 bytes is one of these. There
/// is no error case, so [`classify`] cannot fail.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum EntryKind {
    /// Free, and the specification states no allocated entries follow.
    Terminator,

    /// Free, and previously used.
    ///
    /// The first byte is destroyed. What the entry was is read from the
    /// bytes deletion left alone. ADR-0009 section 3.
    Deleted {
        /// What the surviving bytes establish the entry to have been.
        was: DeletedKind,
    },

    /// One component of a long-name set.
    ///
    /// Retained in on-disk order and counted, not decoded. The set precedes
    /// the short entry it names.
    LongName {
        /// Position within the set, counting from one.
        ordinal: u8,
        /// Whether this is the last component, which appears first on disk.
        last: bool,
        /// Checksum of the associated short name.
        checksum: u8,
    },

    /// The volume label.
    ///
    /// Valid only in the root directory, and the only entry for which the
    /// volume-id attribute stands alone.
    VolumeLabel {
        /// Rendered name, if every byte is printable ASCII.
        name: Option<String>,
        /// The name field as stored.
        raw_name: [u8; NAME_LEN],
    },

    /// A file or subdirectory.
    ShortName {
        /// Rendered `STEM.EXT`, if every byte is printable ASCII.
        name: Option<String>,
        /// The name field as stored, before rendering.
        raw_name: [u8; NAME_LEN],
        /// Whether the directory attribute is set.
        directory: bool,
        /// First cluster, assembled from the high and low words.
        first_cluster: u32,
        /// Size in bytes. Always zero for a directory.
        file_size: u32,
        /// `DIR_NTRes`, recorded and not interpreted.
        nt_res: u8,
    },

    /// Both the directory and volume-id attribute bits are set.
    ///
    /// The specification's own classification names this combination
    /// invalid rather than assigning it a meaning.
    Invalid {
        /// The attribute byte as stored.
        attr: u8,
    },
}

/// What a deleted entry was, as far as its surviving bytes establish.
///
/// Corresponds one to one with the live classification [`classify`]
/// performs, minus every field the deletion marker destroys. Those fields
/// are absent rather than computed: an absent field prompts a question and a
/// wrong one does not.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DeletedKind {
    /// One component of a long-name set.
    ///
    /// The ordinal and the last-entry flag are not carried. Both live in the
    /// destroyed first byte, and `0xE5` has bit 6 set, so every deleted
    /// component reads as ordinal 165 and as the last of its set whatever it
    /// was. EXP-0003 section 4.
    ///
    /// Order is recoverable only from position relative to the short entry
    /// that follows the set.
    LongName {
        /// Checksum of the short name the set belongs to, undamaged.
        checksum: u8,
    },

    /// The volume label.
    VolumeLabel {
        /// Bytes 1 to 10 of the name field. Byte 0 is destroyed.
        surviving_name: [u8; NAME_LEN - 1],
    },

    /// A file or subdirectory.
    ///
    /// No rendered name, because the first character is not yet known.
    /// Recovering it requires the checksum from an associated long-name
    /// entry, which needs a second entry and so cannot happen in
    /// [`classify`]. See [`recover_first_byte`].
    ShortName {
        /// Bytes 1 to 10 of the name field. Byte 0 is destroyed.
        surviving_name: [u8; NAME_LEN - 1],
        /// Whether the directory attribute is set.
        directory: bool,
        /// First cluster, assembled from the high and low words.
        first_cluster: u32,
        /// Size in bytes. Always zero for a directory.
        file_size: u32,
        /// `DIR_NTRes`, recorded and not interpreted.
        nt_res: u8,
    },

    /// Both the directory and volume-id attribute bits are set.
    Invalid {
        /// The attribute byte as stored.
        attr: u8,
    },
}

/// One directory entry and where it was found.
///
/// The cluster is retained so that a later milestone can name an entry's
/// location without walking the chain again.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    /// Cluster the entry was read from.
    pub cluster: u32,
    /// Position within that cluster, counting from zero.
    pub slot: usize,
    /// What the entry contains.
    pub kind: EntryKind,
}

/// Something worth reporting that does not prevent enumeration.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DirectoryObservation {
    /// A cluster holds non-zero bytes after the terminator.
    ///
    /// The specification states that no allocated entries follow a `0x00`
    /// first byte, so a conformant volume has nothing here. Formatters
    /// exist that write the terminator without clearing what follows, and
    /// on real evidence that residue is a record of what the directory
    /// previously held.
    ContentAfterTerminator {
        cluster: u32,
        /// First slot after the terminator holding a non-zero byte.
        first_slot: usize,
        /// How many slots in this cluster hold non-zero bytes.
        slots: usize,
    },

    /// A name field contains bytes outside printable ASCII.
    ///
    /// Expected on volumes written with a non-ASCII OEM code page. The
    /// bytes are reported in hexadecimal rather than rendered, because a
    /// name field is untrusted evidence and rendering it would put
    /// arbitrary bytes on the reader's terminal.
    NameNotPrintableAscii {
        cluster: u32,
        slot: usize,
        raw_name: [u8; NAME_LEN],
    },

    /// An entry sets both the directory and volume-id attribute bits.
    InvalidAttribute { cluster: u32, slot: usize, attr: u8 },
}

impl fmt::Display for DirectoryObservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DirectoryObservation::ContentAfterTerminator {
                cluster,
                first_slot,
                slots,
            } => write!(
                f,
                "cluster {cluster} holds content after the terminator: \
                 {slots} slots from slot {first_slot}"
            ),
            DirectoryObservation::NameNotPrintableAscii {
                cluster,
                slot,
                raw_name,
            } => {
                write!(
                    f,
                    "cluster {cluster} slot {slot} name is not printable ASCII: "
                )?;
                for byte in raw_name {
                    write!(f, "{byte:02x}")?;
                }
                Ok(())
            }
            DirectoryObservation::InvalidAttribute {
                cluster,
                slot,
                attr,
            } => write!(
                f,
                "cluster {cluster} slot {slot} sets both the directory and \
                 volume label attributes: {attr:#04x}"
            ),
        }
    }
}

/// A reason enumeration could not complete.
///
/// This type is deliberately not `PartialEq`: [`DirectoryError::Evidence`]
/// wraps an [`Error`], which holds an [`std::io::Error`]. Tests match on it
/// with `matches!` rather than comparing by value, as the unit tests in
/// `fat32.rs` already do.
#[derive(Debug)]
pub enum DirectoryError {
    /// The directory exceeds the specification's maximum.
    TooLarge { entries: usize },

    /// The chain returns to a cluster it has already visited.
    ///
    /// Enumeration would terminate without this check, because the entry
    /// bound is reached first. Detecting the cycle distinguishes a corrupt
    /// chain from a directory that is merely too large.
    CyclicChain { cluster: u32 },

    /// A chain link names a cluster outside the data region.
    ClusterOutOfRange { cluster: u32, cluster_count: u32 },

    /// A chain link names a cluster marked unusable.
    BadCluster { cluster: u32 },

    /// A chain link names cluster 0 or 1, which are reserved.
    ReservedChainLink { from: u32, next: u32 },

    /// An offset could not be computed without overflow.
    OffsetOverflow { cluster: u32 },

    /// Reading evidence failed.
    Evidence(Error),
}

impl fmt::Display for DirectoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DirectoryError::TooLarge { entries } => write!(
                f,
                "directory exceeds {MAX_ENTRIES} entries, stopped at {entries}"
            ),
            DirectoryError::CyclicChain { cluster } => {
                write!(f, "cluster chain returns to cluster {cluster}")
            }
            DirectoryError::ClusterOutOfRange {
                cluster,
                cluster_count,
            } => write!(
                f,
                "cluster {cluster} is outside the {cluster_count} data clusters"
            ),
            DirectoryError::BadCluster { cluster } => {
                write!(f, "cluster {cluster} is marked unusable")
            }
            DirectoryError::ReservedChainLink { from, next } => {
                write!(f, "cluster {from} chains to reserved cluster {next}")
            }
            DirectoryError::OffsetOverflow { cluster } => {
                write!(f, "offset for cluster {cluster} overflows")
            }
            DirectoryError::Evidence(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DirectoryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DirectoryError::Evidence(e) => Some(e),
            _ => None,
        }
    }
}

impl From<Error> for DirectoryError {
    fn from(e: Error) -> Self {
        DirectoryError::Evidence(e)
    }
}

/// An enumerated directory, the root or any other.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Directory {
    /// Entries in on-disk order, ending at the terminator.
    ///
    /// The terminator itself is not included. Long-name entries are, in the
    /// position they occupy relative to the short entry that follows them.
    pub entries: Vec<Entry>,

    /// Slots holding non-zero bytes after the terminator, classified.
    ///
    /// Separate from `entries` because the distinction is the finding:
    /// `entries` is what the volume claims is present, and this is what is
    /// present anyway. Merging them would present a reconstruction as a
    /// directory listing.
    ///
    /// A slot here may classify as [`EntryKind::Terminator`], which means
    /// its first byte is zero and its remaining bytes are not.
    ///
    /// Bounded only indirectly, by the cluster limit that stops the walk.
    /// The worst case is therefore about twice what `entries` alone permits.
    /// ADR-0009 section 4.5 records why no second limit is imposed.
    pub residue: Vec<Entry>,

    /// Clusters walked, in chain order.
    pub clusters: Vec<u32>,

    /// Findings that did not prevent enumeration.
    pub observations: Vec<DirectoryObservation>,
}

impl Directory {
    /// Short entries: files and subdirectories, but not long-name
    /// components and not the volume label.
    ///
    /// Named for what it counts rather than for files, because a
    /// subdirectory is a short entry too and a reader who assumes otherwise
    /// will be wrong by the number of subdirectories present.
    pub fn short_entry_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::ShortName { .. }))
            .count()
    }

    /// Long-name components retained but not decoded.
    ///
    /// Counts allocated components only. A deleted one classifies as
    /// [`EntryKind::Deleted`] and is counted by [`Directory::deleted_count`].
    pub fn long_name_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::LongName { .. }))
            .count()
    }

    /// Deleted entries in the listing, of any shape.
    ///
    /// Counted separately because the two counts above describe the
    /// allocated listing and a deleted entry is not part of it. Without
    /// this, a directory holding eight deleted entries and three live ones
    /// reports three and appears to have lost the rest.
    ///
    /// Does not count [`Directory::residue`], which is reported on its
    /// own.
    pub fn deleted_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Deleted { .. }))
            .count()
    }
}

/// Classifies one directory entry.
///
/// Performs no I/O and cannot fail. The parameter is a fixed-size array
/// rather than a slice so that the bounds obligation of
/// `crate::filesystem::le_u16` and `crate::filesystem::le_u32`, which
/// index without checking, is discharged by the compiler.
///
/// The first byte decides deletion before the attribute decides anything,
/// because a deleted long-name entry is deleted rather than a long name. The
/// specification's test for a long name requires both the masked attribute
/// to match and the first byte not to be the deleted marker.
///
/// The attribute is then read a second time, by `classify_deleted`, to say
/// what the deleted entry was.
pub fn classify(entry: &[u8; ENTRY_BYTES]) -> EntryKind {
    let first = entry[OFF_NAME];

    if first == NAME_TERMINATOR {
        return EntryKind::Terminator;
    }

    let attr = entry[OFF_ATTR];

    if first == NAME_DELETED {
        return EntryKind::Deleted {
            was: classify_deleted(entry, attr),
        };
    }

    if attr & ATTR_LONG_NAME_MASK == ATTR_LONG_NAME {
        return EntryKind::LongName {
            ordinal: first & !LDIR_ORD_LAST,
            last: first & LDIR_ORD_LAST != 0,
            checksum: entry[OFF_LDIR_CHKSUM],
        };
    }

    let mut raw_name = [0u8; NAME_LEN];
    raw_name.copy_from_slice(&entry[OFF_NAME..OFF_NAME + NAME_LEN]);

    match attr & (ATTR_DIRECTORY | ATTR_VOLUME_ID) {
        ATTR_VOLUME_ID => EntryKind::VolumeLabel {
            name: short_name(&raw_name),
            raw_name,
        },
        combined if combined == ATTR_DIRECTORY | ATTR_VOLUME_ID => EntryKind::Invalid { attr },
        _ => {
            let high = le_u16(entry, OFF_FST_CLUS_HI) as u32;
            let low = le_u16(entry, OFF_FST_CLUS_LO) as u32;

            EntryKind::ShortName {
                name: short_name(&raw_name),
                raw_name,
                directory: attr & ATTR_DIRECTORY != 0,
                first_cluster: (high << 16) | low,
                file_size: le_u32(entry, OFF_FILE_SIZE),
                nt_res: entry[OFF_NT_RES],
            }
        }
    }
}

/// What a deleted entry was, read from the bytes deletion left alone.
///
/// Mirrors the branches of [`classify`] with the destroyed fields omitted.
/// The attribute byte is passed in rather than re-read so that both
/// functions demonstrably test the same byte.
///
/// Performs no I/O and cannot fail, for the reason given on [`classify`].
fn classify_deleted(entry: &[u8; ENTRY_BYTES], attr: u8) -> DeletedKind {
    if attr & ATTR_LONG_NAME_MASK == ATTR_LONG_NAME {
        return DeletedKind::LongName {
            checksum: entry[OFF_LDIR_CHKSUM],
        };
    }

    let mut surviving_name = [0u8; NAME_LEN - 1];
    surviving_name.copy_from_slice(&entry[OFF_NAME + 1..OFF_NAME + NAME_LEN]);

    match attr & (ATTR_DIRECTORY | ATTR_VOLUME_ID) {
        ATTR_VOLUME_ID => DeletedKind::VolumeLabel { surviving_name },
        combined if combined == ATTR_DIRECTORY | ATTR_VOLUME_ID => DeletedKind::Invalid { attr },
        _ => {
            let high = le_u16(entry, OFF_FST_CLUS_HI) as u32;
            let low = le_u16(entry, OFF_FST_CLUS_LO) as u32;

            DeletedKind::ShortName {
                surviving_name,
                directory: attr & ATTR_DIRECTORY != 0,
                first_cluster: (high << 16) | low,
                file_size: le_u32(entry, OFF_FILE_SIZE),
                nt_res: entry[OFF_NT_RES],
            }
        }
    }
}

/// The checksum a long-name entry carries for its short name.
///
/// Computed over all eleven bytes of the short name as stored, so the
/// escaped form of a leading `0xE5` is checksummed as `0x05` and not as the
/// character it stands for.
///
/// Performs no I/O and cannot fail, for the reason given on [`classify`].
pub fn chksum(name: &[u8; NAME_LEN]) -> u8 {
    let mut sum = 0u8;
    for byte in name {
        sum = ((sum & 1) << 7).wrapping_add(sum >> 1).wrapping_add(*byte);
    }
    sum
}

/// Recovers the first byte of a short name that deletion destroyed.
///
/// `surviving` is bytes 1 to 10 of the name field, and `checksum` is the
/// byte a long-name entry of the same set carries. Each round of [`chksum`]
/// is a rotation followed by an addition modulo 256, both of which are
/// bijections, and the first byte enters as the initial accumulator value, so
/// exactly one candidate reproduces any given checksum.
///
/// That argument is not relied upon. All 256 candidates are tried and the
/// result is returned only if exactly one matched, so a checksum function
/// that was not bijective would yield `None` rather than an arbitrary byte.
///
/// Returns `None` when the recovered byte is one a live entry never holds:
/// `0x00` marks the end of the directory and `0xE5` is never stored
/// literally, being escaped to `0x05`. Either outcome is proof that the
/// long-name set does not belong to this short entry, so the association is
/// rejected rather than reported. ADR-0009 section 6.2.
///
/// A returned `0x05` means the recovered character is `0xE5`.
pub fn recover_first_byte(surviving: &[u8; NAME_LEN - 1], checksum: u8) -> Option<u8> {
    let mut name = [0u8; NAME_LEN];
    name[1..].copy_from_slice(surviving);

    let mut found = None;
    for candidate in 0..=u8::MAX {
        name[0] = candidate;
        if chksum(&name) != checksum {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = Some(candidate);
    }

    match found {
        Some(NAME_TERMINATOR) | Some(NAME_DELETED) => None,
        other => other,
    }
}

/// What became of the first byte of a deleted short name.
///
/// Three outcomes, kept apart because they are different findings. A byte
/// derived from a checksum is evidence. A byte that nothing constrains is an
/// absence of evidence. A derivation that produced an impossible byte is
/// evidence that the long-name set belongs to some other entry, which is a
/// positive finding and not an absence.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FirstByte {
    /// Determined by the checksum of a preceding long-name component.
    ///
    /// A value of `0x05` means the character is `0xE5`, escaped as stored.
    Recovered(u8),

    /// No long-name component precedes the entry, so nothing constrains the
    /// destroyed byte and all 254 remaining values fit the evidence equally.
    /// ADR-0009 section 6.4.
    Destroyed,

    /// A component precedes the entry but derives a byte a live entry never
    /// holds, which proves the set names some other short entry.
    /// ADR-0009 section 6.2.
    NotAssociated,
}

/// Recovers the first byte of the deleted short entry at `index`.
///
/// Walks backwards from `index` while each preceding entry is a deleted
/// long-name component carrying the same checksum, then derives the byte
/// from that checksum and the ten surviving name bytes. ADR-0009 §6.3.
///
/// The walk stops at any live entry. A live long-name component belongs to
/// the live short entry that follows it, which a deleted entry is not.
///
/// Position is the only ordering evidence available, because deletion
/// destroys `LDIR_Ord` on every component: `0xE5` has bit 6 set, so each one
/// reads as ordinal 165 and as the last of its set. EXP-0003 §4.
///
/// Returns `None` when the entry at `index` is not a deleted short entry, in
/// which case the question does not arise. A live entry's first byte is not
/// destroyed, and reporting it as such would be false.
pub fn associate(entries: &[Entry], index: usize) -> Option<FirstByte> {
    let EntryKind::Deleted {
        was: DeletedKind::ShortName { surviving_name, .. },
    } = &entries.get(index)?.kind
    else {
        return None;
    };

    let mut checksum = None;
    for candidate in entries[..index].iter().rev() {
        match &candidate.kind {
            EntryKind::Deleted {
                was: DeletedKind::LongName { checksum: c },
            } => match checksum {
                None => checksum = Some(*c),
                Some(seen) if seen == *c => {}
                Some(_) => break,
            },
            _ => break,
        }
    }

    Some(match checksum {
        None => FirstByte::Destroyed,
        Some(c) => match recover_first_byte(surviving_name, c) {
            Some(byte) => FirstByte::Recovered(byte),
            None => FirstByte::NotAssociated,
        },
    })
}

/// Renders a recovered short name as `STEM.EXT`.
///
/// `first` is the byte [`associate`] derived and `surviving` the ten bytes
/// deletion left. Returns `None` on the same terms as any other short name:
/// unless every byte is printable ASCII.
pub fn recovered_name(first: u8, surviving: &[u8; NAME_LEN - 1]) -> Option<String> {
    let mut raw = [0u8; NAME_LEN];
    raw[0] = first;
    raw[1..].copy_from_slice(surviving);

    short_name(&raw)
}

/// Renders an 8.3 name as `STEM.EXT`.
///
/// Returns `None` unless every byte is printable ASCII. Names are stored in
/// the OEM code page current when the entry was created, which the volume
/// does not record, so bytes above `0x7E` cannot be rendered without
/// guessing. A name whose first byte is the escaped form of `0xE5` also
/// returns `None`, because the character it stands for is not ASCII.
///
/// The stem and extension are space-padded separately and are therefore
/// trimmed separately. Trimming the field as a whole would produce
/// `HELLO   TXT` rather than `HELLO.TXT`.
fn short_name(raw: &[u8; NAME_LEN]) -> Option<String> {
    let mut bytes = *raw;
    if bytes[0] == NAME_ESCAPED_E5 {
        bytes[0] = NAME_DELETED;
    }

    if !bytes.iter().all(|b| (0x20..=0x7E).contains(b)) {
        return None;
    }

    let stem = trim_trailing_spaces(&bytes[..NAME_STEM_LEN]);
    let ext = trim_trailing_spaces(&bytes[NAME_STEM_LEN..]);

    if stem.is_empty() {
        return None;
    }

    let mut name = String::with_capacity(NAME_LEN + 1);
    name.push_str(stem);
    if !ext.is_empty() {
        name.push('.');
        name.push_str(ext);
    }
    Some(name)
}

/// Trims trailing `0x20` bytes. The caller has established the slice is
/// printable ASCII, so it is valid UTF-8.
fn trim_trailing_spaces(bytes: &[u8]) -> &str {
    let end = bytes
        .iter()
        .rposition(|b| *b != b' ')
        .map_or(0, |last| last + 1);
    std::str::from_utf8(&bytes[..end]).unwrap_or("")
}

/// Byte offset of a cluster's first sector, relative to the evidence.
pub(crate) fn cluster_offset(
    boot: &Fat32BootSector,
    extent: VolumeExtent,
    cluster: u32,
) -> Result<u64, DirectoryError> {
    let overflow = || DirectoryError::OffsetOverflow { cluster };

    (cluster as u64)
        .checked_sub(FIRST_DATA_CLUSTER as u64)
        .and_then(|n| n.checked_mul(boot.geometry.sectors_per_cluster as u64))
        .and_then(|n| n.checked_add(boot.first_data_sector_absolute(extent)))
        .and_then(|n| n.checked_mul(boot.geometry.bytes_per_sector as u64))
        .ok_or_else(overflow)
}

/// Byte offset of a cluster's entry in the given FAT, relative to the
/// evidence.
///
/// The FAT is large enough to hold an entry for every declared cluster:
/// `parse_boot_sector` refuses a volume where it is not, so this offset
/// cannot fall outside the FAT region for a cluster the caller has already
/// range-checked.
fn fat_entry_offset(
    boot: &Fat32BootSector,
    extent: VolumeExtent,
    fat_index: u8,
    cluster: u32,
) -> Result<u64, DirectoryError> {
    let overflow = || DirectoryError::OffsetOverflow { cluster };

    let fat_start = (fat_index as u64)
        .checked_mul(boot.geometry.fat_size as u64)
        .and_then(|n| n.checked_add(boot.geometry.reserved_sectors as u64))
        .and_then(|n| n.checked_add(extent.start_lba as u64))
        .and_then(|n| n.checked_mul(boot.geometry.bytes_per_sector as u64))
        .ok_or_else(overflow)?;

    (cluster as u64)
        .checked_mul(FAT32_ENTRY_BYTES)
        .and_then(|n| n.checked_add(fat_start))
        .ok_or_else(overflow)
}

/// Enumerates the root directory.
///
/// Walks the cluster chain from `boot.root_cluster`, reading one cluster at
/// a time. Chain links come from the active FAT: FAT 0 when mirroring is
/// enabled, and the FAT named by `BPB_ExtFlags` when it is not. Reading
/// FAT 0 unconditionally would return unmaintained data on a volume where
/// mirroring is disabled.
///
/// Every cluster is scanned in full. The entry listing ends at the first
/// terminator, and non-zero content beyond it is reported as an observation
/// rather than listed or discarded.
pub fn enumerate_root<R: EvidenceReader>(
    reader: &mut R,
    boot: &Fat32BootSector,
    extent: VolumeExtent,
) -> Result<Directory, DirectoryError> {
    enumerate(reader, boot, extent, boot.root_cluster, Chain::Follow).map(|(root, _)| root)
}

/// Enumerates a live directory below the root, from its first cluster.
///
/// ADR-0016 Decision A. A live directory's chain is intact, so it is read
/// exactly as the root is, with the same bounds.
pub fn enumerate_directory<R: EvidenceReader>(
    reader: &mut R,
    boot: &Fat32BootSector,
    extent: VolumeExtent,
    first_cluster: u32,
) -> Result<Directory, DirectoryError> {
    enumerate(reader, boot, extent, first_cluster, Chain::Follow).map(|(dir, _)| dir)
}

/// Why a deleted directory's first cluster was not read.
///
/// ADR-0016 Decision C. Each is a finding about the evidence rather than a
/// failure to read it, so it is reported as a coverage gap with its reason.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeletedDirectoryRefusal {
    /// The FAT marks the cluster in use, so it belongs to something else
    /// now. A cluster reused for a new directory passes the dot checks, and
    /// only this one excludes it.
    InUse {
        /// The FAT entry found for the cluster.
        fat_entry: u32,
    },

    /// Slot 0 is not a `.` entry naming this cluster.
    NoSelfEntry,

    /// Slot 1 is not a `..` entry.
    NoParentEntry,
}

impl fmt::Display for DeletedDirectoryRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeletedDirectoryRefusal::InUse { fat_entry } => {
                write!(f, "its first cluster is in use (FAT entry {fat_entry:#x})")
            }
            DeletedDirectoryRefusal::NoSelfEntry => {
                f.write_str("slot 0 is not a . entry naming its first cluster")
            }
            DeletedDirectoryRefusal::NoParentEntry => f.write_str("slot 1 is not a .. entry"),
        }
    }
}

/// What reading a deleted directory's first cluster produced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DeletedDirectory {
    /// The first cluster passed every check and was read.
    Read {
        /// The entries of the first cluster, and nothing beyond it.
        listing: Directory,

        /// No terminator was found in the first cluster, so the listing may
        /// have continued in a cluster the evidence does not locate.
        /// ADR-0016 Decision D.
        may_continue: bool,
    },

    /// The first cluster was not read.
    Refused(DeletedDirectoryRefusal),
}

/// Reads a deleted directory's first cluster, and nothing else.
///
/// ADR-0016 Decisions A, C and D. The chain is zeroed, so no later cluster
/// can be located, and the adjacent cluster is not read in its place:
/// EXP-0005 finding 6 measured that it returns entries which look live and
/// point beyond the volume. The first cluster is read only if the FAT marks
/// it free and it carries `.` naming itself at slot 0 and `..` at slot 1,
/// which EXP-0005 finding 2 measured.
pub fn enumerate_deleted_directory<R: EvidenceReader>(
    reader: &mut R,
    boot: &Fat32BootSector,
    extent: VolumeExtent,
    first_cluster: u32,
) -> Result<DeletedDirectory, DirectoryError> {
    // Checked before the FAT is consulted, so an out-of-range cluster never
    // becomes an offset into the FAT.
    let last_cluster = boot.geometry.cluster_count as u64 + FIRST_DATA_CLUSTER as u64;
    if (first_cluster as u64) < FIRST_DATA_CLUSTER as u64 || first_cluster as u64 >= last_cluster {
        return Err(DirectoryError::ClusterOutOfRange {
            cluster: first_cluster,
            cluster_count: boot.geometry.cluster_count,
        });
    }

    let fat_index = boot.active_fat().unwrap_or(0);
    let fat_entry = read_fat_entry(reader, boot, extent, fat_index, first_cluster)?;
    if fat_entry != 0 {
        return Ok(DeletedDirectory::Refused(DeletedDirectoryRefusal::InUse {
            fat_entry,
        }));
    }

    let (listing, terminated) =
        enumerate(reader, boot, extent, first_cluster, Chain::FirstClusterOnly)?;

    let mut slots = listing.entries.iter();
    if !slots.next().is_some_and(|e| names_itself(e, first_cluster)) {
        return Ok(DeletedDirectory::Refused(
            DeletedDirectoryRefusal::NoSelfEntry,
        ));
    }
    if !slots.next().is_some_and(names_parent) {
        return Ok(DeletedDirectory::Refused(
            DeletedDirectoryRefusal::NoParentEntry,
        ));
    }

    Ok(DeletedDirectory::Read {
        listing,
        may_continue: !terminated,
    })
}

/// Whether a cluster begins with `.` naming itself and then `..`.
///
/// ADR-0017 Decision A. The orphan search's first test, on slots 0 and 1
/// alone, so a free cluster that is not a directory costs one 64-byte read
/// and is never read whole. It consults no FAT: a cluster that passes is
/// read by [`enumerate_deleted_directory`], which applies every check of
/// ADR-0016 Decision C again, the FAT's included.
pub fn begins_directory<R: EvidenceReader>(
    reader: &mut R,
    boot: &Fat32BootSector,
    extent: VolumeExtent,
    cluster: u32,
) -> Result<bool, DirectoryError> {
    // Checked as `enumerate_deleted_directory` checks it, so the two agree
    // about which clusters exist.
    let last_cluster = boot.geometry.cluster_count as u64 + FIRST_DATA_CLUSTER as u64;
    if (cluster as u64) < FIRST_DATA_CLUSTER as u64 || cluster as u64 >= last_cluster {
        return Err(DirectoryError::ClusterOutOfRange {
            cluster,
            cluster_count: boot.geometry.cluster_count,
        });
    }

    let mut raw = [0u8; 2 * ENTRY_BYTES];
    reader.read_exact_at(cluster_offset(boot, extent, cluster)?, &mut raw)?;
    let [first, second] = raw.as_chunks::<ENTRY_BYTES>().0 else {
        unreachable!("64 bytes are two 32-byte slots");
    };

    let dot = matches!(
        classify(first),
        EntryKind::ShortName { raw_name, directory: true, first_cluster, .. }
            if raw_name == DOT_NAME && first_cluster == cluster
    );
    let dot_dot = matches!(
        classify(second),
        EntryKind::ShortName { raw_name, directory: true, .. } if raw_name == DOT_DOT_NAME
    );

    Ok(dot && dot_dot)
}

/// Raw name of the `.` entry.
const DOT_NAME: [u8; NAME_LEN] = *b".          ";

/// Raw name of the `..` entry.
const DOT_DOT_NAME: [u8; NAME_LEN] = *b"..         ";

/// Whether an entry is a `.` or `..` entry.
///
/// ADR-0016 Decision B: they are listed and never followed. Recognised by
/// raw name and the directory attribute, not by where they point: `..`
/// holding 0 names the root, which EXP-0005 finding 3 measured and the FAT
/// specification requires.
pub fn is_dot_entry(kind: &EntryKind) -> bool {
    matches!(
        kind,
        EntryKind::ShortName { raw_name, directory: true, .. }
            if *raw_name == DOT_NAME || *raw_name == DOT_DOT_NAME
    )
}

/// Whether an entry is slot 0's `.`, naming the cluster it sits in.
fn names_itself(entry: &Entry, cluster: u32) -> bool {
    entry.slot == 0
        && matches!(
            &entry.kind,
            EntryKind::ShortName { raw_name, directory: true, first_cluster, .. }
                if *raw_name == DOT_NAME && *first_cluster == cluster
        )
}

/// Whether an entry is slot 1's `..`.
fn names_parent(entry: &Entry) -> bool {
    entry.slot == 1
        && matches!(
            &entry.kind,
            EntryKind::ShortName { raw_name, directory: true, .. } if *raw_name == DOT_DOT_NAME
        )
}

/// Whether a walk follows the FAT after each cluster.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Chain {
    /// A live directory: follow the chain to its end.
    Follow,

    /// A deleted directory: its chain is zeroed, so stop after the first.
    FirstClusterOnly,
}

/// Enumerates a directory from its first cluster.
///
/// Returns the listing and whether a terminator was found.
fn enumerate<R: EvidenceReader>(
    reader: &mut R,
    boot: &Fat32BootSector,
    extent: VolumeExtent,
    first_cluster: u32,
    chain: Chain,
) -> Result<(Directory, bool), DirectoryError> {
    let cluster_bytes = boot.geometry.cluster_bytes() as usize;
    let slots_per_cluster = cluster_bytes / ENTRY_BYTES;

    // The entry bound stops binding once the terminator is passed, because
    // no further entries are listed while the chain is still being walked.
    // Expressed in clusters it keeps binding: at sixteen slots per cluster
    // this is 4,096, and at the 32 KiB maximum it is 64.
    let max_clusters = MAX_ENTRIES.div_ceil(slots_per_cluster);
    let fat_index = boot.active_fat().unwrap_or(0);
    let last_cluster = boot.geometry.cluster_count as u64 + FIRST_DATA_CLUSTER as u64;

    let mut buffer = vec![0u8; cluster_bytes];
    let mut entries: Vec<Entry> = Vec::new();
    let mut residue: Vec<Entry> = Vec::new();
    let mut clusters: Vec<u32> = Vec::new();
    let mut observations: Vec<DirectoryObservation> = Vec::new();

    let mut cluster = first_cluster;
    let mut terminated = false;

    loop {
        if (cluster as u64) < FIRST_DATA_CLUSTER as u64 || cluster as u64 >= last_cluster {
            return Err(DirectoryError::ClusterOutOfRange {
                cluster,
                cluster_count: boot.geometry.cluster_count,
            });
        }
        if clusters.contains(&cluster) {
            return Err(DirectoryError::CyclicChain { cluster });
        }
        if clusters.len() >= max_clusters {
            return Err(DirectoryError::TooLarge {
                entries: clusters.len() * slots_per_cluster,
            });
        }
        clusters.push(cluster);

        let offset = cluster_offset(boot, extent, cluster)?;
        reader.read_exact_at(offset, &mut buffer)?;

        let mut residue_first: Option<usize> = None;
        let mut residue_slots = 0usize;

        // `as_chunks` yields `&[u8; ENTRY_BYTES]`, so `classify` receives the
        // fixed-size array with no intermediate copy, and the bounds
        // obligation of `le_u16` and `le_u32` is discharged by the type
        // rather than by reconstructing the array here. The remainder is
        // always empty: a cluster is a whole number of 512-byte sectors, so
        // its length is always a multiple of 32.
        for (slot, raw) in buffer.as_chunks::<ENTRY_BYTES>().0.iter().enumerate() {
            if terminated {
                if raw.iter().any(|b| *b != 0) {
                    residue_first.get_or_insert(slot);
                    residue_slots += 1;

                    // No bound is applied here. The cluster limit above
                    // already stops the walk, and a second limit would fail
                    // a directory the first one permits. ADR-0009 §4.5.
                    residue.push(Entry {
                        cluster,
                        slot,
                        kind: classify(raw),
                    });
                }
                continue;
            }

            let kind = classify(raw);

            if matches!(kind, EntryKind::Terminator) {
                terminated = true;
                continue;
            }

            match &kind {
                EntryKind::ShortName {
                    name: None,
                    raw_name,
                    ..
                }
                | EntryKind::VolumeLabel {
                    name: None,
                    raw_name,
                } => observations.push(DirectoryObservation::NameNotPrintableAscii {
                    cluster,
                    slot,
                    raw_name: *raw_name,
                }),
                EntryKind::Invalid { attr } => {
                    observations.push(DirectoryObservation::InvalidAttribute {
                        cluster,
                        slot,
                        attr: *attr,
                    });
                }
                _ => {}
            }

            if entries.len() >= MAX_ENTRIES {
                return Err(DirectoryError::TooLarge {
                    entries: entries.len(),
                });
            }

            entries.push(Entry {
                cluster,
                slot,
                kind,
            });
        }

        if let Some(first_slot) = residue_first {
            observations.push(DirectoryObservation::ContentAfterTerminator {
                cluster,
                first_slot,
                slots: residue_slots,
            });
        }

        if chain == Chain::FirstClusterOnly {
            break;
        }

        let next = read_fat_entry(reader, boot, extent, fat_index, cluster)?;

        if next >= FAT_END_OF_CHAIN {
            break;
        }
        if next == FAT_BAD_CLUSTER {
            return Err(DirectoryError::BadCluster { cluster: next });
        }
        if next < FIRST_DATA_CLUSTER {
            return Err(DirectoryError::ReservedChainLink {
                from: cluster,
                next,
            });
        }

        cluster = next;
    }

    Ok((
        Directory {
            entries,
            residue,
            clusters,
            observations,
        },
        terminated,
    ))
}

/// Reads one FAT entry, masked to its 28 significant bits.
pub(crate) fn read_fat_entry<R: EvidenceReader>(
    reader: &mut R,
    boot: &Fat32BootSector,
    extent: VolumeExtent,
    fat_index: u8,
    cluster: u32,
) -> Result<u32, DirectoryError> {
    let offset = fat_entry_offset(boot, extent, fat_index, cluster)?;
    let mut raw = [0u8; FAT32_ENTRY_BYTES as usize];
    reader.read_exact_at(offset, &mut raw)?;
    Ok(u32::from_le_bytes(raw) & FAT_ENTRY_MASK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::tests::MemoryImage;

    /// An entry with the name field set and everything else zeroed.
    fn entry(name: &[u8; NAME_LEN], attr: u8) -> [u8; ENTRY_BYTES] {
        let mut e = [0u8; ENTRY_BYTES];
        e[OFF_NAME..OFF_NAME + NAME_LEN].copy_from_slice(name);
        e[OFF_ATTR] = attr;
        e
    }

    #[test]
    fn a_zero_first_byte_is_the_terminator() {
        assert_eq!(classify(&[0u8; ENTRY_BYTES]), EntryKind::Terminator);
    }

    #[test]
    fn a_deleted_marker_is_deleted_before_the_attribute_is_read() {
        let mut e = entry(b"HELLO   TXT", ATTR_LONG_NAME);
        e[OFF_NAME] = NAME_DELETED;
        // The checksum a real long-name entry would carry for HELLO.TXT.
        e[OFF_LDIR_CHKSUM] = 0xF1;

        assert_eq!(
            classify(&e),
            EntryKind::Deleted {
                was: DeletedKind::LongName { checksum: 0xF1 }
            },
            "a deleted long-name entry is deleted, not a long name"
        );
    }

    /// Measured against real volumes rather than derived from the
    /// specification alone. `ANNUAL~1.TXT` is the long-named file in
    /// `fat32-root-entries.img`, whose long-name entries `mtools` stamped
    /// with `0xD8`. The other two are the deleted files in
    /// `fat32-deleted-entries.img`, stamped `0xF0` and `0x12`.
    #[test]
    fn the_checksum_matches_what_mtools_wrote() {
        assert_eq!(chksum(b"ANNUAL~1TXT"), 0xD8);
        assert_eq!(chksum(b"PARTIA~1TXT"), 0xF0);
        assert_eq!(chksum(b"COMPLE~1TXT"), 0x12);
    }

    #[test]
    fn every_first_byte_produces_a_distinct_checksum() {
        let mut seen = [false; 256];
        let mut name = *b"?ARTIA~1TXT";

        for candidate in 0..=u8::MAX {
            name[0] = candidate;
            let sum = chksum(&name) as usize;
            assert!(!seen[sum], "checksum {sum:#04x} produced twice");
            seen[sum] = true;
        }
    }

    #[test]
    fn a_destroyed_first_byte_is_recovered_exactly() {
        for name in [b"PARTIA~1TXT", b"COMPLE~1TXT", b"HELLO   TXT"] {
            let sum = chksum(name);
            let mut surviving = [0u8; NAME_LEN - 1];
            surviving.copy_from_slice(&name[1..]);

            assert_eq!(
                recover_first_byte(&surviving, sum),
                Some(name[0]),
                "recovering the first byte of {name:?}"
            );
        }
    }

    /// A recovered `0x00` would mean the entry ended the directory, which a
    /// live entry never did. The checksum belongs to some other short name.
    #[test]
    fn a_recovered_terminator_rejects_the_association() {
        let surviving = *b"ARTIA~1TXT";
        let sum = chksum(b"\x00ARTIA~1TXT");

        assert_eq!(recover_first_byte(&surviving, sum), None);
    }

    /// A recovered `0xE5` would mean the byte was stored literally, which it
    /// never is: a leading `0xE5` is escaped to `0x05`.
    #[test]
    fn a_recovered_delete_marker_rejects_the_association() {
        let surviving = *b"ARTIA~1TXT";
        let sum = chksum(b"\xe5ARTIA~1TXT");

        assert_eq!(recover_first_byte(&surviving, sum), None);
    }

    /// The escape is checksummed as stored. A name whose first character is
    /// `0xE5` is stored with `0x05`, so `0x05` is what the recovery returns
    /// and the caller maps it back.
    #[test]
    fn an_escaped_lead_byte_recovers_as_the_escape() {
        let surviving = *b"ABCDEFGTXT";
        let sum = chksum(b"\x05ABCDEFGTXT");

        assert_eq!(recover_first_byte(&surviving, sum), Some(NAME_ESCAPED_E5));
    }

    /// Builds a deleted entry from a name and an attribute.
    fn deleted(name: &[u8; NAME_LEN], attr: u8) -> Entry {
        let mut e = entry(name, attr);
        e[OFF_NAME] = NAME_DELETED;

        Entry {
            cluster: 2,
            slot: 0,
            kind: classify(&e),
        }
    }

    /// Builds a deleted long-name component carrying `checksum`.
    fn deleted_component(checksum: u8) -> Entry {
        let mut e = entry(b"?          ", ATTR_LONG_NAME);
        e[OFF_NAME] = NAME_DELETED;
        e[OFF_LDIR_CHKSUM] = checksum;

        Entry {
            cluster: 2,
            slot: 0,
            kind: classify(&e),
        }
    }

    #[test]
    fn an_association_recovers_the_byte_the_checksum_determines() {
        let entries = vec![
            deleted_component(chksum(b"HELLO   TXT")),
            deleted(b"HELLO   TXT", ATTR_ARCHIVE),
        ];

        assert_eq!(
            associate(&entries, 1),
            Some(FirstByte::Recovered(b'H')),
            "the component names the entry that follows it"
        );
    }

    #[test]
    fn several_components_of_one_set_are_walked() {
        let sum = chksum(b"HELLO   TXT");
        let entries = vec![
            deleted_component(sum),
            deleted_component(sum),
            deleted_component(sum),
            deleted(b"HELLO   TXT", ATTR_ARCHIVE),
        ];

        assert_eq!(associate(&entries, 3), Some(FirstByte::Recovered(b'H')));
    }

    /// The set was consumed by slot reuse, or never existed. Nothing
    /// constrains the byte. ADR-0009 §6.4.
    #[test]
    fn no_component_leaves_the_byte_destroyed() {
        let entries = vec![
            Entry {
                cluster: 2,
                slot: 0,
                kind: classify(&entry(b"KEEP    TXT", ATTR_ARCHIVE)),
            },
            deleted(b"HELLO   TXT", ATTR_ARCHIVE),
        ];

        assert_eq!(associate(&entries, 1), Some(FirstByte::Destroyed));
    }

    /// A component carrying another entry's checksum derives a byte no live
    /// entry holds, which rejects the association. ADR-0009 §6.2.
    #[test]
    fn a_component_of_another_set_is_rejected() {
        let mut name = *b"HELLO   TXT";
        name[0] = NAME_DELETED;
        let impossible = chksum(&name);

        let entries = vec![
            deleted_component(impossible),
            deleted(b"HELLO   TXT", ATTR_ARCHIVE),
        ];

        assert_eq!(
            associate(&entries, 1),
            Some(FirstByte::NotAssociated),
            "0xE5 is never stored literally, so the set names something else"
        );
    }

    /// A live entry's first byte is not destroyed, so the question does not
    /// arise and no answer is offered.
    #[test]
    fn a_live_entry_has_no_destroyed_first_byte() {
        let entries = vec![Entry {
            cluster: 2,
            slot: 0,
            kind: classify(&entry(b"KEEP    TXT", ATTR_ARCHIVE)),
        }];

        assert_eq!(associate(&entries, 0), None);
    }

    #[test]
    fn a_deleted_component_has_no_short_name_of_its_own() {
        let entries = vec![deleted_component(0x12)];

        assert_eq!(associate(&entries, 0), None);
    }

    #[test]
    fn an_index_past_the_end_yields_nothing() {
        assert_eq!(associate(&[], 0), None);
    }

    #[test]
    fn a_recovered_name_is_rendered_with_its_implied_dot() {
        assert_eq!(
            recovered_name(b'H', b"ELLO   TXT"),
            Some("HELLO.TXT".to_string())
        );
    }

    #[test]
    fn a_long_name_entry_is_not_a_volume_label() {
        // The ordinal and first name field of the long-name set in
        // fat32-root-entries.img, at offset 0x1fc460: " 2019" in UTF-16LE.
        let e = entry(
            b"\x42\x20\x00\x32\x00\x30\x00\x31\x00\x39\x00",
            ATTR_LONG_NAME,
        );

        match classify(&e) {
            EntryKind::LongName { ordinal, last, .. } => {
                assert_eq!(ordinal, 2);
                assert!(last, "0x40 marks the last component of the set");
            }
            other => panic!(
                "0x0F contains the volume-id bit; a bitmask test would \
                 misclassify this. Got {other:?}"
            ),
        }
    }

    #[test]
    fn a_volume_label_is_distinguished_from_a_long_name() {
        let e = entry(b"TAPHFIX    ", ATTR_VOLUME_ID);

        match classify(&e) {
            EntryKind::VolumeLabel { name, .. } => {
                assert_eq!(name.as_deref(), Some("TAPHFIX"));
            }
            other => panic!("expected a volume label, got {other:?}"),
        }
    }

    #[test]
    fn both_directory_and_volume_bits_set_is_invalid() {
        let e = entry(b"CONFUSED   ", ATTR_DIRECTORY | ATTR_VOLUME_ID);
        assert_eq!(
            classify(&e),
            EntryKind::Invalid {
                attr: ATTR_DIRECTORY | ATTR_VOLUME_ID
            }
        );
    }

    #[test]
    fn a_short_name_assembles_its_cluster_from_both_words() {
        let mut e = entry(b"HELLO   TXT", ATTR_ARCHIVE);
        e[OFF_FST_CLUS_HI..OFF_FST_CLUS_HI + 2].copy_from_slice(&0x0012u16.to_le_bytes());
        e[OFF_FST_CLUS_LO..OFF_FST_CLUS_LO + 2].copy_from_slice(&0x3456u16.to_le_bytes());
        e[OFF_FILE_SIZE..OFF_FILE_SIZE + 4].copy_from_slice(&24u32.to_le_bytes());

        match classify(&e) {
            EntryKind::ShortName {
                name,
                first_cluster,
                file_size,
                directory,
                ..
            } => {
                assert_eq!(name.as_deref(), Some("HELLO.TXT"));
                assert_eq!(first_cluster, 0x0012_3456);
                assert_eq!(file_size, 24);
                assert!(!directory);
            }
            other => panic!("expected a short name, got {other:?}"),
        }
    }

    #[test]
    fn nt_res_is_recorded_and_not_applied() {
        let mut e = entry(b"README  MD ", ATTR_ARCHIVE);
        e[OFF_NT_RES] = 0x18;

        match classify(&e) {
            EntryKind::ShortName { name, nt_res, .. } => {
                assert_eq!(nt_res, 0x18);
                assert_eq!(
                    name.as_deref(),
                    Some("README.MD"),
                    "the case flags are evidence, not an instruction to rewrite the name"
                );
            }
            other => panic!("expected a short name, got {other:?}"),
        }
    }

    #[test]
    fn stem_and_extension_are_trimmed_separately() {
        assert_eq!(short_name(b"HELLO   TXT").as_deref(), Some("HELLO.TXT"));
        assert_eq!(short_name(b"NOEXT      ").as_deref(), Some("NOEXT"));
        assert_eq!(short_name(b"ANNUAL~1TXT").as_deref(), Some("ANNUAL~1.TXT"));
    }

    #[test]
    fn a_name_outside_printable_ascii_is_not_rendered() {
        assert_eq!(short_name(b"CAF\xe9    TXT"), None);
    }

    #[test]
    fn an_escaped_kanji_lead_byte_is_not_a_deleted_entry() {
        let mut e = entry(b"XABCDEFGTXT", ATTR_ARCHIVE);
        e[OFF_NAME] = NAME_ESCAPED_E5;

        assert!(
            matches!(classify(&e), EntryKind::ShortName { name: None, .. }),
            "0x05 stands for a literal 0xE5 and marks a live entry"
        );
    }

    #[test]
    fn a_deleted_short_entry_keeps_every_field_deletion_left() {
        let mut e = entry(b"HELLO   TXT", ATTR_ARCHIVE);
        e[OFF_NAME] = NAME_DELETED;
        e[OFF_NT_RES] = 0x18;
        e[OFF_FST_CLUS_HI] = 0x01;
        e[OFF_FST_CLUS_LO] = 0x07;
        e[OFF_FILE_SIZE] = 0x2A;

        assert_eq!(
            classify(&e),
            EntryKind::Deleted {
                was: DeletedKind::ShortName {
                    surviving_name: *b"ELLO   TXT",
                    directory: false,
                    first_cluster: 0x0001_0007,
                    file_size: 0x2A,
                    nt_res: 0x18,
                }
            }
        );
    }

    /// The marker is a known constant, so omitting it loses nothing. Carrying
    /// it would invite rendering `0xE5` as the first character of a name.
    #[test]
    fn the_destroyed_byte_is_not_carried_in_the_surviving_name() {
        let mut e = entry(b"HELLO   TXT", ATTR_ARCHIVE);
        e[OFF_NAME] = NAME_DELETED;

        let EntryKind::Deleted {
            was: DeletedKind::ShortName { surviving_name, .. },
        } = classify(&e)
        else {
            panic!("expected a deleted short entry");
        };

        assert_eq!(surviving_name.len(), NAME_LEN - 1);
        assert!(!surviving_name.contains(&NAME_DELETED));
    }

    #[test]
    fn a_deleted_directory_is_distinguished_from_a_deleted_file() {
        let mut e = entry(b"LOGS       ", ATTR_DIRECTORY);
        e[OFF_NAME] = NAME_DELETED;

        let EntryKind::Deleted {
            was: DeletedKind::ShortName { directory, .. },
        } = classify(&e)
        else {
            panic!("expected a deleted short entry");
        };

        assert!(directory);
    }

    #[test]
    fn a_deleted_volume_label_is_not_a_deleted_file() {
        let mut e = entry(b"TAPHFIX    ", ATTR_VOLUME_ID);
        e[OFF_NAME] = NAME_DELETED;

        assert_eq!(
            classify(&e),
            EntryKind::Deleted {
                was: DeletedKind::VolumeLabel {
                    surviving_name: *b"APHFIX    ",
                }
            }
        );
    }

    #[test]
    fn a_deleted_entry_with_both_attribute_bits_is_invalid() {
        let attr = ATTR_DIRECTORY | ATTR_VOLUME_ID;
        let mut e = entry(b"CONFUSED   ", attr);
        e[OFF_NAME] = NAME_DELETED;

        assert_eq!(
            classify(&e),
            EntryKind::Deleted {
                was: DeletedKind::Invalid { attr }
            }
        );
    }

    /// A deleted long-name component reads as ordinal 165 and as the last of
    /// its set, because 0xE5 has bit 6 set. Neither value is carried.
    #[test]
    fn a_deleted_long_name_carries_only_its_checksum() {
        let mut e = entry(b"?          ", ATTR_LONG_NAME);
        e[OFF_NAME] = NAME_DELETED;
        e[OFF_LDIR_CHKSUM] = 0x12;

        assert_eq!(
            classify(&e),
            EntryKind::Deleted {
                was: DeletedKind::LongName { checksum: 0x12 }
            }
        );
        assert_eq!(
            NAME_DELETED & LDIR_ORD_LAST,
            LDIR_ORD_LAST,
            "the marker sets the last-entry flag, which is why it is not carried"
        );
    }

    #[test]
    fn a_directory_entry_reports_itself_as_one() {
        let e = entry(b"LOGS       ", ATTR_DIRECTORY);

        match classify(&e) {
            EntryKind::ShortName { directory, .. } => assert!(directory),
            other => panic!("expected a short name, got {other:?}"),
        }
    }

    // Chain walking.
    //
    // The offsets below were computed by hand from the fixture geometry and
    // then confirmed against fat32-root-multicluster.img, whose FAT begins at
    // 0x104000 and whose cluster 2 begins at 0x1fc400. They are written out
    // rather than obtained from `cluster_offset` and `fat_entry_offset`,
    // because a test that addresses evidence with the same function it is
    // testing agrees with that function whether or not either is right.

    /// Volume geometry, matching `crate::fat::tests::fat32_sector`.
    const CLUSTER_BYTES: usize = 512;
    const START_LBA: u32 = 2048;
    const TOTAL_SECTORS: u32 = 129_024;
    const CLUSTER_COUNT: u32 = 127_006;

    /// Byte offset of FAT 0, FAT 1, and the first data cluster.
    const FAT0_BASE: u64 = 1_064_960;
    const FAT1_BASE: u64 = 1_573_376;
    const CLUSTER2_BASE: u64 = 2_081_792;

    /// Bytes in the whole evidence image.
    const IMAGE_BYTES: u64 = 67_108_864;

    impl MemoryImage {
        /// Writes one FAT entry into the FAT beginning at `fat_base`.
        fn write_fat(&mut self, fat_base: u64, cluster: u32, value: u32) {
            self.write(fat_base + cluster as u64 * 4, &value.to_le_bytes());
        }

        /// Writes a directory cluster, zero-filling the unused slots.
        fn write_cluster(&mut self, cluster: u32, entries: &[[u8; ENTRY_BYTES]]) {
            let mut bytes = vec![0u8; CLUSTER_BYTES];
            for (slot, e) in entries.iter().enumerate() {
                let start = slot * ENTRY_BYTES;
                bytes[start..start + ENTRY_BYTES].copy_from_slice(e);
            }
            self.write(
                CLUSTER2_BASE + (cluster as u64 - 2) * CLUSTER_BYTES as u64,
                &bytes,
            );
        }
    }

    fn extent() -> VolumeExtent {
        VolumeExtent {
            start_lba: START_LBA,
            sector_count: TOTAL_SECTORS,
        }
    }

    /// A parsed boot sector for the synthetic volume.
    ///
    /// `ext_flags` is written verbatim so that the mirroring-disabled path,
    /// which no fixture can reach because `mkfs.vfat` offers no option to set
    /// the field, can be exercised here.
    fn boot(ext_flags: u16) -> Fat32BootSector {
        use crate::fat32::{
            OFF_BACKUP_BOOT_SECTOR, OFF_EXT_FLAGS, OFF_FS_INFO_SECTOR, OFF_ROOT_CLUSTER,
            parse_boot_sector,
        };

        let mut s = crate::fat::tests::fat32_sector();
        s[OFF_ROOT_CLUSTER..OFF_ROOT_CLUSTER + 4].copy_from_slice(&2u32.to_le_bytes());
        s[OFF_FS_INFO_SECTOR..OFF_FS_INFO_SECTOR + 2].copy_from_slice(&1u16.to_le_bytes());
        s[OFF_BACKUP_BOOT_SECTOR..OFF_BACKUP_BOOT_SECTOR + 2].copy_from_slice(&6u16.to_le_bytes());
        s[OFF_EXT_FLAGS..OFF_EXT_FLAGS + 2].copy_from_slice(&ext_flags.to_le_bytes());

        parse_boot_sector(&s, extent()).expect("the synthetic boot sector is valid FAT32")
    }

    /// A file entry named `FILEnn.TXT`.
    fn numbered(n: usize) -> [u8; ENTRY_BYTES] {
        let mut name = *b"FILE00  TXT";
        name[4] = b'0' + (n / 10) as u8;
        name[5] = b'0' + (n % 10) as u8;
        entry(&name, ATTR_ARCHIVE)
    }

    #[test]
    fn a_single_cluster_root_is_enumerated() {
        let boot = boot(0);
        let mut image = MemoryImage::new(IMAGE_BYTES);
        image.write_cluster(
            2,
            &[
                entry(b"TAPHFIX    ", ATTR_VOLUME_ID),
                numbered(1),
                numbered(2),
            ],
        );
        // The value mkfs.vfat writes for a root directory of one cluster.
        image.write_fat(FAT0_BASE, 2, 0x0FFF_FFF8);

        let root = enumerate_root(&mut image, &boot, extent()).expect("enumerating");

        assert_eq!(root.clusters, vec![2]);
        assert_eq!(root.entries.len(), 3, "the terminator is not an entry");
        assert_eq!(root.short_entry_count(), 2);
        assert!(root.observations.is_empty(), "got {:?}", root.observations);
    }

    /// `mkfs.vfat` terminates the root directory with `0x0FFFFFF8` and files
    /// with `0x0FFFFFFF`. Testing end of chain by equality against the latter
    /// would walk past the end of the root directory on every volume the
    /// project's own fixtures contain.
    #[test]
    fn end_of_chain_is_a_range_not_an_equality() {
        for eoc in [0x0FFF_FFF8u32, 0x0FFF_FFFF] {
            let boot = boot(0);
            let mut image = MemoryImage::new(IMAGE_BYTES);
            image.write_cluster(2, &[numbered(1)]);
            image.write_fat(FAT0_BASE, 2, eoc);

            let root = enumerate_root(&mut image, &boot, extent())
                .unwrap_or_else(|e| panic!("{eoc:#010x} must end the chain, got {e}"));
            assert_eq!(root.clusters, vec![2], "for {eoc:#010x}");
        }
    }

    /// The shape of `fat32-root-multicluster.img`: a full first cluster with
    /// no terminator in it, chaining to a non-adjacent second cluster.
    #[test]
    fn the_chain_is_followed_to_a_non_adjacent_cluster() {
        let boot = boot(0);
        let mut image = MemoryImage::new(IMAGE_BYTES);

        let mut first = vec![entry(b"TAPHFIX    ", ATTR_VOLUME_ID)];
        first.extend((1..=15).map(numbered));
        assert_eq!(
            first.len(),
            CLUSTER_BYTES / ENTRY_BYTES,
            "cluster must be full"
        );
        image.write_cluster(2, &first);

        let second: Vec<_> = (16..=20).map(numbered).collect();
        image.write_cluster(19, &second);

        image.write_fat(FAT0_BASE, 2, 19);
        image.write_fat(FAT0_BASE, 19, 0x0FFF_FFFF);

        let root = enumerate_root(&mut image, &boot, extent()).expect("enumerating");

        assert_eq!(root.clusters, vec![2, 19]);
        assert_eq!(root.entries.len(), 21);
        assert_eq!(root.short_entry_count(), 20);
        assert_eq!(
            root.entries[16].cluster, 19,
            "an entry records the cluster it was read from"
        );
    }

    /// The high four bits of a FAT32 entry are reserved and must be masked
    /// off. No fixture sets them, because no formatting tool writes them,
    /// so this path exists only here.
    #[test]
    fn the_reserved_bits_of_a_fat_entry_are_masked() {
        let boot = boot(0);
        let mut image = MemoryImage::new(IMAGE_BYTES);
        image.write_cluster(2, &[numbered(1)]);
        image.write_cluster(19, &[numbered(2)]);
        image.write_fat(FAT0_BASE, 2, 0xF000_0013);
        image.write_fat(FAT0_BASE, 19, 0xFFFF_FFFF);

        let root = enumerate_root(&mut image, &boot, extent()).expect("enumerating");

        assert_eq!(
            root.clusters,
            vec![2, 19],
            "0xF0000013 addresses cluster 19, and 0xFFFFFFFF ends the chain"
        );
    }

    /// `KNOWN_ISSUES.md` records that no fixture exercises FAT mirroring,
    /// because `mkfs.vfat` offers no option to set `BPB_ExtFlags` and writes
    /// both FATs identically. The in-memory reader can, so the path M5 is
    /// the first to depend on is covered here.
    #[test]
    fn the_active_fat_is_read_when_mirroring_is_disabled() {
        let boot = boot(0x0081);
        assert_eq!(boot.active_fat(), Some(1), "fixture premise");

        let mut image = MemoryImage::new(IMAGE_BYTES);
        image.write_cluster(2, &[numbered(1)]);
        image.write_cluster(19, &[numbered(2)]);

        // FAT 0 is stale and says the directory ends at cluster 2.
        image.write_fat(FAT0_BASE, 2, 0x0FFF_FFFF);
        // FAT 1 is the maintained copy.
        image.write_fat(FAT1_BASE, 2, 19);
        image.write_fat(FAT1_BASE, 19, 0x0FFF_FFFF);

        let root = enumerate_root(&mut image, &boot, extent()).expect("enumerating");

        assert_eq!(
            root.clusters,
            vec![2, 19],
            "reading FAT 0 would stop at cluster 2 and lose half the directory"
        );
    }

    #[test]
    fn mirroring_enabled_reads_fat_zero() {
        let boot = boot(0);
        assert_eq!(boot.active_fat(), None, "fixture premise");

        let mut image = MemoryImage::new(IMAGE_BYTES);
        image.write_cluster(2, &[numbered(1)]);
        image.write_fat(FAT0_BASE, 2, 0x0FFF_FFFF);
        // FAT 1 disagrees. With mirroring enabled it must not be consulted.
        image.write_fat(FAT1_BASE, 2, 19);

        let root = enumerate_root(&mut image, &boot, extent()).expect("enumerating");
        assert_eq!(root.clusters, vec![2]);
    }

    /// The entry bound alone would terminate this walk, at 65,536 entries.
    /// Detecting the cycle is what distinguishes a corrupt chain from a
    /// directory that is merely too large. ADR-0007 Appendix B.3.
    #[test]
    fn a_cyclic_chain_is_named_as_a_cycle() {
        let boot = boot(0);
        let mut image = MemoryImage::new(IMAGE_BYTES);
        image.write_cluster(2, &[numbered(1)]);
        image.write_cluster(3, &[numbered(2)]);
        image.write_fat(FAT0_BASE, 2, 3);
        image.write_fat(FAT0_BASE, 3, 2);

        assert!(
            matches!(
                enumerate_root(&mut image, &boot, extent()),
                Err(DirectoryError::CyclicChain { cluster: 2 })
            ),
            "a loop must be reported as a loop, not as an oversized directory"
        );
    }

    #[test]
    fn a_chain_link_to_a_reserved_cluster_is_refused() {
        let boot = boot(0);
        let mut image = MemoryImage::new(IMAGE_BYTES);
        image.write_cluster(2, &[numbered(1)]);
        image.write_fat(FAT0_BASE, 2, 1);

        assert!(matches!(
            enumerate_root(&mut image, &boot, extent()),
            Err(DirectoryError::ReservedChainLink { from: 2, next: 1 })
        ));
    }

    #[test]
    fn a_chain_link_beyond_the_data_region_is_refused() {
        let boot = boot(0);
        let mut image = MemoryImage::new(IMAGE_BYTES);
        image.write_cluster(2, &[numbered(1)]);
        image.write_fat(FAT0_BASE, 2, CLUSTER_COUNT + 2);

        match enumerate_root(&mut image, &boot, extent()) {
            Err(DirectoryError::ClusterOutOfRange {
                cluster,
                cluster_count,
            }) => {
                assert_eq!(cluster, CLUSTER_COUNT + 2);
                assert_eq!(cluster_count, CLUSTER_COUNT);
            }
            other => panic!("expected ClusterOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn a_bad_cluster_in_the_chain_is_refused() {
        let boot = boot(0);
        let mut image = MemoryImage::new(IMAGE_BYTES);
        image.write_cluster(2, &[numbered(1)]);
        image.write_fat(FAT0_BASE, 2, FAT_BAD_CLUSTER);

        assert!(matches!(
            enumerate_root(&mut image, &boot, extent()),
            Err(DirectoryError::BadCluster { .. })
        ));
    }

    /// ADR-0008 decision C. The specification says nothing follows a
    /// terminator; formatters exist that write one without clearing what
    /// follows, and on real evidence that residue is a record of what the
    /// directory previously held.
    #[test]
    fn content_after_the_terminator_is_reported_not_discarded() {
        let boot = boot(0);
        let mut image = MemoryImage::new(IMAGE_BYTES);

        let mut cluster = vec![0u8; CLUSTER_BYTES];
        cluster[..ENTRY_BYTES].copy_from_slice(&numbered(1));
        // Slot 1 is left zeroed: the terminator.
        let residue = numbered(9);
        cluster[5 * ENTRY_BYTES..6 * ENTRY_BYTES].copy_from_slice(&residue);
        image.write(CLUSTER2_BASE, &cluster);
        image.write_fat(FAT0_BASE, 2, 0x0FFF_FFFF);

        let root = enumerate_root(&mut image, &boot, extent()).expect("enumerating");

        assert_eq!(
            root.entries.len(),
            1,
            "the listing still ends at the terminator"
        );
        assert_eq!(
            root.observations,
            vec![DirectoryObservation::ContentAfterTerminator {
                cluster: 2,
                first_slot: 5,
                slots: 1,
            }],
            "and the finding is still reported"
        );

        assert_eq!(root.residue.len(), 1, "but the slot is now classified");
        assert_eq!(root.residue[0].cluster, 2);
        assert_eq!(root.residue[0].slot, 5);
        assert_eq!(
            root.residue[0].kind,
            classify(&residue),
            "residue is classified by the same function as the listing"
        );
    }

    #[test]
    fn an_invalid_attribute_is_reported_and_the_entry_retained() {
        let boot = boot(0);
        let mut image = MemoryImage::new(IMAGE_BYTES);
        image.write_cluster(2, &[entry(b"CONFUSED   ", ATTR_DIRECTORY | ATTR_VOLUME_ID)]);
        image.write_fat(FAT0_BASE, 2, 0x0FFF_FFFF);

        let root = enumerate_root(&mut image, &boot, extent()).expect("enumerating");

        assert_eq!(root.entries.len(), 1, "an invalid entry is still evidence");
        assert!(matches!(
            root.observations.first(),
            Some(DirectoryObservation::InvalidAttribute {
                cluster: 2,
                slot: 0,
                ..
            })
        ));
    }

    #[test]
    fn a_name_outside_printable_ascii_is_reported_with_its_raw_bytes() {
        let boot = boot(0);
        let mut image = MemoryImage::new(IMAGE_BYTES);
        image.write_cluster(2, &[entry(b"CAF\xe9    TXT", ATTR_ARCHIVE)]);
        image.write_fat(FAT0_BASE, 2, 0x0FFF_FFFF);

        let root = enumerate_root(&mut image, &boot, extent()).expect("enumerating");

        match root.observations.first() {
            Some(DirectoryObservation::NameNotPrintableAscii { raw_name, .. }) => {
                assert_eq!(raw_name, b"CAF\xe9    TXT");
                let rendered = root.observations[0].to_string();
                assert!(
                    rendered.contains("434146e9"),
                    "raw evidence bytes must reach the reader as hex, not as \
                     characters: {rendered}"
                );
            }
            other => panic!("expected NameNotPrintableAscii, got {other:?}"),
        }
    }

    /// The double must fail where `EvidenceFile` fails. If it padded short
    /// reads with zeros instead, every test above would pass against
    /// behaviour the real type does not have.
    #[test]
    fn a_read_past_the_end_of_evidence_fails() {
        let boot = boot(0);
        let mut image = MemoryImage::new(CLUSTER2_BASE);

        assert!(matches!(
            enumerate_root(&mut image, &boot, extent()),
            Err(DirectoryError::Evidence(_))
        ));
    }

    // Directories below the root. ADR-0016.

    /// A directory entry naming `cluster` as its first.
    fn directory_entry(name: &[u8; NAME_LEN], cluster: u32) -> [u8; ENTRY_BYTES] {
        let mut e = entry(name, ATTR_DIRECTORY);
        e[OFF_FST_CLUS_HI..OFF_FST_CLUS_HI + 2]
            .copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
        e[OFF_FST_CLUS_LO..OFF_FST_CLUS_LO + 2].copy_from_slice(&(cluster as u16).to_le_bytes());
        e
    }

    /// The `.` and `..` a directory at `cluster` under the root carries.
    fn dots(cluster: u32) -> [[u8; ENTRY_BYTES]; 2] {
        [
            directory_entry(&DOT_NAME, cluster),
            directory_entry(&DOT_DOT_NAME, 0),
        ]
    }

    /// The positive control for every refusal below: a free first cluster
    /// with both dot entries is read, and its terminator is found.
    #[test]
    fn a_deleted_directory_that_identifies_itself_is_read() {
        let boot = boot(0);
        let mut image = MemoryImage::new(IMAGE_BYTES);
        let [dot, dot_dot] = dots(10);
        image.write_cluster(10, &[dot, dot_dot, numbered(1)]);

        let read = enumerate_deleted_directory(&mut image, &boot, extent(), 10).expect("reading");

        match read {
            DeletedDirectory::Read {
                listing,
                may_continue,
            } => {
                assert_eq!(listing.clusters, vec![10]);
                assert_eq!(listing.entries.len(), 3);
                assert!(!may_continue, "a terminator was written at slot 3");
            }
            other => panic!("expected the directory to be read, got {other:?}"),
        }
    }

    /// ADR-0016 Decision C. A cluster the FAT marks in use belongs to
    /// something else now, even when its dot entries look right.
    #[test]
    fn a_deleted_directory_whose_first_cluster_is_in_use_is_refused() {
        let boot = boot(0);
        let mut image = MemoryImage::new(IMAGE_BYTES);
        let [dot, dot_dot] = dots(10);
        image.write_cluster(10, &[dot, dot_dot]);
        image.write_fat(FAT0_BASE, 10, 0x0FFF_FFF8);

        assert_eq!(
            enumerate_deleted_directory(&mut image, &boot, extent(), 10).expect("reading"),
            DeletedDirectory::Refused(DeletedDirectoryRefusal::InUse {
                fat_entry: 0x0FFF_FFF8
            })
        );
    }

    /// ADR-0016 Decision C. Slot 0 must be `.` naming this cluster: one
    /// naming another cluster, and file data, are both refused.
    #[test]
    fn a_deleted_directory_without_its_own_dot_entry_is_refused() {
        let boot = boot(0);

        let [_, dot_dot] = dots(10);
        let elsewhere = directory_entry(&DOT_NAME, 11);
        for first in [elsewhere, numbered(1)] {
            let mut image = MemoryImage::new(IMAGE_BYTES);
            image.write_cluster(10, &[first, dot_dot]);

            assert_eq!(
                enumerate_deleted_directory(&mut image, &boot, extent(), 10).expect("reading"),
                DeletedDirectory::Refused(DeletedDirectoryRefusal::NoSelfEntry)
            );
        }
    }

    /// ADR-0016 Decision C. Slot 1 must be `..`.
    #[test]
    fn a_deleted_directory_without_a_dot_dot_entry_is_refused() {
        let boot = boot(0);
        let mut image = MemoryImage::new(IMAGE_BYTES);
        let [dot, _] = dots(10);
        image.write_cluster(10, &[dot, numbered(1)]);

        assert_eq!(
            enumerate_deleted_directory(&mut image, &boot, extent(), 10).expect("reading"),
            DeletedDirectory::Refused(DeletedDirectoryRefusal::NoParentEntry)
        );
    }

    /// ADR-0016 Decisions A and D, and EXP-0005 finding 6. A full first
    /// cluster may continue, and the cluster after it is never read in its
    /// place, even when it holds entries that look live.
    #[test]
    fn a_full_first_cluster_may_continue_and_nothing_beyond_it_is_read() {
        let boot = boot(0);
        let mut image = MemoryImage::new(IMAGE_BYTES);
        let [dot, dot_dot] = dots(10);
        let mut first = vec![dot, dot_dot];
        first.extend((1..=14).map(numbered));
        image.write_cluster(10, &first);
        image.write_cluster(11, &[numbered(20), numbered(21)]);

        match enumerate_deleted_directory(&mut image, &boot, extent(), 10).expect("reading") {
            DeletedDirectory::Read {
                listing,
                may_continue,
            } => {
                assert!(may_continue, "sixteen slots and no terminator");
                assert_eq!(listing.clusters, vec![10]);
                assert_eq!(listing.entries.len(), 16);
            }
            other => panic!("expected the directory to be read, got {other:?}"),
        }
    }

    /// Checked before the FAT is consulted, so an out-of-range cluster is an
    /// error about the entry rather than an offset into the FAT.
    #[test]
    fn a_deleted_directory_outside_the_volume_is_an_error() {
        let boot = boot(0);
        let mut image = MemoryImage::new(IMAGE_BYTES);

        for cluster in [0, 1, CLUSTER_COUNT + 2] {
            assert!(matches!(
                enumerate_deleted_directory(&mut image, &boot, extent(), cluster),
                Err(DirectoryError::ClusterOutOfRange { .. })
            ));
        }
    }

    /// ADR-0017 Decision A. The search's first test passes exactly the
    /// clusters whose slots Decision C would accept, and reads no FAT: every
    /// cluster here is marked in use, and Decision C refuses those later.
    #[test]
    fn a_cluster_begins_a_directory_only_with_both_dot_entries() {
        let boot = boot(0);
        let [dot, dot_dot] = dots(10);
        let elsewhere = directory_entry(&DOT_NAME, 11);
        let file_named_dot = entry(&DOT_NAME, ATTR_ARCHIVE);

        let cases: [(&[[u8; ENTRY_BYTES]], bool); 6] = [
            (&[dot, dot_dot], true),
            (&[dot, dot_dot, numbered(1)], true),
            (&[elsewhere, dot_dot], false),
            (&[file_named_dot, dot_dot], false),
            (&[dot, numbered(1)], false),
            (&[], false),
        ];
        for (slots, expected) in cases {
            let mut image = MemoryImage::new(IMAGE_BYTES);
            image.write_cluster(10, slots);
            image.write_fat(FAT0_BASE, 10, 0x0FFF_FFF8);

            assert_eq!(
                begins_directory(&mut image, &boot, extent(), 10).expect("reading"),
                expected,
                "slots {slots:?}"
            );
        }
    }

    /// ADR-0017 Decision A. The search visits the first and last data
    /// clusters, and a cluster outside them is an error, not an offset.
    #[test]
    fn the_search_test_is_bounded_by_the_volume() {
        let boot = boot(0);
        let mut image = MemoryImage::new(IMAGE_BYTES);

        for cluster in [2, CLUSTER_COUNT + 1] {
            assert!(!begins_directory(&mut image, &boot, extent(), cluster).expect("reading"));
        }
        for cluster in [0, 1, CLUSTER_COUNT + 2] {
            assert!(matches!(
                begins_directory(&mut image, &boot, extent(), cluster),
                Err(DirectoryError::ClusterOutOfRange { .. })
            ));
        }
    }

    /// ADR-0016 Decision A. A live directory is read along its chain.
    #[test]
    fn a_live_directory_is_read_along_its_chain() {
        let boot = boot(0);
        let mut image = MemoryImage::new(IMAGE_BYTES);
        let [dot, dot_dot] = dots(10);
        let mut first = vec![dot, dot_dot];
        first.extend((1..=14).map(numbered));
        image.write_cluster(10, &first);
        image.write_cluster(12, &[numbered(15)]);
        image.write_fat(FAT0_BASE, 10, 12);
        image.write_fat(FAT0_BASE, 12, 0x0FFF_FFF8);

        let directory = enumerate_directory(&mut image, &boot, extent(), 10).expect("reading");

        assert_eq!(directory.clusters, vec![10, 12]);
        assert_eq!(directory.entries.len(), 17);
    }

    /// ADR-0016 Decision B. Exactly `.` and `..`, as directories: a
    /// directory merely named with dots, or a file named `.`, is not one.
    #[test]
    fn only_the_two_dot_entries_are_dot_entries() {
        let [dot, dot_dot] = dots(10);
        assert!(is_dot_entry(&classify(&dot)));
        assert!(is_dot_entry(&classify(&dot_dot)));

        assert!(!is_dot_entry(&classify(&directory_entry(
            b"DOT        ",
            10
        ))));
        assert!(!is_dot_entry(&classify(&entry(&DOT_NAME, ATTR_ARCHIVE))));
    }
}
