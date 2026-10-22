//! exFAT directories: following a directory's clusters, and reading the
//! entry sets in them; and reading a live file's content.
//!
//! ADR-0019 section 3. The root directory is always a FAT chain
//! (specification section 3.1.10); every other directory follows its entry's
//! `NoFatChain` flag (section 6.3.4.2): one contiguous run, or a chain.
//!
//! Everything read is untrusted evidence, and nothing is loaded whole. A FAT
//! can describe gigabytes and a directory is allowed 256 MB (section 9.5), so
//! FAT entries are read on demand through a one-sector cache, and a directory
//! is read as a stream of entries in chunks of at most 4 KiB. An entry set,
//! which is at most 256 entries, is the only thing held at once.
//!
//! # What stops a directory
//!
//! Every cluster is read as directory data at most once for a whole volume,
//! so a cycle, or two entries naming the same directory, cannot loop. A chain
//! longer than the directory limit, a link outside the cluster heap, a link
//! to a cluster the FAT marks bad, and the total number of clusters read as
//! directories all stop the directory they occur in, with the reason. The
//! last is a safety bound this module chose, [`MAX_DIRECTORY_CLUSTERS`]; the
//! specification has no such limit. A subdirectory's allocation must be
//! exactly its `DataLength` (section 7.6.7), so a chain that goes on past
//! that length, and a first cluster with a length of 0, stop it too.
//!
//! # What is and is not used
//!
//! An entry set is used only if it is well formed and its checksum is valid
//! (ADR-0019 Decision C). One that is not is reported, with the reason, and
//! not used. Deleted entry sets, those whose `InUse` bit is clear, are not
//! examined in this milestone: an unused File entry is counted, and nothing
//! in its secondaries is interpreted. The allocation bitmap's entry is
//! located and range checked by the walk. Its contents are read only
//! through [`ExFatReader::open_bitmap`] and
//! [`ExFatReader::cluster_allocated`], which the walk does not call yet.
//! Where the up-case table lies, and every live set's allocation, are kept
//! for [`ExFatReader::check_bitmap`], which checks the bitmap against what
//! the walk knows is in use; nothing calls it yet either.
//!
//! # Content
//!
//! [`ExFatReader::read_content`] reads a live file's data: a contiguous run
//! where `NoFatChain` is set, otherwise the chain, which is followed
//! through the FAT and must be exactly as long as `DataLength` needs before
//! any data is read. Bytes past `ValidDataLength` are handed over as zeros
//! and never read (ADR-0019 Decision F). Data is handed over in chunks, so
//! memory does not grow with the file. Deleted files are not read here.
//!
//! # Names
//!
//! A name is UTF-16 in the evidence and is kept as code units. [`escaped_name`]
//! is the one way it becomes text: no code unit outside printable ASCII
//! reaches a terminal or a path as itself.

use std::collections::HashSet;
use std::fmt;

use crate::error::Error;
use crate::evidence::EvidenceReader;
use crate::exfat_boot::ExFatBootSector;
use crate::filesystem::{VolumeExtent, le_u16, le_u32};
use crate::partition::SECTOR_SIZE;

/// Bytes in a directory entry. Section 6.
pub const ENTRY_BYTES: usize = 32;

/// The most data a directory may hold: 256 MB, 8,388,608 entries.
/// Sections 7.6.7 and 9.5.
pub const MAX_DIRECTORY_BYTES: u64 = 268_435_456;

/// The most clusters read as directories on one volume.
///
/// A safety bound chosen here, not a limit the specification gives. Every
/// directory cluster is remembered so that none is read twice, and a hostile
/// volume could otherwise make that memory, and the run, as large as the
/// cluster heap. 2^20 clusters is 4 GiB of directories at 4 KiB a cluster.
/// A volume that reaches it has its remaining directories counted as not
/// read.
pub const MAX_DIRECTORY_CLUSTERS: u32 = 1 << 20;

/// The limits a walk is held to.
///
/// The defaults are the constants above. A walk takes them as a value so a
/// test can use a small one, rather than building a volume that reaches 256 MB
/// to see the limit act.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    /// Most clusters read as directories over the whole volume.
    pub directory_clusters: u32,

    /// Most bytes one directory's chain may span.
    pub directory_bytes: u64,

    /// Most directories held at once, named and not yet read.
    ///
    /// Read by the caller that queues directories, and kept here so one value
    /// carries every bound a walk is held to.
    pub queued_directories: usize,

    /// Most live sets whose allocation is kept for the bitmap check.
    pub live_allocations: usize,
}

impl Limits {
    /// [`MAX_DIRECTORY_CLUSTERS`], [`MAX_DIRECTORY_BYTES`],
    /// [`MAX_QUEUED_DIRECTORIES`] and [`MAX_LIVE_ALLOCATIONS`].
    pub const DEFAULT: Limits = Limits {
        directory_clusters: MAX_DIRECTORY_CLUSTERS,
        directory_bytes: MAX_DIRECTORY_BYTES,
        queued_directories: MAX_QUEUED_DIRECTORIES,
        live_allocations: MAX_LIVE_ALLOCATIONS,
    };
}

/// The most live files and directories whose allocation is kept for the
/// bitmap check.
///
/// A safety bound chosen here, not a limit the specification gives. Each is
/// 12 bytes, so the full record is 48 MiB; while it grows from half that
/// size the old and new buffers are briefly both held, up to 72 MiB. 2^22 is
/// eight times the files of a 2 TB card filled with 4 MB photographs. A
/// volume with more has
/// the rest unrecorded, and its bitmap is then not believed, because a live
/// file the check did not see could be marked free.
pub const MAX_LIVE_ALLOCATIONS: usize = 1 << 22;

/// How many clusters the bitmap check looks up for live allocations, as a
/// multiple of the volume's ClusterCount.
///
/// A safety bound chosen here, not the specification's. Each cluster belongs
/// to one allocation at a time, so a consistent volume needs at most one
/// times; four leaves room for allocations that overlap, and stops a volume
/// whose millions of entries name the same long chain after four lookups
/// per cluster of the volume, not millions. A lookup reads at most one FAT
/// sector and one bitmap sector, so the worst case is about eight sector
/// reads per cluster. The same multiple as ADR-0022 Decision J's links
/// bound.
pub const LIVE_CLUSTERS_PER_CLUSTER: u64 = 4;

/// The most directories held at once, named by entries and not yet read.
///
/// A safety bound chosen here, not a limit the specification gives. Each
/// queued directory costs its own name, up to 510 bytes, and a hostile
/// volume can name millions. A directory that would exceed the bound is not
/// queued, and is counted as not read. A queued directory costs at most
/// about 0.6 KB, so the queue itself is held to about 80 MB. The names of its
/// ancestors stay alive while any descendant is queued, and those are bounded
/// by [`MAX_DIRECTORY_CLUSTERS`], not by this.
pub const MAX_QUEUED_DIRECTORIES: usize = 1 << 17;

/// The most entries in one set: the primary and 255 secondaries.
/// Section 6.3.2.
const MAX_SET_ENTRIES: usize = 256;

/// The fewest secondaries a File set has: a Stream Extension and one File
/// Name. Section 7.4.
const MIN_FILE_SECONDARIES: usize = 2;

/// Characters in one File Name entry. Section 7.7.3.
const NAME_UNITS_PER_ENTRY: usize = 15;

/// The longest name. Section 7.6.3.
const MAX_NAME_UNITS: usize = 255;

/// Bytes read from the evidence at a time while streaming a directory.
const READ_CHUNK_BYTES: usize = 4096;

/// The longest up-case table whose clusters the bitmap check follows:
/// 131,072 bytes, one 2-byte mapping for each of the 65,536 code units,
/// uncompressed (section 7.2.5).
///
/// A choice of this project, not a limit the specification gives: it sets
/// no maximum, and a compressed table that spends more than two bytes on a
/// code unit could be longer. exfatprogs' checker refuses a longer table.
/// The bound holds the chain followed to at most 256 clusters, whatever
/// `DataLength` claims. The tables measured are 5,836 bytes, the
/// specification's recommended one, and 4,104 where FatFs formatted the
/// volume (EXP-0011 and its Appendix A).
pub const MAX_UP_CASE_BYTES: u64 = 131_072;

/// Most bytes of a file's content read, or handed over, at a time. A cluster
/// can be 32 MiB (section 3.1.9), so a cluster is read in pieces of this.
pub const CONTENT_CHUNK_BYTES: usize = 64 * 1024;

/// A FAT entry marking its cluster bad, and one ending a chain.
/// Section 4.1.3.
const FAT_BAD_CLUSTER: u32 = 0xFFFF_FFF7;
const FAT_END_OF_CHAIN: u32 = 0xFFFF_FFFF;

/// `EntryType` bits. Section 6.2.1.
const TYPE_IN_USE: u8 = 0x80;
const TYPE_SECONDARY: u8 = 0x40;
const TYPE_BENIGN: u8 = 0x20;

/// The entry types this module recognises. Each is a TypeCode plus the
/// importance, category and InUse bits.
const TYPE_END_OF_DIRECTORY: u8 = 0x00;
const TYPE_UNUSED_FILE: u8 = 0x05;
const TYPE_INVALID: u8 = 0x80;
const TYPE_BITMAP: u8 = 0x81;
const TYPE_UP_CASE: u8 = 0x82;
const TYPE_VOLUME_LABEL: u8 = 0x83;
const TYPE_FILE: u8 = 0x85;
const TYPE_VOLUME_GUID: u8 = 0xA0;
const TYPE_STREAM: u8 = 0xC0;
const TYPE_FILE_NAME: u8 = 0xC1;

/// A secondary that is in use and benign: any such entry a File set carries
/// after its names is ignorable. Section 6.4.1.2.2.
const BENIGN_SECONDARY_MASK: u8 = TYPE_IN_USE | TYPE_SECONDARY | TYPE_BENIGN;

/// `FileAttributes` bit for a directory. Section 7.4.4.
const ATTRIBUTE_DIRECTORY: u16 = 0x10;

/// `GeneralSecondaryFlags` bits. Section 6.4.2.
const FLAG_ALLOCATION_POSSIBLE: u8 = 0x01;
const FLAG_NO_FAT_CHAIN: u8 = 0x02;

/// Most characters in a volume label. Section 7.3.2.
const MAX_LABEL_UNITS: usize = 11;

/// Where an entry lies: the cluster it was read from and its slot there,
/// counting from zero. The same position FAT32 reports.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExFatPosition {
    /// The directory cluster holding the entry.
    pub cluster: u32,

    /// The entry's slot within that cluster.
    pub slot: usize,
}

/// A live file or directory: a File entry set that passed every check.
///
/// No timestamp is read. Content is not here; where the clusters are and how
/// long the data is, are.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ExFatFile {
    /// The name, as UTF-16 code units, exactly as stored.
    pub name: Vec<u16>,

    /// `FileAttributes`, as stored.
    pub attributes: u16,

    /// First cluster of the data, or 0 where there is none.
    pub first_cluster: u32,

    /// Size of the allocation in bytes.
    pub data_length: u64,

    /// How far data has been written. At most `data_length`; equal to it for
    /// a directory. Bytes past it are not the file's content (section 7.6.5).
    pub valid_data_length: u64,

    /// The data is one contiguous run and the FAT does not describe it.
    pub no_fat_chain: bool,
}

impl ExFatFile {
    /// Whether this is a directory.
    pub const fn is_directory(&self) -> bool {
        self.attributes & ATTRIBUTE_DIRECTORY != 0
    }
}

/// The allocation bitmap's entry, located and range checked by the walk.
///
/// Its contents are read only through [`ExFatReader::open_bitmap`] and
/// [`ExFatReader::cluster_allocated`], which the walk does not call yet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExFatBitmapEntry {
    /// First cluster of the bitmap, which the FAT describes.
    pub first_cluster: u32,

    /// Size of the bitmap in bytes.
    pub data_length: u64,
}

/// The allocation bitmap, located through its FAT chain (section 7.1.3).
///
/// Holds where the bitmap's clusters are and one sector of it, so a lookup
/// costs at most one sector read. The runs grow with how fragmented the
/// bitmap's chain is, and are bounded by the bitmap the volume needs: at most
/// 2^20 clusters, whatever the evidence's size.
#[derive(Debug)]
pub struct ExFatBitmap {
    /// The bitmap's clusters, in chain order, as runs of consecutive ones.
    runs: Vec<BitmapRun>,

    /// The one bitmap sector held, and its index within the bitmap.
    sector: Vec<u8>,
    sector_index: Option<u64>,

    /// The volume the bitmap was opened on. A lookup through a reader of
    /// another volume is refused, so the runs and the sector buffer are
    /// never applied to a geometry they were not built for.
    volume: BitmapVolume,
}

/// What identifies the volume a bitmap belongs to: where it starts and the
/// geometry its runs and sector buffer were sized from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BitmapVolume {
    volume_start: u64,
    bytes_per_sector_shift: u8,
    sectors_per_cluster_shift: u8,
    cluster_heap_offset: u32,
    cluster_count: u32,
}

impl ExFatBitmap {
    /// Clusters the bitmap occupies.
    pub fn clusters(&self) -> u64 {
        self.runs.iter().map(|r| r.clusters as u64).sum()
    }
}

/// The up-case table's entry, as the root held it. Section 7.2.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct UpCaseEntry {
    first_cluster: u32,
    data_length: u64,
}

/// Where a live file or directory's data is, as its set describes it: the
/// clusters `DataLength` needs, from `first_cluster`, as one run or a chain.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct LiveAllocation {
    first_cluster: u32,
    clusters: u32,
    no_fat_chain: bool,
    directory: bool,
}

/// Consecutive clusters of a chain the walk follows for structure (the
/// bitmap's, or the up-case table's): `clusters` of them from `first`,
/// holding the chain's clusters from index `start` on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BitmapRun {
    start: u32,
    first: u32,
    clusters: u32,
}

/// Why the allocation bitmap could not be located or read.
#[derive(Debug)]
pub enum ExFatBitmapError {
    /// The evidence could not be read.
    Evidence(Error),

    /// The bitmap's first cluster lies outside the cluster heap.
    FirstClusterOutOfRange { cluster: u32 },

    /// The bitmap has fewer bits than the volume has clusters.
    TooShort { data_length: u64, needed: u64 },

    /// The bitmap is longer than the volume's clusters need, rounded up to
    /// a whole cluster: `limit` bytes.
    ///
    /// A choice of this project. Section 7.1.5 allows reserved bits after
    /// the last cluster's; mkfs.exfat writes none (`round_up(clusters, 8) /
    /// 8`), exfatprogs' checker treats any other length as an error, and
    /// The Sleuth Kit refuses any length but the exact one. Refusing a
    /// longer one bounds both the chain walked and the runs held by the
    /// bitmap's size, not the evidence's. Every image in EXP-0011 has a
    /// bitmap of exactly the needed length, so none is refused.
    LongerThanVolumeNeeds { data_length: u64, limit: u64 },

    /// A FAT entry the FAT region does not hold.
    FatOutOfRange { cluster: u32 },

    /// The bitmap's chain reaches a cluster the FAT marks bad.
    BadCluster { cluster: u32 },

    /// A FAT entry links outside the heap, or to a free or reserved one.
    LinkOutOfRange { from: u32, to: u32 },

    /// The chain ends at `cluster`, before the `needed` clusters
    /// `DataLength` takes.
    ChainEndsEarly { cluster: u32, needed: u64 },

    /// The chain goes on past `cluster`, the last of the `needed` clusters
    /// `DataLength` takes. A choice of this project, as for content.
    ChainLongerThanData { cluster: u32, needed: u64 },

    /// A cluster asked about lies outside the cluster heap.
    ClusterOutOfRange { cluster: u32 },

    /// The bitmap was opened on another volume than the reader's.
    OtherVolume,
}

impl fmt::Display for ExFatBitmapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Evidence(e) => write!(f, "{e}"),
            Self::FirstClusterOutOfRange { cluster } => write!(
                f,
                "allocation bitmap first cluster {cluster} is outside the cluster heap"
            ),
            Self::TooShort {
                data_length,
                needed,
            } => write!(
                f,
                "allocation bitmap of {data_length} bytes is shorter than the {needed} the volume needs"
            ),
            Self::LongerThanVolumeNeeds { data_length, limit } => write!(
                f,
                "allocation bitmap of {data_length} bytes is longer than the {limit} the volume's clusters need"
            ),
            Self::FatOutOfRange { cluster } => {
                write!(f, "the FAT does not hold an entry for cluster {cluster}")
            }
            Self::BadCluster { cluster } => {
                write!(f, "allocation bitmap chain reaches bad cluster {cluster}")
            }
            Self::LinkOutOfRange { from, to } => {
                write!(f, "cluster {from} links to {to}, outside the cluster heap")
            }
            Self::ChainEndsEarly { cluster, needed } => write!(
                f,
                "allocation bitmap chain ends at cluster {cluster}, short of the {needed} clusters its length needs"
            ),
            Self::ChainLongerThanData { cluster, needed } => write!(
                f,
                "allocation bitmap chain goes on past cluster {cluster}, the last of the {needed} its length needs"
            ),
            Self::ClusterOutOfRange { cluster } => {
                write!(f, "cluster {cluster} is outside the cluster heap")
            }
            Self::OtherVolume => f.write_str("allocation bitmap belongs to another volume"),
        }
    }
}

impl std::error::Error for ExFatBitmapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Evidence(e) => Some(e),
            _ => None,
        }
    }
}

impl From<Error> for ExFatBitmapError {
    fn from(e: Error) -> Self {
        Self::Evidence(e)
    }
}

impl From<ChainFault> for ExFatBitmapError {
    fn from(fault: ChainFault) -> Self {
        match fault {
            ChainFault::Evidence(e) => Self::Evidence(e),
            ChainFault::FatOutOfRange { cluster } => Self::FatOutOfRange { cluster },
            ChainFault::BadCluster { cluster } => Self::BadCluster { cluster },
            ChainFault::LinkOutOfRange { from, to } => Self::LinkOutOfRange { from, to },
            ChainFault::EndsEarly { cluster, needed } => Self::ChainEndsEarly { cluster, needed },
            ChainFault::LongerThanData { cluster, needed } => {
                Self::ChainLongerThanData { cluster, needed }
            }
        }
    }
}

/// Why a structure chain could not be followed. Each caller turns it into
/// an error that names its structure.
enum ChainFault {
    Evidence(Error),
    FatOutOfRange { cluster: u32 },
    BadCluster { cluster: u32 },
    LinkOutOfRange { from: u32, to: u32 },
    EndsEarly { cluster: u32, needed: u64 },
    LongerThanData { cluster: u32, needed: u64 },
}

impl From<Error> for ChainFault {
    fn from(e: Error) -> Self {
        Self::Evidence(e)
    }
}

/// What the walk knows is in use, which the bitmap must mark so.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum ExFatStructure {
    /// A cluster of the root or of a directory.
    Directory,

    /// A cluster of the allocation bitmap itself.
    Bitmap,

    /// A cluster of the up-case table.
    UpCaseTable,

    /// A cluster of a live file's data.
    File,
}

impl fmt::Display for ExFatStructure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Directory => "directory",
            Self::Bitmap => "allocation bitmap",
            Self::UpCaseTable => "up-case table",
            Self::File => "live file",
        })
    }
}

/// What the bitmap check found in use: clusters of each kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExFatStructureChecked {
    /// Clusters read as the root and as directories.
    pub directory_clusters: u64,

    /// Clusters of the allocation bitmap.
    pub bitmap_clusters: u64,

    /// Clusters of the up-case table.
    pub up_case_clusters: u64,

    /// Clusters of live files' and directories' allocations, as their sets
    /// describe them. A directory that was read is counted here too.
    pub live_clusters: u64,
}

/// Why the allocation bitmap is not believed (ADR-0022 Decision E).
#[derive(Debug)]
pub enum ExFatStructureError {
    /// The evidence could not be read.
    Evidence(Error),

    /// A lookup in the bitmap failed.
    Bitmap(ExFatBitmapError),

    /// The root held no up-case table entry. Section 7.2 requires one.
    NoUpCaseTable,

    /// The root held `entries` up-case table entries. Section 7.2 allows
    /// one, so none of them can be shown to be the table.
    UpCaseTables { entries: usize },

    /// The up-case table's first cluster lies outside the cluster heap.
    UpCaseFirstClusterOutOfRange { cluster: u32 },

    /// The up-case table has a `DataLength` of 0, so it holds no table.
    UpCaseEmpty,

    /// The up-case table is longer than [`MAX_UP_CASE_BYTES`], a bound this
    /// project chose.
    UpCaseTooLong { data_length: u64, limit: u64 },

    /// A FAT entry the FAT region does not hold.
    UpCaseFatOutOfRange { cluster: u32 },

    /// The up-case table's chain reaches a cluster the FAT marks bad.
    UpCaseBadCluster { cluster: u32 },

    /// A FAT entry of the up-case table's chain links outside the heap.
    UpCaseLinkOutOfRange { from: u32, to: u32 },

    /// The up-case table's chain ends at `cluster`, before the `needed`
    /// clusters `DataLength` takes.
    UpCaseChainEndsEarly { cluster: u32, needed: u64 },

    /// The up-case table's chain goes on past `cluster`, the last of the
    /// `needed` clusters `DataLength` takes.
    UpCaseChainLongerThanData { cluster: u32, needed: u64 },

    /// More live sets than [`Limits::live_allocations`] were found, so
    /// `unrecorded` of them were not kept and cannot be checked.
    LiveAllocationsUnrecorded { bound: usize, unrecorded: u64 },

    /// Live allocations name more clusters than [`LIVE_CLUSTERS_PER_CLUSTER`]
    /// times the volume's, `limit`: a bound this project chose.
    LiveClustersBeyondBound { limit: u64 },

    /// The bitmap marks a cluster the walk knows is in use as free.
    /// `cluster` is the lowest such, and `structure` what it belongs to.
    MarkedFree {
        structure: ExFatStructure,
        cluster: u32,
    },
}

impl fmt::Display for ExFatStructureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Evidence(e) => write!(f, "{e}"),
            Self::Bitmap(e) => write!(f, "{e}"),
            Self::NoUpCaseTable => f.write_str("the root holds no up-case table entry"),
            Self::UpCaseTables { entries } => {
                write!(f, "the root holds {entries} up-case table entries, not one")
            }
            Self::UpCaseFirstClusterOutOfRange { cluster } => write!(
                f,
                "up-case table first cluster {cluster} is outside the cluster heap"
            ),
            Self::UpCaseEmpty => f.write_str("up-case table has a length of 0"),
            Self::UpCaseTooLong { data_length, limit } => write!(
                f,
                "up-case table of {data_length} bytes is longer than the {limit} this check follows"
            ),
            Self::UpCaseFatOutOfRange { cluster } => write!(
                f,
                "up-case table chain reaches cluster {cluster}, which the FAT holds no entry for"
            ),
            Self::UpCaseBadCluster { cluster } => {
                write!(f, "up-case table chain reaches bad cluster {cluster}")
            }
            Self::UpCaseLinkOutOfRange { from, to } => write!(
                f,
                "up-case table chain: cluster {from} links to {to}, outside the cluster heap"
            ),
            Self::UpCaseChainEndsEarly { cluster, needed } => write!(
                f,
                "up-case table chain ends at cluster {cluster}, short of the {needed} clusters its length needs"
            ),
            Self::UpCaseChainLongerThanData { cluster, needed } => write!(
                f,
                "up-case table chain goes on past cluster {cluster}, the last of the {needed} its length needs"
            ),
            Self::LiveAllocationsUnrecorded { bound, unrecorded } => write!(
                f,
                "{unrecorded} live files or directories past the {bound} kept were not checked"
            ),
            Self::LiveClustersBeyondBound { limit } => write!(
                f,
                "live allocations name more than the {limit} clusters this check looks up"
            ),
            Self::MarkedFree { structure, cluster } => write!(
                f,
                "the allocation bitmap marks cluster {cluster} of the {structure} free"
            ),
        }
    }
}

impl std::error::Error for ExFatStructureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Evidence(e) => Some(e),
            Self::Bitmap(e) => Some(e),
            _ => None,
        }
    }
}

impl From<Error> for ExFatStructureError {
    fn from(e: Error) -> Self {
        Self::Evidence(e)
    }
}

impl From<ExFatBitmapError> for ExFatStructureError {
    fn from(e: ExFatBitmapError) -> Self {
        Self::Bitmap(e)
    }
}

impl From<ChainFault> for ExFatStructureError {
    fn from(fault: ChainFault) -> Self {
        match fault {
            ChainFault::Evidence(e) => Self::Evidence(e),
            ChainFault::FatOutOfRange { cluster } => Self::UpCaseFatOutOfRange { cluster },
            ChainFault::BadCluster { cluster } => Self::UpCaseBadCluster { cluster },
            ChainFault::LinkOutOfRange { from, to } => Self::UpCaseLinkOutOfRange { from, to },
            ChainFault::EndsEarly { cluster, needed } => {
                Self::UpCaseChainEndsEarly { cluster, needed }
            }
            ChainFault::LongerThanData { cluster, needed } => {
                Self::UpCaseChainLongerThanData { cluster, needed }
            }
        }
    }
}

/// Why an entry set, or an entry, was not used.
///
/// Each is a statement about the evidence. A set refused here is counted as
/// not analysed, so a run cannot report a volume complete while skipping it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SetRefusal {
    /// `SecondaryCount` is below 2, so there is no room for a Stream
    /// Extension and a File Name. Section 7.4.
    SecondaryCountTooSmall { count: u8 },

    /// Fewer entries followed than `SecondaryCount` declared, or one that
    /// is not a secondary entry in use came first.
    SetTruncated { declared: usize, found: usize },

    /// The set's checksum is not the one stored. Section 6.3.3.
    ChecksumMismatch { stored: u16, computed: u16 },

    /// The first secondary is not a Stream Extension. Section 7.6.
    StreamExtensionMissing { found: u8 },

    /// The Stream Extension says no allocation is possible. Section 7.6.2.1.
    AllocationNotPossible,

    /// `NameLength` is outside 1 to 255. Section 7.6.3.
    NameLengthOutOfRange { length: u8 },

    /// The number of File Name entries is not `ceil(NameLength / 15)`.
    /// Section 7.7.
    NameEntries { expected: usize, found: usize },

    /// A critical secondary this module does not recognise, which makes the
    /// whole set unrecognised. Section 6.4.1.2.1.
    UnrecognisedCriticalSecondary { entry_type: u8 },

    /// `ValidDataLength` exceeds `DataLength`. Section 7.6.5.
    ValidDataLengthExceedsDataLength {
        valid_data_length: u64,
        data_length: u64,
    },

    /// A directory whose `ValidDataLength` is not its `DataLength`.
    /// Section 7.6.5.
    DirectoryValidDataLength {
        valid_data_length: u64,
        data_length: u64,
    },

    /// A directory larger than 256 MB. Section 7.6.7.
    DirectoryTooLarge { data_length: u64 },

    /// `DataLength` exceeds the whole cluster heap. Section 6.2.3.
    DataLengthTooLarge { data_length: u64 },

    /// A first cluster of 0 with data, which section 6.2.3 allows only for
    /// an allocation that does not exist.
    DataWithoutCluster { data_length: u64 },

    /// `NoFatChain` with no first cluster. Section 6.3.4.2.
    NoFatChainWithoutCluster,

    /// `NoFatChain` with no data. Section 6.3.6.
    NoFatChainWithoutData,

    /// A first cluster outside 2 to `ClusterCount + 1`. Section 6.2.2.
    FirstClusterOutOfRange { cluster: u32 },

    /// A contiguous allocation that runs past the cluster heap.
    AllocationOutOfRange { first_cluster: u32, clusters: u64 },

    /// A critical primary entry that is in use and has type `80h`, which the
    /// specification calls invalid. Section 6.2.1.
    InvalidEntryType { entry_type: u8 },

    /// A secondary entry in use where an entry set must begin.
    StraySecondary { entry_type: u8 },

    /// A benign primary this module does not recognise, which makes the set
    /// unrecognised. Section 6.3.1.2.2.
    UnrecognisedBenignPrimary { entry_type: u8 },

    /// A volume label longer than 11 characters. Section 7.3.2.
    LabelLength { count: u8 },

    /// An allocation bitmap entry for the second bitmap, on a volume with
    /// one FAT. Section 7.1.2.1.
    BitmapIdentifier { flags: u8 },

    /// A bitmap first cluster outside the cluster heap. Section 7.1.3.
    BitmapFirstCluster { cluster: u32 },

    /// A bitmap with fewer bits than the volume has clusters. Section 7.1.5.
    BitmapTooShort { data_length: u64, needed: u64 },

    /// A second allocation bitmap entry. Section 7.1.
    DuplicateBitmap,
}

impl fmt::Display for SetRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SecondaryCountTooSmall { count } => {
                write!(
                    f,
                    "secondary count {count} leaves no room for a stream and a name"
                )
            }
            Self::SetTruncated { declared, found } => write!(
                f,
                "set declares {declared} secondary entries and {found} followed"
            ),
            Self::ChecksumMismatch { stored, computed } => write!(
                f,
                "set checksum {computed:#06x} does not match the stored {stored:#06x}"
            ),
            Self::StreamExtensionMissing { found } => {
                write!(
                    f,
                    "entry type {found:#04x} where a stream extension belongs"
                )
            }
            Self::AllocationNotPossible => {
                f.write_str("stream extension says no allocation is possible")
            }
            Self::NameLengthOutOfRange { length } => {
                write!(f, "name length {length} is outside 1 to {MAX_NAME_UNITS}")
            }
            Self::NameEntries { expected, found } => write!(
                f,
                "name length needs {expected} name entries and {found} follow"
            ),
            Self::UnrecognisedCriticalSecondary { entry_type } => write!(
                f,
                "critical secondary entry {entry_type:#04x} is not recognised"
            ),
            Self::ValidDataLengthExceedsDataLength {
                valid_data_length,
                data_length,
            } => write!(
                f,
                "valid data length {valid_data_length} exceeds data length {data_length}"
            ),
            Self::DirectoryValidDataLength {
                valid_data_length,
                data_length,
            } => write!(
                f,
                "directory valid data length {valid_data_length} is not its data length {data_length}"
            ),
            Self::DirectoryTooLarge { data_length } => write!(
                f,
                "directory data length {data_length} exceeds {MAX_DIRECTORY_BYTES}"
            ),
            Self::DataLengthTooLarge { data_length } => {
                write!(f, "data length {data_length} exceeds the cluster heap")
            }
            Self::DataWithoutCluster { data_length } => {
                write!(f, "data length {data_length} with no first cluster")
            }
            Self::NoFatChainWithoutCluster => {
                f.write_str("no FAT chain flag with no first cluster")
            }
            Self::NoFatChainWithoutData => f.write_str("no FAT chain flag with no data"),
            Self::FirstClusterOutOfRange { cluster } => {
                write!(f, "first cluster {cluster} is outside the cluster heap")
            }
            Self::AllocationOutOfRange {
                first_cluster,
                clusters,
            } => write!(
                f,
                "{clusters} contiguous clusters from {first_cluster} run past the cluster heap"
            ),
            Self::InvalidEntryType { entry_type } => {
                write!(f, "entry type {entry_type:#04x} is invalid")
            }
            Self::StraySecondary { entry_type } => {
                write!(f, "secondary entry {entry_type:#04x} outside any set")
            }
            Self::UnrecognisedBenignPrimary { entry_type } => {
                write!(
                    f,
                    "benign primary entry {entry_type:#04x} is not recognised"
                )
            }
            Self::LabelLength { count } => {
                write!(f, "volume label length {count} exceeds {MAX_LABEL_UNITS}")
            }
            Self::BitmapIdentifier { flags } => write!(
                f,
                "allocation bitmap flags {flags:#04x} name the second bitmap"
            ),
            Self::BitmapFirstCluster { cluster } => {
                write!(
                    f,
                    "allocation bitmap first cluster {cluster} is outside the cluster heap"
                )
            }
            Self::BitmapTooShort {
                data_length,
                needed,
            } => write!(
                f,
                "allocation bitmap of {data_length} bytes is shorter than the {needed} the volume needs"
            ),
            Self::DuplicateBitmap => f.write_str("second allocation bitmap entry"),
        }
    }
}

/// Why a directory could not be read, wholly or in part.
#[derive(Debug)]
pub enum ExFatDirectoryError {
    /// The evidence could not be read.
    Evidence(Error),

    /// A directory's first cluster lies outside the cluster heap.
    ClusterOutOfRange { cluster: u32 },

    /// A FAT entry links to a cluster outside the cluster heap, or to a free
    /// or reserved one.
    LinkOutOfRange { from: u32, to: u32 },

    /// A directory's chain reaches a cluster the FAT marks bad.
    BadCluster { cluster: u32 },

    /// A cluster that was already read as a directory.
    ClusterAlreadyRead { cluster: u32 },

    /// A directory's chain ends before its data length is covered.
    ChainEndsEarly { cluster: u32 },

    /// A directory's chain goes on past `cluster`, the last its data length
    /// needs. A directory's length is its whole allocation, section 7.6.7.
    ChainLongerThanData { cluster: u32 },

    /// A subdirectory whose first cluster is `cluster` and whose data length
    /// is 0. A directory's length is its whole allocation, section 7.6.7.
    ClusterWithoutData { cluster: u32 },

    /// A directory's chain spans more than the directory limit.
    ChainTooLong { limit_bytes: u64 },

    /// The volume's cluster cap on directories was reached. This and every
    /// directory not yet read are counted as not read.
    ClusterCapReached { cap: u32 },

    /// A FAT entry that the FAT region does not hold.
    FatOutOfRange { cluster: u32 },

    /// A critical primary entry this module does not recognise, in the root
    /// directory, which makes the volume invalid. Section 8.2.
    UnrecognisedCriticalPrimary { entry_type: u8 },

    /// A critical primary entry other than a File entry, outside the root
    /// directory, which makes that directory invalid. Section 8.2.
    CriticalPrimaryOutsideRoot { entry_type: u8 },
}

impl fmt::Display for ExFatDirectoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Evidence(e) => write!(f, "{e}"),
            Self::ClusterOutOfRange { cluster } => {
                write!(f, "first cluster {cluster} is outside the cluster heap")
            }
            Self::LinkOutOfRange { from, to } => {
                write!(f, "cluster {from} links to {to}, outside the cluster heap")
            }
            Self::BadCluster { cluster } => write!(f, "chain reaches bad cluster {cluster}"),
            Self::ClusterAlreadyRead { cluster } => {
                write!(f, "cluster {cluster} was already read as a directory")
            }
            Self::ChainEndsEarly { cluster } => {
                write!(
                    f,
                    "chain ends at cluster {cluster}, short of the directory's length"
                )
            }
            Self::ChainLongerThanData { cluster } => {
                write!(
                    f,
                    "chain goes on past cluster {cluster}, the last the directory's length needs"
                )
            }
            Self::ClusterWithoutData { cluster } => {
                write!(f, "first cluster {cluster} with a data length of 0")
            }
            Self::ChainTooLong { limit_bytes } => {
                write!(f, "chain spans more than {limit_bytes} bytes")
            }
            Self::ClusterCapReached { cap } => {
                write!(f, "volume has more than {cap} directory clusters")
            }
            Self::FatOutOfRange { cluster } => {
                write!(f, "the FAT does not hold an entry for cluster {cluster}")
            }
            Self::UnrecognisedCriticalPrimary { entry_type } => write!(
                f,
                "critical primary entry {entry_type:#04x} is not recognised"
            ),
            Self::CriticalPrimaryOutsideRoot { entry_type } => write!(
                f,
                "critical primary entry {entry_type:#04x} outside the root directory"
            ),
        }
    }
}

impl std::error::Error for ExFatDirectoryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Evidence(e) => Some(e),
            _ => None,
        }
    }
}

impl From<Error> for ExFatDirectoryError {
    fn from(e: Error) -> Self {
        Self::Evidence(e)
    }
}

/// Why a live file's content could not be read.
///
/// Every error but `Evidence` is found before any byte of the content is
/// handed over.
#[derive(Debug)]
pub enum ExFatContentError {
    /// The evidence could not be read.
    Evidence(Error),

    /// The entry is a directory: its data is entries, not content.
    Directory,

    /// The entry's allocation fails the checks a listing applies to it.
    Allocation(SetRefusal),

    /// A FAT entry links to a cluster outside the cluster heap, or to a free
    /// or reserved one.
    LinkOutOfRange { from: u32, to: u32 },

    /// The chain reaches a cluster the FAT marks bad.
    BadCluster { cluster: u32 },

    /// A FAT entry that the FAT region does not hold.
    FatOutOfRange { cluster: u32 },

    /// The chain ends at `cluster`, before the `needed` clusters
    /// `DataLength` takes.
    ChainEndsEarly { cluster: u32, needed: u64 },

    /// The chain goes on past `cluster`, the last of the `needed` clusters
    /// `DataLength` takes.
    ///
    /// The specification does not say whether a chain may be longer than
    /// its data needs. It is refused here as a choice of this project, not a
    /// rule of the specification.
    ChainLongerThanData { cluster: u32, needed: u64 },
}

impl fmt::Display for ExFatContentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Evidence(e) => write!(f, "{e}"),
            Self::Directory => f.write_str("a directory has no file content to read"),
            Self::Allocation(refusal) => write!(f, "{refusal}"),
            Self::LinkOutOfRange { from, to } => {
                write!(f, "cluster {from} links to {to}, outside the cluster heap")
            }
            Self::BadCluster { cluster } => write!(f, "chain reaches bad cluster {cluster}"),
            Self::FatOutOfRange { cluster } => {
                write!(f, "the FAT does not hold an entry for cluster {cluster}")
            }
            Self::ChainEndsEarly { cluster, needed } => write!(
                f,
                "chain ends at cluster {cluster}, short of the {needed} clusters the data length needs"
            ),
            Self::ChainLongerThanData { cluster, needed } => write!(
                f,
                "chain goes on past cluster {cluster}, the last of the {needed} the data length needs"
            ),
        }
    }
}

impl std::error::Error for ExFatContentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Evidence(e) => Some(e),
            _ => None,
        }
    }
}

impl From<Error> for ExFatContentError {
    fn from(e: Error) -> Self {
        Self::Evidence(e)
    }
}

/// Where a directory's data is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DirectorySource {
    /// The root directory: always a FAT chain, with no length of its own.
    Root { first_cluster: u32 },

    /// Any other directory, as its entry describes it.
    Subdirectory {
        first_cluster: u32,
        data_length: u64,
        no_fat_chain: bool,
    },
}

/// One thing a directory held, handed to the caller as it is read.
///
/// Items are not collected: a directory can hold millions, and the caller
/// decides what to keep.
#[derive(Debug)]
pub enum DirItem<'a> {
    /// A live file or directory.
    File {
        position: ExFatPosition,
        file: &'a ExFatFile,
    },

    /// The volume label, in the root.
    VolumeLabel {
        position: ExFatPosition,
        label: &'a [u16],
    },

    /// The allocation bitmap's entry, in the root.
    Bitmap {
        position: ExFatPosition,
        bitmap: &'a ExFatBitmapEntry,
    },

    /// An entry or set that was not used, and why.
    Refused {
        position: ExFatPosition,
        refusal: &'a SetRefusal,
    },

    /// An unused File entry: a deleted set, not examined in this milestone.
    UnusedFileSet { position: ExFatPosition },

    /// The directory held `entries` entries of a type other than zero after
    /// its end marker. Handed over once, at the end of the directory, and not
    /// at all where there were none.
    AfterEndMarker { entries: u64 },
}

/// What reading one directory found, counted.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct DirectorySummary {
    /// Entries read at the top of the directory, of every kind: an entry
    /// set counts once, however many secondaries it held.
    pub entries_read: u64,

    /// Live files, not directories.
    pub files: u64,

    /// Live directories.
    pub directories: u64,

    /// Entries and sets that were not used.
    pub refused: u64,

    /// Unused File entries.
    pub unused_file_sets: u64,

    /// Clusters read as this directory's data.
    pub clusters_read: u64,

    /// The directory ended at an end-of-directory marker, and not by running
    /// out of data.
    pub terminated: bool,

    /// Entries after the end marker whose type is not zero. The
    /// specification says there are none. They are counted, not read.
    pub entries_after_end_marker: u64,
}

/// The longest run of an entry's bytes: one slot.
type RawBytes = [u8; ENTRY_BYTES];

/// An entry as read, and where.
#[derive(Clone, Copy)]
struct RawEntry {
    bytes: RawBytes,
    position: ExFatPosition,
}

/// What a FAT entry says about its cluster.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Link {
    Next(u32),
    End,
    Bad,
    Invalid(u32),
}

/// Where a directory's walk is. Holds no reference, so the reader can be
/// borrowed while it is advanced.
struct Cursor {
    /// The directory's first cluster, entered on the first advance.
    first_cluster: u32,

    /// Clusters are contiguous and the FAT is not consulted.
    contiguous: bool,

    /// Bytes the directory may still take, where its length is known.
    data_left: Option<u64>,

    /// Clusters entered so far.
    clusters_taken: u64,

    /// The current cluster, 0 before the first.
    cluster: u32,

    /// What the FAT says about the current cluster, for a chain.
    link: Option<Link>,

    /// Bytes of the current cluster not yet read into the chunk.
    cluster_left: u64,

    /// Bytes of the current cluster already read.
    cluster_read: u64,

    /// Slot of the next entry in the current cluster.
    slot: usize,

    chunk: Vec<u8>,
    chunk_pos: usize,
    chunk_len: usize,

    /// An entry handed back, to be returned next.
    pushed: Option<RawEntry>,

    /// No more data.
    finished: bool,
}

/// Reads exFAT directories from one volume.
///
/// Holds the evidence, the geometry, the FAT cache and the set of clusters
/// already read as directories, which spans every directory read through it.
pub struct ExFatReader<'a, R: EvidenceReader> {
    reader: &'a mut R,
    boot: &'a ExFatBootSector,
    volume_start: u64,
    limits: Limits,
    seen: HashSet<u32>,

    /// The one FAT sector held, and its index within the FAT.
    fat_sector: Vec<u8>,
    fat_sector_index: Option<u64>,

    /// The first up-case table entry the root held, and how many it held.
    /// Kept for the bitmap check, which needs the table's clusters.
    up_case: Option<UpCaseEntry>,
    up_case_entries: usize,

    /// Every live file's and directory's allocation, for the bitmap check,
    /// up to `limits.live_allocations`; and how many more were not kept.
    live: Vec<LiveAllocation>,
    live_unrecorded: u64,
}

impl<'a, R: EvidenceReader> ExFatReader<'a, R> {
    /// A reader for the volume `boot` describes, in the partition `extent`.
    pub fn new(
        reader: &'a mut R,
        boot: &'a ExFatBootSector,
        extent: VolumeExtent,
        limits: Limits,
    ) -> Self {
        Self {
            reader,
            boot,
            volume_start: extent.start_lba as u64 * SECTOR_SIZE as u64,
            limits,
            seen: HashSet::new(),
            // At most 4096 bytes: `bytes_per_sector_shift` is at most 12.
            fat_sector: vec![0u8; boot.bytes_per_sector() as usize],
            fat_sector_index: None,
            up_case: None,
            up_case_entries: 0,
            live: Vec::new(),
            live_unrecorded: 0,
        }
    }

    /// Clusters read as directories so far, over every directory.
    pub fn clusters_read(&self) -> usize {
        self.seen.len()
    }

    /// The limits this reader was given.
    pub const fn limits(&self) -> Limits {
        self.limits
    }

    /// Reads one directory, handing each thing in it to `visit`.
    ///
    /// Returns what was counted. An error stops this directory only, and
    /// everything already handed to `visit` stands; the caller decides what a
    /// partly read directory means.
    pub fn read_directory(
        &mut self,
        source: DirectorySource,
        visit: &mut dyn FnMut(DirItem<'_>),
    ) -> Result<DirectorySummary, ExFatDirectoryError> {
        // Section 7.6.7: a directory's length is its whole allocation, so a
        // cluster with no length is allocation that would go unread.
        if let DirectorySource::Subdirectory {
            first_cluster,
            data_length: 0,
            ..
        } = source
            && first_cluster != 0
        {
            return Err(ExFatDirectoryError::ClusterWithoutData {
                cluster: first_cluster,
            });
        }

        let is_root = matches!(source, DirectorySource::Root { .. });
        let mut cursor = self.cursor_for(source);
        let mut summary = DirectorySummary::default();

        // One set at a time, and never more than a set holds.
        let mut window: Vec<u8> = Vec::with_capacity(MAX_SET_ENTRIES * ENTRY_BYTES);
        let mut bitmaps = 0usize;

        let result = self.read_entries(
            &mut cursor,
            is_root,
            &mut window,
            &mut bitmaps,
            &mut summary,
            visit,
        );
        summary.clusters_read = cursor.clusters_taken;
        result.map(|()| summary)
    }

    /// Reads a live file's content, handing it to `visit` in order, in
    /// chunks of at most [`CONTENT_CHUNK_BYTES`].
    ///
    /// Bytes up to `ValidDataLength` are read from the file's clusters, each
    /// cluster at most once. The rest, up to `DataLength`, are handed over
    /// as zeros and never read: ADR-0019 Decision F, specification section
    /// 7.6.5. A chain is followed through the whole FAT before any data is
    /// read, so a malformed chain is refused with nothing handed over.
    /// Memory held is one chunk, whatever the file's size.
    ///
    /// An evidence error part way through leaves what was handed over
    /// incomplete; the caller decides what that means.
    pub fn read_content(
        &mut self,
        file: &ExFatFile,
        visit: &mut dyn FnMut(&[u8]),
    ) -> Result<(), ExFatContentError> {
        if file.is_directory() {
            return Err(ExFatContentError::Directory);
        }
        // The checks a listing applied, again, because the fields are
        // public: every cluster below is in the heap, and the lengths agree.
        self.check_allocation(
            false,
            file.first_cluster,
            file.data_length,
            file.valid_data_length,
            file.no_fat_chain,
        )
        .map_err(ExFatContentError::Allocation)?;

        let cluster_bytes = self.boot.cluster_bytes() as u64;
        let needed = file.data_length.div_ceil(cluster_bytes);
        if needed == 0 {
            return Ok(());
        }
        if !file.no_fat_chain {
            self.check_chain(file.first_cluster, needed)?;
        }

        let mut chunk = vec![0u8; file.data_length.min(CONTENT_CHUNK_BYTES as u64) as usize];

        let mut cluster = file.first_cluster;
        let mut valid_left = file.valid_data_length;
        while valid_left > 0 {
            let in_cluster = valid_left.min(cluster_bytes);
            let mut offset = self.cluster_offset(cluster);
            let mut cluster_left = in_cluster;
            while cluster_left > 0 {
                let take = cluster_left.min(chunk.len() as u64) as usize;
                self.reader.read_exact_at(offset, &mut chunk[..take])?;
                visit(&chunk[..take]);
                offset += take as u64;
                cluster_left -= take as u64;
            }

            valid_left -= in_cluster;
            if valid_left > 0 {
                cluster = if file.no_fat_chain {
                    cluster + 1
                } else {
                    match self.content_link(cluster)? {
                        Link::Next(next) => next,
                        _ => return Err(ExFatContentError::ChainEndsEarly { cluster, needed }),
                    }
                };
            }
        }

        let mut zeros_left = file.data_length - file.valid_data_length;
        chunk.fill(0);
        while zeros_left > 0 {
            let take = zeros_left.min(chunk.len() as u64) as usize;
            visit(&chunk[..take]);
            zeros_left -= take as u64;
        }

        Ok(())
    }

    /// Follows a file's chain from `first` through the FAT: links 1 to
    /// `needed - 1` must each name a next cluster in the heap, and link
    /// `needed` must end the chain. Reads no data.
    fn check_chain(&mut self, first: u32, needed: u64) -> Result<(), ExFatContentError> {
        // No cluster repeats, with none remembered: each cluster has one
        // successor, so a repeat would loop forever and never reach
        // end-of-chain.
        let mut cluster = first;
        for _ in 1..needed {
            match self.content_link(cluster)? {
                Link::Next(next) => cluster = next,
                _ => return Err(ExFatContentError::ChainEndsEarly { cluster, needed }),
            }
        }
        match self.content_link(cluster)? {
            Link::End => Ok(()),
            _ => Err(ExFatContentError::ChainLongerThanData { cluster, needed }),
        }
    }

    /// Locates the allocation bitmap's clusters through its FAT chain.
    ///
    /// The entry is checked again, because its fields are public. The chain
    /// is held to the rules content is: exactly `ceil(DataLength / cluster
    /// size)` clusters, each link but the last a next cluster in the heap,
    /// the last the end of the chain (ADR-0022 Decision E). Reads the FAT,
    /// and no bitmap data.
    pub fn open_bitmap(
        &mut self,
        entry: &ExFatBitmapEntry,
    ) -> Result<ExFatBitmap, ExFatBitmapError> {
        let cluster_bytes = self.boot.cluster_bytes() as u64;
        let last_cluster = self.boot.cluster_count as u64 + 1;
        if entry.first_cluster < 2 || entry.first_cluster as u64 > last_cluster {
            return Err(ExFatBitmapError::FirstClusterOutOfRange {
                cluster: entry.first_cluster,
            });
        }
        let needed_bytes = (self.boot.cluster_count as u64).div_ceil(8);
        if entry.data_length < needed_bytes {
            return Err(ExFatBitmapError::TooShort {
                data_length: entry.data_length,
                needed: needed_bytes,
            });
        }
        // Bounds the walk below, and the runs it holds, by the bitmap the
        // volume needs: at most 2^20 clusters, whatever the evidence's size.
        // Without it a crafted length would have a cycle in the FAT followed
        // for that many links, and a fragmented chain held a run per link.
        let limit = needed_bytes.div_ceil(cluster_bytes) * cluster_bytes;
        if entry.data_length > limit {
            return Err(ExFatBitmapError::LongerThanVolumeNeeds {
                data_length: entry.data_length,
                limit,
            });
        }

        let needed = entry.data_length.div_ceil(cluster_bytes);
        let runs = self.structure_runs(entry.first_cluster, needed)?;

        Ok(ExFatBitmap {
            runs,
            sector: vec![0u8; self.boot.bytes_per_sector() as usize],
            sector_index: None,
            volume: self.bitmap_volume(),
        })
    }

    /// Follows a structure's chain from `first` through the FAT: exactly
    /// `needed` clusters, each link but the last a next cluster in the heap,
    /// the last the end of the chain. Returns the clusters as runs, in chain
    /// order. `first` is in the heap, and `needed` is at least 1 and at most
    /// ClusterCount for the bitmap or 256 for the up-case table
    /// ([`MAX_UP_CASE_BYTES`] over the smallest cluster).
    fn structure_runs(&mut self, first: u32, needed: u64) -> Result<Vec<BitmapRun>, ChainFault> {
        let mut runs: Vec<BitmapRun> = Vec::new();
        let mut cluster = first;
        for index in 0..needed {
            match runs.last_mut() {
                Some(run) if run.first as u64 + run.clusters as u64 == cluster as u64 => {
                    run.clusters += 1;
                }
                // `needed` is at most ClusterCount or 256, so the index fits.
                _ => runs.push(BitmapRun {
                    start: index as u32,
                    first: cluster,
                    clusters: 1,
                }),
            }
            let link = match self.fat_link(cluster)? {
                None => return Err(ChainFault::FatOutOfRange { cluster }),
                Some(Link::Bad) => return Err(ChainFault::BadCluster { cluster }),
                Some(Link::Invalid(to)) => {
                    return Err(ChainFault::LinkOutOfRange { from: cluster, to });
                }
                Some(link) => link,
            };
            match (link, index + 1 == needed) {
                (Link::End, true) => {}
                (Link::Next(next), false) => cluster = next,
                (Link::End, false) => return Err(ChainFault::EndsEarly { cluster, needed }),
                _ => return Err(ChainFault::LongerThanData { cluster, needed }),
            }
        }
        Ok(runs)
    }

    /// Checks the allocation bitmap against what the walk knows is in use
    /// (ADR-0022 Decision E, as its Appendix A extends it): every cluster
    /// read as the root or as a directory, every cluster of the bitmap and
    /// of the up-case table, and every cluster of each live file's and
    /// directory's allocation must be marked in use. A crafted bitmap that
    /// marks structure free (the class of CVE-2025-40307) is not believed,
    /// and neither is one that marks a live file's data free, over which a
    /// deleted file would otherwise pass condition 1.
    ///
    /// A live allocation is followed as its set describes it: a run where
    /// `NoFatChain` is set, otherwise the chain, until it has as many
    /// clusters as `DataLength` needs or the FAT does not go on. Every
    /// cluster reached counts. Live sets past [`Limits::live_allocations`],
    /// or more clusters than [`LIVE_CLUSTERS_PER_CLUSTER`] times the
    /// volume's, leave the bitmap unbelieved, because what was not checked
    /// could be marked free.
    ///
    /// Call it once, after the walk of live directories and before any
    /// deleted directory is read: a deleted directory is read only where its
    /// clusters are free (ADR-0022 Decision K), so once one is read its
    /// clusters are among those read, and this check would fail on them.
    /// Call it only where the root was read whole: a root read part way may
    /// not have reached the up-case entry, which would then be refused as
    /// missing.
    ///
    /// It checks what the walk recorded, and nothing else. A live set in a
    /// directory that was not read, or past the point where a read stopped,
    /// is not recorded, so its clusters are not checked. A directory that was
    /// not read has its own allocation checked, not its children's. The
    /// caller must account for every directory not read whole.
    ///
    /// A directory cluster counts once it was taken as the next cluster of a
    /// directory, even where reading it then failed: the FAT marking it bad,
    /// or its FAT entry unreadable. Section 7.1.5.1 describes a cluster the
    /// FAT marks bad as not available for allocation, which is what a set
    /// bit says.
    ///
    /// The up-case table is located through its FAT chain (section 7.2.3),
    /// held to the rules the bitmap's chain is, and to
    /// [`MAX_UP_CASE_BYTES`]. Its contents are not read, so its
    /// `TableChecksum`, which section 7.2.2 requires before the table is
    /// used, is not checked: the table is not used.
    pub fn check_bitmap(
        &mut self,
        bitmap: &mut ExFatBitmap,
    ) -> Result<ExFatStructureChecked, ExFatStructureError> {
        if bitmap.volume != self.bitmap_volume() {
            return Err(ExFatBitmapError::OtherVolume.into());
        }
        let up_case = match (self.up_case, self.up_case_entries) {
            (Some(entry), 1) => entry,
            (None, _) => return Err(ExFatStructureError::NoUpCaseTable),
            (Some(_), entries) => return Err(ExFatStructureError::UpCaseTables { entries }),
        };
        if self.live_unrecorded > 0 {
            return Err(ExFatStructureError::LiveAllocationsUnrecorded {
                bound: self.limits.live_allocations,
                unrecorded: self.live_unrecorded,
            });
        }
        let up_case_runs = self.up_case_runs(up_case)?;

        // The lowest free cluster, whatever order the clusters are visited
        // in, so the error does not depend on how the set is hashed.
        let mut lowest: Option<(u32, ExFatStructure)> = None;
        let mut check = |reader: &mut Self,
                         bitmap: &mut ExFatBitmap,
                         cluster: u32,
                         structure: ExFatStructure|
         -> Result<(), ExFatBitmapError> {
            if !reader.cluster_allocated(bitmap, cluster)?
                && lowest.is_none_or(|l| (cluster, structure) < l)
            {
                lowest = Some((cluster, structure));
            }
            Ok(())
        };

        for index in 0..bitmap.runs.len() {
            let run = bitmap.runs[index];
            for cluster in run.first..run.first + run.clusters {
                check(self, bitmap, cluster, ExFatStructure::Bitmap)?;
            }
        }
        for run in &up_case_runs {
            for cluster in run.first..run.first + run.clusters {
                check(self, bitmap, cluster, ExFatStructure::UpCaseTable)?;
            }
        }
        // Copied, because each lookup borrows the reader: at most
        // `directory_clusters` of them, 4 MiB under the default limits.
        let directories: Vec<u32> = self.seen.iter().copied().collect();
        for &cluster in &directories {
            check(self, bitmap, cluster, ExFatStructure::Directory)?;
        }

        let limit = LIVE_CLUSTERS_PER_CLUSTER * self.boot.cluster_count as u64;
        let mut live_clusters = 0u64;
        for index in 0..self.live.len() {
            let live = self.live[index];
            let structure = if live.directory {
                ExFatStructure::Directory
            } else {
                ExFatStructure::File
            };
            let mut cluster = live.first_cluster;
            for taken in 1..=live.clusters {
                live_clusters += 1;
                if live_clusters > limit {
                    return Err(ExFatStructureError::LiveClustersBeyondBound { limit });
                }
                check(self, bitmap, cluster, structure)?;
                if taken == live.clusters {
                    break;
                }
                cluster = if live.no_fat_chain {
                    // In the heap: validate_file_set checked the run.
                    cluster + 1
                } else {
                    match self.fat_link(cluster)? {
                        Some(Link::Next(next)) => next,
                        // The chain does not go on: what it reached counts.
                        _ => break,
                    }
                };
            }
        }

        if let Some((cluster, structure)) = lowest {
            return Err(ExFatStructureError::MarkedFree { structure, cluster });
        }
        Ok(ExFatStructureChecked {
            directory_clusters: directories.len() as u64,
            bitmap_clusters: bitmap.clusters(),
            up_case_clusters: up_case_runs.iter().map(|r| r.clusters as u64).sum(),
            live_clusters,
        })
    }

    /// Locates the up-case table's clusters through its FAT chain.
    fn up_case_runs(&mut self, entry: UpCaseEntry) -> Result<Vec<BitmapRun>, ExFatStructureError> {
        if entry.first_cluster < 2
            || entry.first_cluster as u64 > self.boot.cluster_count as u64 + 1
        {
            return Err(ExFatStructureError::UpCaseFirstClusterOutOfRange {
                cluster: entry.first_cluster,
            });
        }
        if entry.data_length == 0 {
            return Err(ExFatStructureError::UpCaseEmpty);
        }
        if entry.data_length > MAX_UP_CASE_BYTES {
            return Err(ExFatStructureError::UpCaseTooLong {
                data_length: entry.data_length,
                limit: MAX_UP_CASE_BYTES,
            });
        }
        let needed = entry.data_length.div_ceil(self.boot.cluster_bytes() as u64);
        Ok(self.structure_runs(entry.first_cluster, needed)?)
    }

    /// The volume this reader reads, as a bitmap records it.
    fn bitmap_volume(&self) -> BitmapVolume {
        BitmapVolume {
            volume_start: self.volume_start,
            bytes_per_sector_shift: self.boot.bytes_per_sector_shift,
            sectors_per_cluster_shift: self.boot.sectors_per_cluster_shift,
            cluster_heap_offset: self.boot.cluster_heap_offset,
            cluster_count: self.boot.cluster_count,
        }
    }

    /// Whether the bitmap marks `cluster` in use: bit `cluster - 2`, the
    /// lowest-order bit of a byte first (section 7.1.5). Reads at most one
    /// sector of the bitmap, and none where the sector is the one held.
    pub fn cluster_allocated(
        &mut self,
        bitmap: &mut ExFatBitmap,
        cluster: u32,
    ) -> Result<bool, ExFatBitmapError> {
        if bitmap.volume != self.bitmap_volume() {
            return Err(ExFatBitmapError::OtherVolume);
        }
        if cluster < 2 || cluster as u64 > self.boot.cluster_count as u64 + 1 {
            return Err(ExFatBitmapError::ClusterOutOfRange { cluster });
        }
        let bit = (cluster - 2) as u64;
        let byte = bit >> 3;
        let shift = self.boot.bytes_per_sector_shift;
        let sector_index = byte >> shift;

        if bitmap.sector_index != Some(sector_index) {
            // Cleared first: a failed read must not leave the old index
            // naming bytes that are no longer there.
            bitmap.sector_index = None;
            let cluster_bytes = self.boot.cluster_bytes() as u64;
            // Below the chain's length: `byte` is under ceil(ClusterCount /
            // 8), which open_bitmap held DataLength to.
            let chain_index = (byte / cluster_bytes) as u32;
            let run = bitmap.runs[bitmap.runs.partition_point(|r| r.start <= chain_index) - 1];
            let held = run.first + (chain_index - run.start);
            let within = (byte % cluster_bytes) & !((1u64 << shift) - 1);
            let offset = self.cluster_offset(held) + within;
            self.reader.read_exact_at(offset, &mut bitmap.sector)?;
            bitmap.sector_index = Some(sector_index);
        }

        let in_sector = (byte & ((1u64 << shift) - 1)) as usize;
        Ok((bitmap.sector[in_sector] >> (bit & 7)) & 1 == 1)
    }

    /// What the FAT says follows `cluster` in a file's chain: the next
    /// cluster, or the end. Anything else is an error naming the cluster.
    fn content_link(&mut self, cluster: u32) -> Result<Link, ExFatContentError> {
        match self.fat_link(cluster)? {
            None => Err(ExFatContentError::FatOutOfRange { cluster }),
            Some(Link::Bad) => Err(ExFatContentError::BadCluster { cluster }),
            Some(Link::Invalid(to)) => Err(ExFatContentError::LinkOutOfRange { from: cluster, to }),
            Some(link) => Ok(link),
        }
    }

    fn cursor_for(&self, source: DirectorySource) -> Cursor {
        let (first_cluster, contiguous, data_left) = match source {
            DirectorySource::Root { first_cluster } => (first_cluster, false, None),
            DirectorySource::Subdirectory {
                first_cluster,
                data_length,
                no_fat_chain,
            } => (first_cluster, no_fat_chain, Some(data_length)),
        };

        Cursor {
            first_cluster,
            contiguous,
            data_left,
            clusters_taken: 0,
            cluster: 0,
            link: None,
            cluster_left: 0,
            cluster_read: 0,
            slot: 0,
            chunk: vec![0u8; READ_CHUNK_BYTES],
            chunk_pos: 0,
            chunk_len: 0,
            pushed: None,
            // A directory of no data has no clusters to enter.
            finished: data_left == Some(0),
        }
    }

    fn read_entries(
        &mut self,
        cursor: &mut Cursor,
        is_root: bool,
        window: &mut Vec<u8>,
        bitmaps: &mut usize,
        summary: &mut DirectorySummary,
        visit: &mut dyn FnMut(DirItem<'_>),
    ) -> Result<(), ExFatDirectoryError> {
        fn refuse(
            visit: &mut dyn FnMut(DirItem<'_>),
            summary: &mut DirectorySummary,
            position: ExFatPosition,
            refusal: SetRefusal,
        ) {
            summary.refused += 1;
            visit(DirItem::Refused {
                position,
                refusal: &refusal,
            });
        }

        loop {
            let Some(entry) = self.next_entry(cursor)? else {
                return Ok(());
            };
            summary.entries_read += 1;
            let kind = entry.bytes[0];
            let position = entry.position;

            if kind == TYPE_END_OF_DIRECTORY {
                summary.terminated = true;

                // Section 6.2.1: every entry after the marker is a marker too.
                // The rest of the directory's data is read to see whether
                // that holds, within the same limits as the rest, because a
                // directory read as complete while its tail was skipped would
                // overstate what was analysed. Nothing there is interpreted.
                let mut after = 0u64;
                while let Some(rest) = self.next_entry(cursor)? {
                    if rest.bytes[0] != TYPE_END_OF_DIRECTORY {
                        after += 1;
                    }
                }
                if after > 0 {
                    summary.entries_after_end_marker = after;
                    visit(DirItem::AfterEndMarker { entries: after });
                }
                return Ok(());
            }

            if kind & TYPE_IN_USE == 0 {
                // Unused: skipped, its secondaries included. Only a File
                // entry is counted, because that is a deleted set.
                if kind == TYPE_UNUSED_FILE {
                    summary.unused_file_sets += 1;
                    visit(DirItem::UnusedFileSet { position });
                }
                continue;
            }

            if kind == TYPE_INVALID {
                let refusal = SetRefusal::InvalidEntryType { entry_type: kind };
                refuse(visit, summary, position, refusal);
                continue;
            }

            if kind & TYPE_SECONDARY != 0 {
                let refusal = SetRefusal::StraySecondary { entry_type: kind };
                refuse(visit, summary, position, refusal);
                continue;
            }

            if kind & TYPE_BENIGN != 0 {
                // A benign primary: recognised and ignored if it is the
                // volume GUID, unrecognised and refused otherwise. Its
                // secondaries are consumed with it either way.
                let consumed = self.collect_set(cursor, entry, window)?;
                if let Err(refusal) = consumed {
                    refuse(visit, summary, position, refusal);
                } else if kind != TYPE_VOLUME_GUID {
                    let refusal = SetRefusal::UnrecognisedBenignPrimary { entry_type: kind };
                    refuse(visit, summary, position, refusal);
                }
                continue;
            }

            // A critical primary.
            match kind {
                TYPE_FILE => match self.read_file_set(cursor, entry, window)? {
                    Ok(file) => {
                        self.record_live(&file);
                        if file.is_directory() {
                            summary.directories += 1;
                        } else {
                            summary.files += 1;
                        }
                        visit(DirItem::File {
                            position,
                            file: &file,
                        });
                    }
                    Err(refusal) => refuse(visit, summary, position, refusal),
                },
                TYPE_BITMAP | TYPE_UP_CASE | TYPE_VOLUME_LABEL if !is_root => {
                    return Err(ExFatDirectoryError::CriticalPrimaryOutsideRoot {
                        entry_type: kind,
                    });
                }
                TYPE_BITMAP => match self.read_bitmap(&entry.bytes, bitmaps) {
                    Ok(bitmap) => visit(DirItem::Bitmap {
                        position,
                        bitmap: &bitmap,
                    }),
                    Err(refusal) => refuse(visit, summary, position, refusal),
                },
                TYPE_VOLUME_LABEL => match volume_label(&entry.bytes) {
                    Ok(label) => visit(DirItem::VolumeLabel {
                        position,
                        label: &label,
                    }),
                    Err(refusal) => refuse(visit, summary, position, refusal),
                },
                // The table itself only matters to a lookup by name, which
                // is not made. Where it lies is kept for the bitmap check;
                // whether there is exactly one is that check's to decide.
                TYPE_UP_CASE => {
                    self.up_case_entries += 1;
                    if self.up_case.is_none() {
                        self.up_case = Some(UpCaseEntry {
                            first_cluster: le_u32(&entry.bytes, 20),
                            data_length: le_u64(&entry.bytes, 24),
                        });
                    }
                }
                _ if is_root => {
                    return Err(ExFatDirectoryError::UnrecognisedCriticalPrimary {
                        entry_type: kind,
                    });
                }
                _ => {
                    return Err(ExFatDirectoryError::CriticalPrimaryOutsideRoot {
                        entry_type: kind,
                    });
                }
            }
        }
    }

    /// Reads the secondaries a primary declares into `window`, after the
    /// primary. Returns what stopped it where it could not.
    ///
    /// An entry that is not a secondary in use is handed back, not consumed,
    /// so a wrong count cannot swallow the sets after it.
    fn collect_set(
        &mut self,
        cursor: &mut Cursor,
        primary: RawEntry,
        window: &mut Vec<u8>,
    ) -> Result<Result<(), SetRefusal>, ExFatDirectoryError> {
        window.clear();
        window.extend_from_slice(&primary.bytes);

        let declared = primary.bytes[1] as usize;
        for found in 0..declared {
            match self.next_entry(cursor)? {
                Some(entry)
                    if entry.bytes[0] & (TYPE_IN_USE | TYPE_SECONDARY)
                        == (TYPE_IN_USE | TYPE_SECONDARY) =>
                {
                    window.extend_from_slice(&entry.bytes);
                }
                Some(entry) => {
                    cursor.pushed = Some(entry);
                    return Ok(Err(SetRefusal::SetTruncated { declared, found }));
                }
                None => return Ok(Err(SetRefusal::SetTruncated { declared, found })),
            }
        }

        Ok(Ok(()))
    }

    /// Keeps a live set's allocation for the bitmap check. A set with no
    /// data has none.
    fn record_live(&mut self, file: &ExFatFile) {
        if file.first_cluster == 0 || file.data_length == 0 {
            return;
        }
        if self.live.len() >= self.limits.live_allocations {
            self.live_unrecorded += 1;
            return;
        }
        // At most ClusterCount clusters: validate_file_set refused a longer
        // DataLength, so the count fits.
        let clusters = file.data_length.div_ceil(self.boot.cluster_bytes() as u64) as u32;
        self.live.push(LiveAllocation {
            first_cluster: file.first_cluster,
            clusters,
            no_fat_chain: file.no_fat_chain,
            directory: file.is_directory(),
        });
    }

    /// Reads and validates a File entry set.
    fn read_file_set(
        &mut self,
        cursor: &mut Cursor,
        primary: RawEntry,
        window: &mut Vec<u8>,
    ) -> Result<Result<ExFatFile, SetRefusal>, ExFatDirectoryError> {
        let declared = primary.bytes[1];
        if (declared as usize) < MIN_FILE_SECONDARIES {
            return Ok(Err(SetRefusal::SecondaryCountTooSmall { count: declared }));
        }

        if let Err(refusal) = self.collect_set(cursor, primary, window)? {
            return Ok(Err(refusal));
        }

        Ok(self.validate_file_set(window))
    }

    /// Validates a whole File set held in `set`: the primary and every
    /// secondary it declared. Performs no I/O.
    fn validate_file_set(&self, set: &[u8]) -> Result<ExFatFile, SetRefusal> {
        // The checksum first: no other byte of the set is used until it is
        // known to be the set the writer wrote. Section 6.3.3.
        let stored = le_u16(set, 2);
        let computed = entry_set_checksum(set);
        if stored != computed {
            return Err(SetRefusal::ChecksumMismatch { stored, computed });
        }

        let entry = |index: usize| &set[index * ENTRY_BYTES..(index + 1) * ENTRY_BYTES];
        let entries = set.len() / ENTRY_BYTES;

        let stream = entry(1);
        if stream[0] != TYPE_STREAM {
            return Err(SetRefusal::StreamExtensionMissing { found: stream[0] });
        }
        if stream[1] & FLAG_ALLOCATION_POSSIBLE == 0 {
            return Err(SetRefusal::AllocationNotPossible);
        }

        let name_length = stream[3];
        if name_length == 0 {
            return Err(SetRefusal::NameLengthOutOfRange {
                length: name_length,
            });
        }

        // Exactly the names `NameLength` needs, however many entries the set
        // declared: a count that is too small, or too large, is refused, and
        // no name is read past the entries that hold it.
        let expected = (name_length as usize).div_ceil(NAME_UNITS_PER_ENTRY);
        let found = (2..entries)
            .take_while(|&index| entry(index)[0] == TYPE_FILE_NAME)
            .count();
        if found != expected {
            return Err(SetRefusal::NameEntries { expected, found });
        }

        // Whatever follows the names must be ignorable. Section 6.4.1.2.
        for index in 2 + found..entries {
            let kind = entry(index)[0];
            if kind & BENIGN_SECONDARY_MASK != BENIGN_SECONDARY_MASK {
                return Err(SetRefusal::UnrecognisedCriticalSecondary { entry_type: kind });
            }
        }

        let mut name = Vec::with_capacity(name_length as usize);
        'names: for index in 2..2 + found {
            for pair in entry(index)[2..].as_chunks::<2>().0 {
                if name.len() == name_length as usize {
                    break 'names;
                }
                name.push(u16::from_le_bytes(*pair));
            }
        }

        let primary = entry(0);
        let attributes = le_u16(primary, 4);
        let valid_data_length = le_u64(stream, 8);
        let first_cluster = le_u32(stream, 20);
        let data_length = le_u64(stream, 24);
        let no_fat_chain = stream[1] & FLAG_NO_FAT_CHAIN != 0;

        self.check_allocation(
            attributes & ATTRIBUTE_DIRECTORY != 0,
            first_cluster,
            data_length,
            valid_data_length,
            no_fat_chain,
        )?;

        Ok(ExFatFile {
            name,
            attributes,
            first_cluster,
            data_length,
            valid_data_length,
            no_fat_chain,
        })
    }

    /// The allocation rules of sections 6.2 to 6.4 and 7.6, for one stream.
    fn check_allocation(
        &self,
        directory: bool,
        first_cluster: u32,
        data_length: u64,
        valid_data_length: u64,
        no_fat_chain: bool,
    ) -> Result<(), SetRefusal> {
        let cluster_bytes = self.boot.cluster_bytes() as u64;
        let last_cluster = self.boot.cluster_count as u64 + 1;

        if valid_data_length > data_length {
            return Err(SetRefusal::ValidDataLengthExceedsDataLength {
                valid_data_length,
                data_length,
            });
        }
        if directory {
            if data_length > MAX_DIRECTORY_BYTES {
                return Err(SetRefusal::DirectoryTooLarge { data_length });
            }
            if valid_data_length != data_length {
                return Err(SetRefusal::DirectoryValidDataLength {
                    valid_data_length,
                    data_length,
                });
            }
        }
        // At most 2^32 clusters of at most 2^25 bytes: no overflow.
        if data_length > self.boot.cluster_count as u64 * cluster_bytes {
            return Err(SetRefusal::DataLengthTooLarge { data_length });
        }

        if first_cluster == 0 {
            if data_length != 0 {
                return Err(SetRefusal::DataWithoutCluster { data_length });
            }
            if no_fat_chain {
                return Err(SetRefusal::NoFatChainWithoutCluster);
            }
            return Ok(());
        }
        if first_cluster < 2 || first_cluster as u64 > last_cluster {
            return Err(SetRefusal::FirstClusterOutOfRange {
                cluster: first_cluster,
            });
        }

        if no_fat_chain {
            if data_length == 0 {
                return Err(SetRefusal::NoFatChainWithoutData);
            }
            let clusters = data_length.div_ceil(cluster_bytes);
            if first_cluster as u64 + clusters - 1 > last_cluster {
                return Err(SetRefusal::AllocationOutOfRange {
                    first_cluster,
                    clusters,
                });
            }
        }

        Ok(())
    }

    /// The allocation bitmap's entry: located and range checked. Its
    /// contents are read only through [`Self::open_bitmap`] and
    /// [`Self::cluster_allocated`], which the walk does not call yet.
    fn read_bitmap(
        &self,
        entry: &RawBytes,
        bitmaps: &mut usize,
    ) -> Result<ExFatBitmapEntry, SetRefusal> {
        // One FAT, so the only valid bitmap is the first. Section 7.1.
        *bitmaps += 1;
        if *bitmaps > 1 {
            return Err(SetRefusal::DuplicateBitmap);
        }
        if entry[1] & 1 != 0 {
            return Err(SetRefusal::BitmapIdentifier { flags: entry[1] });
        }

        let first_cluster = le_u32(entry, 20);
        let data_length = le_u64(entry, 24);
        if first_cluster < 2 || first_cluster as u64 > self.boot.cluster_count as u64 + 1 {
            return Err(SetRefusal::BitmapFirstCluster {
                cluster: first_cluster,
            });
        }

        // A bit for every cluster. Section 7.1.5.
        let needed = (self.boot.cluster_count as u64).div_ceil(8);
        if data_length < needed {
            return Err(SetRefusal::BitmapTooShort {
                data_length,
                needed,
            });
        }

        Ok(ExFatBitmapEntry {
            first_cluster,
            data_length,
        })
    }

    // --- the stream ------------------------------------------------------

    /// The next entry of the directory, or `None` where its data ends.
    fn next_entry(&mut self, cursor: &mut Cursor) -> Result<Option<RawEntry>, ExFatDirectoryError> {
        if let Some(entry) = cursor.pushed.take() {
            return Ok(Some(entry));
        }

        loop {
            if cursor.chunk_pos < cursor.chunk_len {
                let mut bytes = [0u8; ENTRY_BYTES];
                bytes.copy_from_slice(
                    &cursor.chunk[cursor.chunk_pos..cursor.chunk_pos + ENTRY_BYTES],
                );
                cursor.chunk_pos += ENTRY_BYTES;

                let position = ExFatPosition {
                    cluster: cursor.cluster,
                    slot: cursor.slot,
                };
                cursor.slot += 1;
                return Ok(Some(RawEntry { bytes, position }));
            }

            if cursor.cluster_left >= ENTRY_BYTES as u64 {
                self.read_chunk(cursor)?;
                continue;
            }

            if cursor.finished || !self.enter_next_cluster(cursor)? {
                cursor.finished = true;
                return Ok(None);
            }
        }
    }

    /// Reads the next chunk of the current cluster.
    fn read_chunk(&mut self, cursor: &mut Cursor) -> Result<(), ExFatDirectoryError> {
        // Whole entries only: a length that is not a multiple of 32 leaves a
        // tail that is not an entry.
        let usable = cursor.cluster_left - cursor.cluster_left % ENTRY_BYTES as u64;
        let take = usable.min(READ_CHUNK_BYTES as u64) as usize;

        let offset = self.cluster_offset(cursor.cluster) + cursor.cluster_read;
        self.reader
            .read_exact_at(offset, &mut cursor.chunk[..take])?;

        cursor.chunk_pos = 0;
        cursor.chunk_len = take;
        cursor.cluster_read += take as u64;
        cursor.cluster_left -= take as u64;
        Ok(())
    }

    /// Moves to the directory's next cluster. Returns `false` where there is
    /// none.
    fn enter_next_cluster(&mut self, cursor: &mut Cursor) -> Result<bool, ExFatDirectoryError> {
        let cluster_bytes = self.boot.cluster_bytes() as u64;

        let next = if cursor.clusters_taken == 0 {
            cursor.first_cluster
        } else if cursor.data_left == Some(0) {
            // The length is covered, so a chain must end here: a directory's
            // length is its whole allocation (section 7.6.7), and only
            // end-of-chain may be a chain's last entry (section 4.1.3).
            if cursor.contiguous {
                return Ok(false);
            }
            return match cursor.link {
                // A chain's cluster always has its link once entered; `None`
                // is only the state before the first cluster.
                Some(Link::End) | None => Ok(false),
                Some(Link::Next(_)) => Err(ExFatDirectoryError::ChainLongerThanData {
                    cluster: cursor.cluster,
                }),
                Some(Link::Invalid(to)) => Err(ExFatDirectoryError::LinkOutOfRange {
                    from: cursor.cluster,
                    to,
                }),
                // Refused when the cluster was entered; handled, not assumed.
                Some(Link::Bad) => Err(ExFatDirectoryError::BadCluster {
                    cluster: cursor.cluster,
                }),
            };
        } else if cursor.contiguous {
            cursor.cluster + 1
        } else {
            match cursor.link {
                Some(Link::Next(next)) => next,
                Some(Link::End) | None => {
                    // The root has no length, so its chain's end is its end.
                    if cursor.data_left.is_some() {
                        return Err(ExFatDirectoryError::ChainEndsEarly {
                            cluster: cursor.cluster,
                        });
                    }
                    return Ok(false);
                }
                Some(Link::Bad) => {
                    return Err(ExFatDirectoryError::BadCluster {
                        cluster: cursor.cluster,
                    });
                }
                Some(Link::Invalid(to)) => {
                    return Err(ExFatDirectoryError::LinkOutOfRange {
                        from: cursor.cluster,
                        to,
                    });
                }
            }
        };

        if next < 2 || next as u64 > self.boot.cluster_count as u64 + 1 {
            return Err(if cursor.clusters_taken == 0 {
                ExFatDirectoryError::ClusterOutOfRange { cluster: next }
            } else {
                ExFatDirectoryError::LinkOutOfRange {
                    from: cursor.cluster,
                    to: next,
                }
            });
        }

        if (cursor.clusters_taken + 1).saturating_mul(cluster_bytes) > self.limits.directory_bytes {
            return Err(ExFatDirectoryError::ChainTooLong {
                limit_bytes: self.limits.directory_bytes,
            });
        }
        if self.seen.len() >= self.limits.directory_clusters as usize {
            return Err(ExFatDirectoryError::ClusterCapReached {
                cap: self.limits.directory_clusters,
            });
        }
        if !self.seen.insert(next) {
            return Err(ExFatDirectoryError::ClusterAlreadyRead { cluster: next });
        }

        // A chain's entry for a cluster is read before the cluster is, so a
        // bad cluster is not read as a directory. A contiguous run has no
        // FAT entries to interpret.
        cursor.link = if cursor.contiguous {
            None
        } else {
            let link = self.link_of(next)?;
            if link == Link::Bad {
                return Err(ExFatDirectoryError::BadCluster { cluster: next });
            }
            Some(link)
        };

        let take = match cursor.data_left {
            Some(left) => left.min(cluster_bytes),
            None => cluster_bytes,
        };
        if let Some(left) = cursor.data_left.as_mut() {
            *left -= take;
        }

        cursor.cluster = next;
        cursor.clusters_taken += 1;
        cursor.cluster_left = take;
        cursor.cluster_read = 0;
        cursor.slot = 0;
        cursor.chunk_pos = 0;
        cursor.chunk_len = 0;
        Ok(true)
    }

    /// Byte offset of a cluster in the evidence. The cluster is in range.
    fn cluster_offset(&self, cluster: u32) -> u64 {
        let sectors = self.boot.cluster_heap_offset as u64
            + (cluster as u64 - 2) * (1u64 << self.boot.sectors_per_cluster_shift);
        self.volume_start + (sectors << self.boot.bytes_per_sector_shift)
    }

    /// What the FAT says about `cluster`, reading its sector if it is not the
    /// one held.
    fn link_of(&mut self, cluster: u32) -> Result<Link, ExFatDirectoryError> {
        self.fat_link(cluster)?
            .ok_or(ExFatDirectoryError::FatOutOfRange { cluster })
    }

    /// What the FAT says about `cluster`, or `None` where the FAT region
    /// holds no entry for it.
    fn fat_link(&mut self, cluster: u32) -> Result<Option<Link>, Error> {
        let shift = self.boot.bytes_per_sector_shift;
        let byte = cluster as u64 * 4;
        let sector = byte >> shift;
        if sector >= self.boot.fat_length as u64 {
            return Ok(None);
        }

        if self.fat_sector_index != Some(sector) {
            // Cleared first: a failed read must not leave the old index
            // naming bytes that are no longer there.
            self.fat_sector_index = None;
            let offset = self.volume_start + ((self.boot.fat_offset as u64 + sector) << shift);
            self.reader.read_exact_at(offset, &mut self.fat_sector)?;
            self.fat_sector_index = Some(sector);
        }

        let value = le_u32(&self.fat_sector, (byte & ((1u64 << shift) - 1)) as usize);
        let last = self.boot.cluster_count as u64 + 1;
        Ok(Some(match value {
            FAT_END_OF_CHAIN => Link::End,
            FAT_BAD_CLUSTER => Link::Bad,
            v if v >= 2 && v as u64 <= last => Link::Next(v),
            v => Link::Invalid(v),
        }))
    }
}

/// A volume label entry's text, as code units. Section 7.3.
fn volume_label(entry: &RawBytes) -> Result<Vec<u16>, SetRefusal> {
    let count = entry[1];
    if count as usize > MAX_LABEL_UNITS {
        return Err(SetRefusal::LabelLength { count });
    }

    Ok(entry[2..2 + count as usize * 2]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect())
}

/// The checksum of an entry set. Section 6.3.3, Figure 2.
///
/// `set` is the primary and every secondary. Bytes 2 and 3 of the primary,
/// where the checksum is stored, are skipped.
pub fn entry_set_checksum(set: &[u8]) -> u16 {
    let mut checksum: u16 = 0;

    for (index, &byte) in set.iter().enumerate() {
        if index == 2 || index == 3 {
            continue;
        }
        let rotated_in: u16 = if checksum & 1 == 1 { 0x8000 } else { 0 };
        checksum = rotated_in
            .wrapping_add(checksum >> 1)
            .wrapping_add(byte as u16);
    }

    checksum
}

/// A name, or any UTF-16 text from the evidence, as text safe to print.
///
/// A code unit from `0x20` to `0x7E` is shown as itself, except backslash and
/// slash. Every other code unit is shown as `\u{XXXX}`: four uppercase
/// hexadecimal digits, one escape per code unit, so a surrogate pair is two.
/// Backslash and slash are escaped because both are illegal in a name
/// (section 7.7.3) and printable: left as they are, a backslash would make
/// the escapes ambiguous and a slash would make a path ambiguous. The
/// rendering can be reversed.
pub fn escaped_name(units: &[u16]) -> String {
    let mut text = String::with_capacity(units.len());

    for &unit in units {
        match unit {
            0x5C | 0x2F => text.push_str(&format!("\\u{{{unit:04X}}}")),
            0x20..=0x7E => text.push(unit as u8 as char),
            _ => text.push_str(&format!("\\u{{{unit:04X}}}")),
        }
    }

    text
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
    use crate::exfat_boot::BootRegion;

    /// Entries in one cluster of the synthetic volume.
    const PER_CLUSTER: usize = 16;

    /// Clusters in the synthetic volume's heap, numbered 2 to 201.
    const CLUSTER_COUNT: u32 = 200;

    const FAT_BYTE_OFFSET: u64 = 24 * 512;

    /// A boot sector for a volume of 512-byte clusters and 200 of them, with
    /// the FAT at sector 24 and the heap at sector 64.
    pub(crate) fn boot() -> ExFatBootSector {
        ExFatBootSector {
            region: BootRegion::Main,
            bytes_per_sector_shift: 9,
            sectors_per_cluster_shift: 0,
            volume_length: 4096,
            fat_offset: 24,
            fat_length: 8,
            cluster_heap_offset: 64,
            cluster_count: CLUSTER_COUNT,
            root_cluster: 2,
            volume_serial: 0,
            revision_minor: 0,
            volume_flags: Some(0),
            percent_in_use: Some(0),
            observations: Vec::new(),
            located_by_search: false,
        }
    }

    pub(crate) fn extent() -> VolumeExtent {
        VolumeExtent {
            start_lba: 0,
            sector_count: 4096,
        }
    }

    /// A synthetic volume: evidence and the boot sector that describes it.
    pub(crate) struct Vol {
        pub(crate) image: MemoryImage,
        pub(crate) boot: ExFatBootSector,
    }

    impl Vol {
        pub(crate) fn new() -> Self {
            Self {
                image: MemoryImage::new((64 + CLUSTER_COUNT as u64) * 512 + 4096),
                boot: boot(),
            }
        }

        fn cluster_offset(cluster: u32) -> u64 {
            (64 + (cluster as u64 - 2)) * 512
        }

        /// Writes `entries` from `slot` of `cluster`.
        pub(crate) fn put(&mut self, cluster: u32, slot: usize, entries: &[[u8; 32]]) {
            let bytes: Vec<u8> = entries.iter().flatten().copied().collect();
            self.image
                .write(Self::cluster_offset(cluster) + slot as u64 * 32, &bytes);
        }

        pub(crate) fn fat(&mut self, cluster: u32, value: u32) {
            self.image
                .write(FAT_BYTE_OFFSET + cluster as u64 * 4, &value.to_le_bytes());
        }

        /// Links `clusters` into a chain and ends it.
        pub(crate) fn chain(&mut self, clusters: &[u32]) {
            for pair in clusters.windows(2) {
                self.fat(pair[0], pair[1]);
            }
            if let Some(&last) = clusters.last() {
                self.fat(last, FAT_END_OF_CHAIN);
            }
        }

        /// Fills a cluster with unused entries that are not File entries,
        /// so a walk reads on past it without finding a terminator.
        pub(crate) fn fill_unused(&mut self, cluster: u32) {
            self.put(cluster, 0, &[[0x01; 32]; PER_CLUSTER]);
        }
    }

    pub(crate) fn utf16(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    /// What a walk handed to its visitor, owned.
    #[derive(Debug, PartialEq)]
    pub(crate) enum Seen {
        File(ExFatPosition, ExFatFile),
        Label(Vec<u16>),
        Bitmap(ExFatBitmapEntry),
        Refused(ExFatPosition, SetRefusal),
        Unused(ExFatPosition),
        AfterEndMarker(u64),
    }

    pub(crate) fn walk(
        vol: &mut Vol,
        source: DirectorySource,
        limits: Limits,
    ) -> (Result<DirectorySummary, ExFatDirectoryError>, Vec<Seen>) {
        let boot = vol.boot.clone();
        let mut reader = ExFatReader::new(&mut vol.image, &boot, extent(), limits);
        walk_with(&mut reader, source)
    }

    fn walk_with(
        reader: &mut ExFatReader<'_, MemoryImage>,
        source: DirectorySource,
    ) -> (Result<DirectorySummary, ExFatDirectoryError>, Vec<Seen>) {
        let mut seen = Vec::new();
        let result = reader.read_directory(source, &mut |item| {
            seen.push(match item {
                DirItem::File { position, file } => Seen::File(position, file.clone()),
                DirItem::VolumeLabel { label, .. } => Seen::Label(label.to_vec()),
                DirItem::Bitmap { bitmap, .. } => Seen::Bitmap(*bitmap),
                DirItem::Refused { position, refusal } => Seen::Refused(position, refusal.clone()),
                DirItem::UnusedFileSet { position } => Seen::Unused(position),
                DirItem::AfterEndMarker { entries } => Seen::AfterEndMarker(entries),
            });
        });
        (result, seen)
    }

    pub(crate) fn root(cluster: u32) -> DirectorySource {
        DirectorySource::Root {
            first_cluster: cluster,
        }
    }

    // --- entry builders ----------------------------------------------------

    pub(crate) fn seal(set: &mut [[u8; 32]]) {
        let flat: Vec<u8> = set.iter().flatten().copied().collect();
        let checksum = entry_set_checksum(&flat);
        set[0][2..4].copy_from_slice(&checksum.to_le_bytes());
    }

    fn primary(attributes: u16, secondaries: usize) -> [u8; 32] {
        let mut entry = [0u8; 32];
        entry[0] = TYPE_FILE;
        entry[1] = secondaries as u8;
        entry[4..6].copy_from_slice(&attributes.to_le_bytes());
        entry
    }

    fn stream(flags: u8, name_length: usize, vdl: u64, first: u32, dl: u64) -> [u8; 32] {
        let mut entry = [0u8; 32];
        entry[0] = TYPE_STREAM;
        entry[1] = flags;
        entry[3] = name_length as u8;
        entry[8..16].copy_from_slice(&vdl.to_le_bytes());
        entry[20..24].copy_from_slice(&first.to_le_bytes());
        entry[24..32].copy_from_slice(&dl.to_le_bytes());
        entry
    }

    fn name_entry(units: &[u16]) -> [u8; 32] {
        let mut entry = [0u8; 32];
        entry[0] = TYPE_FILE_NAME;
        for (i, unit) in units.iter().enumerate() {
            entry[2 + i * 2..4 + i * 2].copy_from_slice(&unit.to_le_bytes());
        }
        entry
    }

    /// A sealed File set: the primary, a stream and as many name entries as
    /// the name needs.
    pub(crate) fn file_set(
        name: &[u16],
        attributes: u16,
        first: u32,
        vdl: u64,
        dl: u64,
        no_fat_chain: bool,
    ) -> Vec<[u8; 32]> {
        let names = name.len().div_ceil(NAME_UNITS_PER_ENTRY).max(1);
        let flags = FLAG_ALLOCATION_POSSIBLE | if no_fat_chain { FLAG_NO_FAT_CHAIN } else { 0 };

        let mut set = vec![primary(attributes, 1 + names)];
        set.push(stream(flags, name.len(), vdl, first, dl));
        for chunk in name.chunks(NAME_UNITS_PER_ENTRY) {
            set.push(name_entry(chunk));
        }
        if name.is_empty() {
            set.push(name_entry(&[]));
        }
        seal(&mut set);
        set
    }

    /// A valid file of `size` bytes at `first`.
    pub(crate) fn file(name: &str, first: u32, size: u64) -> Vec<[u8; 32]> {
        file_set(&utf16(name), 0x20, first, size, size, false)
    }

    /// A valid directory of `clusters` clusters at `first`, as a FAT chain.
    pub(crate) fn directory(name: &str, first: u32, clusters: u64) -> Vec<[u8; 32]> {
        let size = clusters * 512;
        file_set(&utf16(name), ATTRIBUTE_DIRECTORY, first, size, size, false)
    }

    /// `set` changed by `edit` and sealed again, so only the change is wrong.
    fn mutated(mut set: Vec<[u8; 32]>, edit: impl FnOnce(&mut Vec<[u8; 32]>)) -> Vec<[u8; 32]> {
        edit(&mut set);
        seal(&mut set);
        set
    }

    /// Reads a root of one cluster holding `set` and then a valid empty file
    /// `OK`, which shows the walk went on. Returns the one refusal.
    fn refusal_of(set: Vec<[u8; 32]>) -> SetRefusal {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        let used = set.len();
        vol.put(2, 0, &set);
        vol.put(2, used, &file("OK", 0, 0));

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        result.expect("the directory reads");

        let refusals: Vec<&SetRefusal> = seen
            .iter()
            .filter_map(|s| match s {
                Seen::Refused(_, r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(refusals.len(), 1, "{seen:?}");
        assert!(
            seen.iter()
                .any(|s| matches!(s, Seen::File(_, f) if f.name == utf16("OK"))),
            "the walk did not reach the entry after the refused set: {seen:?}"
        );
        refusals[0].clone()
    }

    // --- the allocation bitmap ----------------------------------------------

    /// Sets the bit of each of `clusters` in a bitmap whose clusters are
    /// `held`, in order.
    fn mark(vol: &mut Vol, held: &[u32], clusters: &[u32]) {
        for &c in clusters {
            let byte = (c - 2) as u64 / 8;
            let at = Vol::cluster_offset(held[(byte / 512) as usize]) + byte % 512;
            let mut current = [0u8; 1];
            vol.image
                .read_exact_at(at, &mut current)
                .expect("in the image");
            vol.image.write(at, &[current[0] | 1 << ((c - 2) % 8)]);
        }
    }

    fn bitmap_at(first: u32, data_length: u64) -> ExFatBitmapEntry {
        ExFatBitmapEntry {
            first_cluster: first,
            data_length,
        }
    }

    #[test]
    fn a_bitmap_of_one_cluster_answers_for_each_cluster() {
        let mut vol = Vol::new();
        vol.chain(&[3]);
        mark(&mut vol, &[3], &[2, 9, 201]);
        let boot = vol.boot.clone();
        let mut reader = ExFatReader::new(&mut vol.image, &boot, extent(), Limits::DEFAULT);

        let mut bitmap = reader.open_bitmap(&bitmap_at(3, 25)).expect("opens");
        assert_eq!(bitmap.clusters(), 1);
        for (cluster, allocated) in [(2, true), (3, false), (9, true), (10, false), (201, true)] {
            assert_eq!(
                reader
                    .cluster_allocated(&mut bitmap, cluster)
                    .expect("reads"),
                allocated,
                "cluster {cluster}"
            );
        }
    }

    /// The bitmap's second cluster is 20, not 11: read contiguously, the
    /// answer for cluster 4200 would come from cluster 11, which holds none.
    /// Cluster 11 ends a chain of its own, so a walk that took it would open
    /// and answer wrongly rather than fail.
    #[test]
    fn a_bitmap_that_is_a_chain_is_read_through_the_fat() {
        let mut vol = Vol::new();
        vol.boot.cluster_count = 5000;
        vol.chain(&[10, 20]);
        vol.fat(11, FAT_END_OF_CHAIN);
        mark(&mut vol, &[10, 20], &[4200]);
        let boot = vol.boot.clone();
        let mut reader = ExFatReader::new(&mut vol.image, &boot, extent(), Limits::DEFAULT);

        let mut bitmap = reader.open_bitmap(&bitmap_at(10, 1024)).expect("opens");
        assert_eq!(bitmap.clusters(), 2);
        assert!(reader.cluster_allocated(&mut bitmap, 4200).expect("reads"));
        assert!(!reader.cluster_allocated(&mut bitmap, 4201).expect("reads"));
        assert!(!reader.cluster_allocated(&mut bitmap, 2).expect("reads"));
    }

    #[test]
    fn a_bitmap_chain_that_ends_early_is_refused() {
        let mut vol = Vol::new();
        vol.boot.cluster_count = 5000;
        vol.chain(&[10]);
        let boot = vol.boot.clone();
        let mut reader = ExFatReader::new(&mut vol.image, &boot, extent(), Limits::DEFAULT);
        assert!(matches!(
            reader.open_bitmap(&bitmap_at(10, 1024)),
            Err(ExFatBitmapError::ChainEndsEarly {
                cluster: 10,
                needed: 2
            })
        ));
    }

    #[test]
    fn a_bitmap_chain_longer_than_its_length_is_refused() {
        let mut vol = Vol::new();
        vol.chain(&[3, 4]);
        let boot = vol.boot.clone();
        let mut reader = ExFatReader::new(&mut vol.image, &boot, extent(), Limits::DEFAULT);
        assert!(matches!(
            reader.open_bitmap(&bitmap_at(3, 25)),
            Err(ExFatBitmapError::ChainLongerThanData {
                cluster: 3,
                needed: 1
            })
        ));
    }

    #[test]
    fn a_bitmap_chain_through_a_bad_or_out_of_heap_link_is_refused() {
        let mut vol = Vol::new();
        vol.boot.cluster_count = 5000;
        vol.fat(10, FAT_BAD_CLUSTER);
        vol.fat(30, 1);
        let boot = vol.boot.clone();
        let mut reader = ExFatReader::new(&mut vol.image, &boot, extent(), Limits::DEFAULT);
        assert!(matches!(
            reader.open_bitmap(&bitmap_at(10, 1024)),
            Err(ExFatBitmapError::BadCluster { cluster: 10 })
        ));
        assert!(matches!(
            reader.open_bitmap(&bitmap_at(30, 1024)),
            Err(ExFatBitmapError::LinkOutOfRange { from: 30, to: 1 })
        ));
    }

    /// A length past the needed bytes rounded up to a cluster is refused
    /// before the FAT is read; one exactly at that limit is not. 200
    /// clusters need 25 bytes, which round up to one 512-byte cluster.
    #[test]
    fn a_bitmap_longer_than_the_volume_needs_or_shorter_is_refused() {
        let mut vol = Vol::new();
        vol.chain(&[3]);
        let boot = vol.boot.clone();
        let mut reader = ExFatReader::new(&mut vol.image, &boot, extent(), Limits::DEFAULT);
        assert!(matches!(
            reader.open_bitmap(&bitmap_at(3, 513)),
            Err(ExFatBitmapError::LongerThanVolumeNeeds {
                data_length: 513,
                limit: 512
            })
        ));
        assert!(reader.open_bitmap(&bitmap_at(3, 512)).is_ok());
        assert!(matches!(
            reader.open_bitmap(&bitmap_at(3, 200 * 512 + 1)),
            Err(ExFatBitmapError::LongerThanVolumeNeeds {
                data_length: 102_401,
                limit: 512
            })
        ));
        assert!(matches!(
            reader.open_bitmap(&bitmap_at(3, 24)),
            Err(ExFatBitmapError::TooShort {
                data_length: 24,
                needed: 25
            })
        ));
        for first in [1, 202] {
            assert!(matches!(
                reader.open_bitmap(&bitmap_at(first, 25)),
                Err(ExFatBitmapError::FirstClusterOutOfRange { cluster }) if cluster == first
            ));
        }
    }

    #[test]
    fn a_cluster_outside_the_heap_has_no_answer() {
        let mut vol = Vol::new();
        vol.chain(&[3]);
        let boot = vol.boot.clone();
        let mut reader = ExFatReader::new(&mut vol.image, &boot, extent(), Limits::DEFAULT);
        let mut bitmap = reader.open_bitmap(&bitmap_at(3, 25)).expect("opens");
        for asked in [0, 1, 202] {
            assert!(matches!(
                reader.cluster_allocated(&mut bitmap, asked),
                Err(ExFatBitmapError::ClusterOutOfRange { cluster }) if cluster == asked
            ));
        }
    }

    /// A bitmap opened on one volume is refused by a reader of another,
    /// whose sector size differs: its buffer and runs do not fit there.
    #[test]
    fn a_bitmap_is_refused_by_a_reader_of_another_volume() {
        let mut vol = Vol::new();
        vol.chain(&[3]);
        let boot = vol.boot.clone();
        let mut bitmap = {
            let mut reader = ExFatReader::new(&mut vol.image, &boot, extent(), Limits::DEFAULT);
            reader.open_bitmap(&bitmap_at(3, 25)).expect("opens")
        };
        let mut other = boot.clone();
        other.bytes_per_sector_shift = 12;
        let mut reader = ExFatReader::new(&mut vol.image, &other, extent(), Limits::DEFAULT);
        assert!(matches!(
            reader.cluster_allocated(&mut bitmap, 2),
            Err(ExFatBitmapError::OtherVolume)
        ));
    }

    // --- the bitmap check ----------------------------------------------------

    /// An up-case table entry for a table of `data_length` bytes at `first`.
    fn up_case_at(first: u32, data_length: u64) -> [u8; 32] {
        let mut entry = [0u8; 32];
        entry[0] = TYPE_UP_CASE;
        entry[20..24].copy_from_slice(&first.to_le_bytes());
        entry[24..32].copy_from_slice(&data_length.to_le_bytes());
        entry
    }

    fn bitmap_entry_at(first: u32, data_length: u64) -> [u8; 32] {
        let mut entry = [0u8; 32];
        entry[0] = TYPE_BITMAP;
        entry[20..24].copy_from_slice(&first.to_le_bytes());
        entry[24..32].copy_from_slice(&data_length.to_le_bytes());
        entry
    }

    /// Every cluster of structure on [`structured`]'s volume.
    const STRUCTURE: [(u32, ExFatStructure); 6] = [
        (2, ExFatStructure::Directory),
        (5, ExFatStructure::Directory),
        (12, ExFatStructure::Directory),
        (3, ExFatStructure::Bitmap),
        (6, ExFatStructure::UpCaseTable),
        (9, ExFatStructure::UpCaseTable),
    ];

    /// A volume whose root is the chain 2, 5 and holds the bitmap (cluster
    /// 3), the up-case table (the chain 6, 9) and a directory (cluster 12).
    /// Cluster 7 is marked in use and is no structure's: read contiguously
    /// from 6, the up-case table would take 7 for 9.
    fn structured(root_entries: &[[u8; 32]]) -> Vol {
        let mut vol = Vol::new();
        vol.chain(&[2, 5]);
        vol.chain(&[3]);
        vol.chain(&[6, 9]);
        vol.fat(7, FAT_END_OF_CHAIN);
        vol.chain(&[12]);
        // The root's first cluster holds 16 entries; the rest go in its
        // second, 5, not in the cluster after 2, which is the bitmap.
        let (first, rest) = root_entries.split_at(root_entries.len().min(PER_CLUSTER));
        assert!(rest.len() <= PER_CLUSTER, "the root holds 32 entries");
        vol.put(2, 0, first);
        vol.put(5, 0, rest);
        mark(&mut vol, &[3], &[7]);
        vol
    }

    fn standard_root() -> Vec<[u8; 32]> {
        let mut entries = vec![bitmap_entry_at(3, 25), up_case_at(6, 1024)];
        entries.extend(directory("D", 12, 1));
        entries
    }

    /// Walks the root and directory 12, opens the bitmap and checks it.
    fn check(vol: &mut Vol) -> Result<ExFatStructureChecked, ExFatStructureError> {
        check_with(vol, Limits::DEFAULT)
    }

    fn check_with(
        vol: &mut Vol,
        limits: Limits,
    ) -> Result<ExFatStructureChecked, ExFatStructureError> {
        let boot = vol.boot.clone();
        let mut reader = ExFatReader::new(&mut vol.image, &boot, extent(), limits);
        let (result, seen) = walk_with(&mut reader, root(2));
        result.expect("the root reads");
        let _ = walk_with(
            &mut reader,
            DirectorySource::Subdirectory {
                first_cluster: 12,
                data_length: 512,
                no_fat_chain: false,
            },
        );
        let entry = seen
            .iter()
            .find_map(|s| match s {
                Seen::Bitmap(entry) => Some(*entry),
                _ => None,
            })
            .expect("a bitmap entry");
        let mut bitmap = reader.open_bitmap(&entry).expect("opens");
        reader.check_bitmap(&mut bitmap)
    }

    fn all_of(clusters: &[(u32, ExFatStructure)]) -> Vec<u32> {
        clusters.iter().map(|&(c, _)| c).collect()
    }

    #[test]
    fn a_bitmap_marking_every_structure_cluster_in_use_is_believed() {
        let mut vol = structured(&standard_root());
        mark(&mut vol, &[3], &all_of(&STRUCTURE));

        assert_eq!(
            check(&mut vol).expect("believed"),
            ExFatStructureChecked {
                directory_clusters: 3,
                bitmap_clusters: 1,
                up_case_clusters: 2,
                live_clusters: 1,
            }
        );
    }

    /// The root's second cluster, a directory's, the bitmap's own (the
    /// class of CVE-2025-40307) and each of the up-case table's: any one
    /// marked free and the bitmap is not believed. Cluster 9 free shows the
    /// up-case table is followed through the FAT: 7, which a contiguous
    /// reading would take instead, is marked in use.
    #[test]
    fn a_bitmap_marking_any_structure_cluster_free_is_not_believed() {
        for (free, kind) in STRUCTURE {
            let mut vol = structured(&standard_root());
            let others: Vec<u32> = all_of(&STRUCTURE)
                .into_iter()
                .filter(|&c| c != free)
                .collect();
            mark(&mut vol, &[3], &others);

            let result = check(&mut vol);
            assert!(
                matches!(
                    result,
                    Err(ExFatStructureError::MarkedFree { structure, cluster })
                        if structure == kind && cluster == free
                ),
                "cluster {free}: {result:?}"
            );
        }
    }

    /// The bitmap's and the up-case table's clusters are visited before any
    /// directory's, so the lowest free one, 5, is not the first found, 9.
    #[test]
    fn the_lowest_cluster_marked_free_is_named() {
        let mut vol = structured(&standard_root());
        mark(&mut vol, &[3], &[2, 3, 6]);

        let result = check(&mut vol);
        assert!(
            matches!(
                result,
                Err(ExFatStructureError::MarkedFree {
                    structure: ExFatStructure::Directory,
                    cluster: 5,
                })
            ),
            "{result:?}"
        );
    }

    /// Section 7.1.5.1: a cluster the FAT marks bad is not available for
    /// allocation. A directory's cluster stopped there still counts.
    #[test]
    fn a_directory_cluster_the_fat_marks_bad_is_still_checked() {
        let mut vol = structured(&standard_root());
        vol.fat(12, FAT_BAD_CLUSTER);
        mark(&mut vol, &[3], &[2, 5, 3, 6, 9]);

        let result = check(&mut vol);
        assert!(
            matches!(
                result,
                Err(ExFatStructureError::MarkedFree {
                    structure: ExFatStructure::Directory,
                    cluster: 12,
                })
            ),
            "{result:?}"
        );
    }

    /// The root has no set, so no live record covers its clusters: a root
    /// cluster the FAT marks bad is checked through the clusters the walk
    /// took, alone. The walk stops there, and the check still names it.
    #[test]
    fn a_root_cluster_the_fat_marks_bad_is_still_checked() {
        let mut vol = structured(&standard_root());
        vol.fat(5, FAT_BAD_CLUSTER);
        mark(&mut vol, &[3], &[2, 3, 6, 9, 12]);
        let boot = vol.boot.clone();
        let mut reader = ExFatReader::new(&mut vol.image, &boot, extent(), Limits::DEFAULT);

        let (result, seen) = walk_with(&mut reader, root(2));
        assert!(
            matches!(result, Err(ExFatDirectoryError::BadCluster { cluster: 5 })),
            "{result:?}"
        );
        let entry = seen
            .iter()
            .find_map(|s| match s {
                Seen::Bitmap(entry) => Some(*entry),
                _ => None,
            })
            .expect("a bitmap entry");
        let mut bitmap = reader.open_bitmap(&entry).expect("opens");

        let result = reader.check_bitmap(&mut bitmap);
        assert!(
            matches!(
                result,
                Err(ExFatStructureError::MarkedFree {
                    structure: ExFatStructure::Directory,
                    cluster: 5,
                })
            ),
            "{result:?}"
        );
    }

    #[test]
    fn a_root_without_one_up_case_table_entry_leaves_the_bitmap_unbelieved() {
        let mut none = vec![bitmap_entry_at(3, 25)];
        none.extend(directory("D", 12, 1));
        let mut vol = structured(&none);
        mark(&mut vol, &[3], &all_of(&STRUCTURE));
        let result = check(&mut vol);
        assert!(
            matches!(result, Err(ExFatStructureError::NoUpCaseTable)),
            "{result:?}"
        );

        let mut two = standard_root();
        two.push(up_case_at(6, 1024));
        let mut vol = structured(&two);
        mark(&mut vol, &[3], &all_of(&STRUCTURE));
        let result = check(&mut vol);
        assert!(
            matches!(
                result,
                Err(ExFatStructureError::UpCaseTables { entries: 2 })
            ),
            "{result:?}"
        );
    }

    #[test]
    fn an_up_case_table_whose_fields_cannot_be_followed_is_refused() {
        let with = |first: u32, data_length: u64| {
            let mut entries = vec![bitmap_entry_at(3, 25), up_case_at(first, data_length)];
            entries.extend(directory("D", 12, 1));
            let mut vol = structured(&entries);
            mark(&mut vol, &[3], &all_of(&STRUCTURE));
            check(&mut vol)
        };

        for first in [0, 1, 202] {
            let result = with(first, 1024);
            assert!(
                matches!(
                    result,
                    Err(ExFatStructureError::UpCaseFirstClusterOutOfRange { cluster })
                        if cluster == first
                ),
                "{first}: {result:?}"
            );
        }
        let result = with(6, 0);
        assert!(
            matches!(result, Err(ExFatStructureError::UpCaseEmpty)),
            "{result:?}"
        );
        let result = with(6, MAX_UP_CASE_BYTES + 1);
        assert!(
            matches!(
                result,
                Err(ExFatStructureError::UpCaseTooLong {
                    data_length,
                    limit: MAX_UP_CASE_BYTES
                }) if data_length == MAX_UP_CASE_BYTES + 1
            ),
            "{result:?}"
        );
        // At the bound the chain is followed, and here ends too soon.
        let result = with(6, MAX_UP_CASE_BYTES);
        assert!(
            matches!(
                result,
                Err(ExFatStructureError::UpCaseChainEndsEarly {
                    cluster: 9,
                    needed: 256
                })
            ),
            "{result:?}"
        );
    }

    #[test]
    fn an_up_case_chain_that_is_short_long_or_bad_is_refused() {
        let mut vol = structured(&standard_root());
        vol.fat(6, FAT_END_OF_CHAIN);
        mark(&mut vol, &[3], &all_of(&STRUCTURE));
        let result = check(&mut vol);
        assert!(
            matches!(
                result,
                Err(ExFatStructureError::UpCaseChainEndsEarly {
                    cluster: 6,
                    needed: 2
                })
            ),
            "{result:?}"
        );

        let mut vol = structured(&standard_root());
        vol.chain(&[6, 9, 10]);
        mark(&mut vol, &[3], &all_of(&STRUCTURE));
        let result = check(&mut vol);
        assert!(
            matches!(
                result,
                Err(ExFatStructureError::UpCaseChainLongerThanData {
                    cluster: 9,
                    needed: 2
                })
            ),
            "{result:?}"
        );

        let mut vol = structured(&standard_root());
        vol.fat(6, FAT_BAD_CLUSTER);
        mark(&mut vol, &[3], &all_of(&STRUCTURE));
        let result = check(&mut vol);
        assert!(
            matches!(
                result,
                Err(ExFatStructureError::UpCaseBadCluster { cluster: 6 })
            ),
            "{result:?}"
        );
    }

    #[test]
    fn a_bitmap_of_another_volume_is_not_checked() {
        let mut vol = structured(&standard_root());
        let boot = vol.boot.clone();
        let mut bitmap = {
            let mut reader = ExFatReader::new(&mut vol.image, &boot, extent(), Limits::DEFAULT);
            reader.open_bitmap(&bitmap_at(3, 25)).expect("opens")
        };
        let mut other = boot.clone();
        other.cluster_heap_offset = 65;
        let mut reader = ExFatReader::new(&mut vol.image, &other, extent(), Limits::DEFAULT);
        assert!(matches!(
            reader.check_bitmap(&mut bitmap),
            Err(ExFatStructureError::Bitmap(ExFatBitmapError::OtherVolume))
        ));
    }

    /// The root of [`structured`], with live files: F is the chain 20, 30,
    /// with 21 marked as the decoy a contiguous reading would take; G is a
    /// run of 40 to 42; E is a directory at 50 that [`check`] does not read;
    /// Z has no data.
    fn root_with_live_files() -> (Vec<[u8; 32]>, Vec<(u32, ExFatStructure)>) {
        let mut entries = standard_root();
        entries.extend(file("F", 20, 1024));
        entries.extend(file_set(&utf16("G"), 0, 40, 1500, 1500, true));
        entries.extend(directory("E", 50, 1));
        entries.extend(file("Z", 0, 0));
        let mut clusters = STRUCTURE.to_vec();
        clusters.extend([
            (20, ExFatStructure::File),
            (30, ExFatStructure::File),
            (40, ExFatStructure::File),
            (41, ExFatStructure::File),
            (42, ExFatStructure::File),
            (50, ExFatStructure::Directory),
        ]);
        (entries, clusters)
    }

    fn with_live_files() -> Vol {
        let (entries, _) = root_with_live_files();
        let mut vol = structured(&entries);
        vol.chain(&[20, 30]);
        vol.fat(21, FAT_END_OF_CHAIN);
        vol.chain(&[50]);
        mark(&mut vol, &[3], &[21]);
        vol
    }

    #[test]
    fn live_allocations_marked_in_use_are_believed_and_counted() {
        let (_, clusters) = root_with_live_files();
        let mut vol = with_live_files();
        mark(&mut vol, &[3], &all_of(&clusters));

        // D's one cluster, F's two, G's three and E's one.
        assert_eq!(
            check(&mut vol).expect("believed"),
            ExFatStructureChecked {
                directory_clusters: 3,
                bitmap_clusters: 1,
                up_case_clusters: 2,
                live_clusters: 7,
            }
        );
    }

    /// A chain followed through the FAT (30, not the decoy 21), a run, and
    /// a directory the walk did not read: any cluster of them marked free
    /// and the bitmap is not believed.
    #[test]
    fn a_live_allocation_marked_free_is_not_believed() {
        let (_, clusters) = root_with_live_files();
        for &(free, kind) in clusters.iter().filter(|&&(c, _)| c >= 20) {
            let mut vol = with_live_files();
            let others: Vec<u32> = all_of(&clusters)
                .into_iter()
                .filter(|&c| c != free)
                .collect();
            mark(&mut vol, &[3], &others);

            let result = check(&mut vol);
            assert!(
                matches!(
                    result,
                    Err(ExFatStructureError::MarkedFree { structure, cluster })
                        if structure == kind && cluster == free
                ),
                "cluster {free}: {result:?}"
            );
        }
    }

    /// F needs three clusters and its chain ends after one: the cluster it
    /// reached is checked, and nothing past the end is taken.
    #[test]
    fn a_live_chain_that_ends_early_is_checked_as_far_as_it_goes() {
        let mut entries = standard_root();
        entries.extend(file("F", 60, 1500));
        let mut vol = structured(&entries);
        vol.chain(&[60]);
        let mut all = all_of(&STRUCTURE);
        all.push(60);
        mark(&mut vol, &[3], &all);

        let checked = check(&mut vol).expect("believed");
        assert_eq!(checked.live_clusters, 2);
    }

    #[test]
    fn live_sets_past_the_bound_leave_the_bitmap_unbelieved() {
        let (_, clusters) = root_with_live_files();
        let mut vol = with_live_files();
        mark(&mut vol, &[3], &all_of(&clusters));
        let limits = Limits {
            live_allocations: 2,
            ..Limits::DEFAULT
        };

        let result = check_with(&mut vol, limits);
        assert!(
            matches!(
                result,
                Err(ExFatStructureError::LiveAllocationsUnrecorded {
                    bound: 2,
                    unrecorded: 2
                })
            ),
            "{result:?}"
        );
    }

    /// Five runs of the whole heap name 1,000 clusters, more than four times
    /// the volume's 200.
    #[test]
    fn live_allocations_naming_more_than_four_times_the_volume_are_refused() {
        let mut entries = standard_root();
        for name in ["A", "B", "C", "H", "I"] {
            entries.extend(file_set(&utf16(name), 0, 2, 102_400, 102_400, true));
        }
        let mut vol = structured(&entries);
        let all: Vec<u32> = (2..=201).collect();
        mark(&mut vol, &[3], &all);

        let result = check(&mut vol);
        assert!(
            matches!(
                result,
                Err(ExFatStructureError::LiveClustersBeyondBound { limit: 800 })
            ),
            "{result:?}"
        );
    }

    /// [`MAX_LIVE_ALLOCATIONS`]'s memory, 48 MiB, rests on this size.
    #[test]
    fn a_live_allocation_record_is_twelve_bytes() {
        assert_eq!(std::mem::size_of::<LiveAllocation>(), 12);
        assert_eq!(
            MAX_LIVE_ALLOCATIONS * std::mem::size_of::<LiveAllocation>(),
            48 << 20
        );
    }

    // --- checksum and names ---------------------------------------------------

    /// Computed by an independent implementation of the specification's
    /// Figure 2, in Python.
    #[test]
    fn the_set_checksum_matches_values_an_independent_implementation_computed() {
        let pattern: Vec<u8> = (0..96).map(|i| ((i * 5 + 1) % 256) as u8).collect();
        assert_eq!(entry_set_checksum(&pattern), 0x10b2);

        assert_eq!(entry_set_checksum(&[0u8; 64]), 0);

        let mut primary = vec![0u8; 64];
        primary[0] = 0x85;
        primary[1] = 0x02;
        assert_eq!(entry_set_checksum(&primary), 0x0448);

        let full: Vec<u8> = (0..32 * 256)
            .map(|i| (((i * 13) ^ 7) % 256) as u8)
            .collect();
        assert_eq!(entry_set_checksum(&full), 0x8478);
    }

    #[test]
    fn the_checksum_skips_the_two_bytes_it_is_stored_in() {
        let mut a = vec![0x55u8; 64];
        let b = a.clone();
        a[2] = 0;
        a[3] = 0xFF;

        assert_eq!(entry_set_checksum(&a), entry_set_checksum(&b));
    }

    #[test]
    fn printable_ascii_is_shown_as_itself() {
        assert_eq!(escaped_name(&utf16("KEEP file-1.txt~")), "KEEP file-1.txt~");
    }

    #[test]
    fn backslash_and_slash_are_escaped_so_the_rendering_is_reversible() {
        assert_eq!(escaped_name(&utf16("a\\b/c")), "a\\u{005C}b\\u{002F}c");
    }

    #[test]
    fn an_embedded_nul_and_other_controls_are_escaped() {
        assert_eq!(
            escaped_name(&[0x41, 0x0000, 0x0A, 0x7F]),
            "A\\u{0000}\\u{000A}\\u{007F}"
        );
    }

    #[test]
    fn non_ascii_is_escaped_one_code_unit_at_a_time_with_surrogates_apart() {
        // U+00E9, then U+1F600 as the pair D83D DE00.
        assert_eq!(
            escaped_name(&utf16("\u{e9}\u{1F600}")),
            "\\u{00E9}\\u{D83D}\\u{DE00}"
        );
    }

    #[test]
    fn a_lone_surrogate_is_escaped_not_replaced() {
        assert_eq!(escaped_name(&[0xD800, 0x41]), "\\u{D800}A");
    }

    #[test]
    fn an_escaped_name_holds_nothing_but_printable_ascii() {
        let all: Vec<u16> = (0..=0xFFFF).collect();
        assert!(
            escaped_name(&all)
                .bytes()
                .all(|b| (0x20..=0x7E).contains(&b))
        );
    }

    // --- a directory that is read -------------------------------------------

    #[test]
    fn a_label_a_bitmap_and_a_file_are_read_from_the_root() {
        let mut vol = Vol::new();
        vol.chain(&[2]);

        let mut label = [0u8; 32];
        label[0] = TYPE_VOLUME_LABEL;
        label[1] = 4;
        for (i, unit) in utf16("EXP9").iter().enumerate() {
            label[2 + i * 2..4 + i * 2].copy_from_slice(&unit.to_le_bytes());
        }
        let mut bitmap = [0u8; 32];
        bitmap[0] = TYPE_BITMAP;
        bitmap[20..24].copy_from_slice(&3u32.to_le_bytes());
        bitmap[24..32].copy_from_slice(&25u64.to_le_bytes());

        vol.put(2, 0, &[label, bitmap]);
        vol.put(2, 2, &file("KEEP.txt", 8, 100));

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        let summary = result.expect("the directory reads");

        assert_eq!(seen[0], Seen::Label(utf16("EXP9")));
        assert_eq!(
            seen[1],
            Seen::Bitmap(ExFatBitmapEntry {
                first_cluster: 3,
                data_length: 25
            })
        );
        match &seen[2] {
            Seen::File(position, f) => {
                assert_eq!(
                    *position,
                    ExFatPosition {
                        cluster: 2,
                        slot: 2
                    }
                );
                assert_eq!(f.name, utf16("KEEP.txt"));
                assert_eq!(f.first_cluster, 8);
                assert_eq!(f.data_length, 100);
                assert!(!f.is_directory());
            }
            other => panic!("expected the file, got {other:?}"),
        }
        assert_eq!(summary.files, 1);
        // The label, the bitmap, the file's set, and the terminator.
        assert_eq!(summary.entries_read, 4);
        assert!(summary.terminated);
    }

    #[test]
    fn a_set_that_straddles_two_clusters_is_read_whole() {
        let mut vol = Vol::new();
        vol.chain(&[2, 3]);
        vol.fill_unused(2);
        let set = file("straddle.bin", 9, 64);
        assert_eq!(set.len(), 3);
        // The primary is the last slot of cluster 2; the rest begin cluster 3.
        vol.put(2, PER_CLUSTER - 1, &set[..1]);
        vol.put(3, 0, &set[1..]);
        vol.put(3, 2, &file("next", 0, 0));

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        result.expect("the directory reads");

        let files: Vec<_> = seen
            .iter()
            .filter_map(|s| match s {
                Seen::File(p, f) => Some((*p, f.name.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            files,
            vec![
                (
                    ExFatPosition {
                        cluster: 2,
                        slot: 15
                    },
                    utf16("straddle.bin")
                ),
                (
                    ExFatPosition {
                        cluster: 3,
                        slot: 2
                    },
                    utf16("next")
                ),
            ]
        );
    }

    #[test]
    fn a_name_of_255_units_over_17_entries_is_read() {
        let name: String = std::iter::repeat_n('n', 255).collect();
        let mut vol = Vol::new();
        vol.chain(&[2, 3]);
        vol.put(2, 0, &file(&name, 0, 0));

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        result.expect("the directory reads");
        match &seen[0] {
            Seen::File(_, f) => assert_eq!(f.name, utf16(&name)),
            other => panic!("expected the file, got {other:?}"),
        }
    }

    #[test]
    fn a_subdirectory_that_is_one_contiguous_run_ignores_the_fat() {
        let mut vol = Vol::new();
        // No FAT entries at all for 10 and 11: a contiguous run must not
        // consult them.
        vol.put(10, 0, &[[0x01; 32]; PER_CLUSTER]);
        vol.put(11, 0, &file("inside", 0, 0));

        let source = DirectorySource::Subdirectory {
            first_cluster: 10,
            data_length: 1024,
            no_fat_chain: true,
        };
        let (result, seen) = walk(&mut vol, source, Limits::DEFAULT);
        let summary = result.expect("the directory reads");

        assert_eq!(summary.clusters_read, 2);
        assert!(matches!(&seen[0], Seen::File(p, f)
            if p.cluster == 11 && f.name == utf16("inside")));
    }

    #[test]
    fn a_subdirectory_that_is_a_fat_chain_follows_the_chain() {
        let mut vol = Vol::new();
        vol.chain(&[10, 40]);
        vol.fill_unused(10);
        vol.put(40, 0, &file("far", 0, 0));

        let source = DirectorySource::Subdirectory {
            first_cluster: 10,
            data_length: 1024,
            no_fat_chain: false,
        };
        let (result, seen) = walk(&mut vol, source, Limits::DEFAULT);
        result.expect("the directory reads");
        assert!(matches!(&seen[0], Seen::File(p, _) if p.cluster == 40));
    }

    #[test]
    fn a_subdirectory_with_no_data_has_no_clusters() {
        let mut vol = Vol::new();
        let source = DirectorySource::Subdirectory {
            first_cluster: 0,
            data_length: 0,
            no_fat_chain: false,
        };

        let (result, seen) = walk(&mut vol, source, Limits::DEFAULT);
        assert_eq!(
            result.expect("an empty directory"),
            DirectorySummary::default()
        );
        assert!(seen.is_empty());
    }

    #[test]
    fn a_directory_length_that_is_not_a_whole_entry_leaves_the_tail_unread() {
        let mut vol = Vol::new();
        vol.chain(&[10]);
        vol.put(10, 0, &file("a", 0, 0));
        vol.put(10, 3, &file("b", 0, 0));

        // 3 entries and 10 bytes: the partial fourth is not an entry.
        let source = DirectorySource::Subdirectory {
            first_cluster: 10,
            data_length: 106,
            no_fat_chain: false,
        };
        let (result, seen) = walk(&mut vol, source, Limits::DEFAULT);
        result.expect("the directory reads");
        assert_eq!(seen.len(), 1, "{seen:?}");
    }

    // --- chain faults ---------------------------------------------------------

    #[test]
    fn a_cycle_is_a_fault_not_a_hang() {
        let mut vol = Vol::new();
        vol.fat(2, 3);
        vol.fat(3, 2);
        vol.fill_unused(2);
        vol.fill_unused(3);

        let (result, _) = walk(&mut vol, root(2), Limits::DEFAULT);
        assert!(matches!(
            result,
            Err(ExFatDirectoryError::ClusterAlreadyRead { cluster: 2 })
        ));
    }

    #[test]
    fn a_link_outside_the_cluster_heap_is_a_fault() {
        // 0 and 1 are reserved, 202 is one past the last cluster, and the
        // largest value is not a marker.
        for to in [0u32, 1, 202, 9999, 0xFFFF_FFF8, 0xFFFF_FFF6] {
            let mut vol = Vol::new();
            vol.fat(2, to);
            vol.fill_unused(2);

            let (result, _) = walk(&mut vol, root(2), Limits::DEFAULT);
            assert!(
                matches!(result, Err(ExFatDirectoryError::LinkOutOfRange { from: 2, to: t }) if t == to),
                "{to:#x}: {result:?}"
            );
        }
    }

    #[test]
    fn a_link_to_a_bad_cluster_is_a_fault_and_the_bad_cluster_is_not_read() {
        let mut vol = Vol::new();
        vol.fat(2, 3);
        vol.fat(3, FAT_BAD_CLUSTER);
        vol.fill_unused(2);
        // A file in the bad cluster must not be listed.
        vol.put(3, 0, &file("never", 0, 0));

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        assert!(matches!(
            result,
            Err(ExFatDirectoryError::BadCluster { cluster: 3 })
        ));
        assert!(seen.is_empty(), "{seen:?}");
    }

    #[test]
    fn a_chain_over_the_byte_limit_is_a_fault() {
        let mut vol = Vol::new();
        vol.chain(&[2, 3, 4, 5]);
        for c in 2..=5 {
            vol.fill_unused(c);
        }
        let limits = Limits {
            directory_bytes: 3 * 512,
            ..Limits::DEFAULT
        };

        let (result, _) = walk(&mut vol, root(2), limits);
        assert!(matches!(
            result,
            Err(ExFatDirectoryError::ChainTooLong { limit_bytes: 1536 })
        ));
    }

    #[test]
    fn a_chain_exactly_at_the_byte_limit_is_read() {
        let mut vol = Vol::new();
        vol.chain(&[2, 3, 4]);
        for c in 2..=4 {
            vol.fill_unused(c);
        }
        let limits = Limits {
            directory_bytes: 3 * 512,
            ..Limits::DEFAULT
        };

        let (result, _) = walk(&mut vol, root(2), limits);
        assert_eq!(result.expect("three clusters fit").clusters_read, 3);
    }

    #[test]
    fn the_cluster_cap_stops_a_walk_with_its_reason() {
        let mut vol = Vol::new();
        vol.chain(&[2, 3, 4]);
        for c in 2..=4 {
            vol.fill_unused(c);
        }
        let limits = Limits {
            directory_clusters: 2,
            ..Limits::DEFAULT
        };

        let (result, _) = walk(&mut vol, root(2), limits);
        assert!(matches!(
            result,
            Err(ExFatDirectoryError::ClusterCapReached { cap: 2 })
        ));
    }

    #[test]
    fn the_cluster_cap_spans_every_directory_read_through_one_reader() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.chain(&[3]);
        vol.fill_unused(2);
        vol.fill_unused(3);
        let boot = vol.boot.clone();
        let limits = Limits {
            directory_clusters: 1,
            ..Limits::DEFAULT
        };
        let mut reader = ExFatReader::new(&mut vol.image, &boot, extent(), limits);

        let (first, _) = walk_with(&mut reader, root(2));
        assert!(first.is_ok());
        let (second, _) = walk_with(&mut reader, root(3));
        assert!(matches!(
            second,
            Err(ExFatDirectoryError::ClusterCapReached { cap: 1 })
        ));
        assert_eq!(reader.clusters_read(), 1);
    }

    #[test]
    fn a_first_cluster_outside_the_heap_is_a_fault() {
        for first in [0u32, 1, 202, 300] {
            let mut vol = Vol::new();
            let (result, _) = walk(&mut vol, root(first), Limits::DEFAULT);
            assert!(
                matches!(result, Err(ExFatDirectoryError::ClusterOutOfRange { cluster }) if cluster == first),
                "{first}: {result:?}"
            );
        }
    }

    #[test]
    fn a_chain_that_ends_before_the_directorys_length_is_a_fault() {
        let mut vol = Vol::new();
        vol.chain(&[10]);
        vol.fill_unused(10);
        let source = DirectorySource::Subdirectory {
            first_cluster: 10,
            data_length: 1024,
            no_fat_chain: false,
        };

        let (result, _) = walk(&mut vol, source, Limits::DEFAULT);
        assert!(matches!(
            result,
            Err(ExFatDirectoryError::ChainEndsEarly { cluster: 10 })
        ));
    }

    /// One cluster of length, as a chain from cluster 10.
    fn one_cluster_chain() -> DirectorySource {
        DirectorySource::Subdirectory {
            first_cluster: 10,
            data_length: 512,
            no_fat_chain: false,
        }
    }

    #[test]
    fn a_chain_that_goes_on_past_the_directorys_length_is_a_fault() {
        let mut vol = Vol::new();
        vol.chain(&[10, 11]);
        // The zeros after the file are an end marker.
        vol.put(10, 0, &file("a", 0, 0));

        let (result, seen) = walk(&mut vol, one_cluster_chain(), Limits::DEFAULT);
        assert!(
            matches!(
                result,
                Err(ExFatDirectoryError::ChainLongerThanData { cluster: 10 })
            ),
            "{result:?}"
        );
        // What was read before the fault stands.
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!(matches!(&seen[0], Seen::File(_, f) if f.name == utf16("a")));
    }

    #[test]
    fn a_longer_chain_is_a_fault_without_an_end_marker_too() {
        let mut vol = Vol::new();
        vol.chain(&[10, 11]);
        vol.fill_unused(10);

        let (result, _) = walk(&mut vol, one_cluster_chain(), Limits::DEFAULT);
        assert!(
            matches!(
                result,
                Err(ExFatDirectoryError::ChainLongerThanData { cluster: 10 })
            ),
            "{result:?}"
        );
    }

    #[test]
    fn a_last_link_outside_the_heap_is_a_fault() {
        // Section 4.1.3: only end-of-chain may end a chain, so a last link
        // outside the heap, or to free cluster 0, is not an end.
        for to in [5000u32, 0] {
            let mut vol = Vol::new();
            vol.fat(10, to);
            vol.fill_unused(10);

            let (result, _) = walk(&mut vol, one_cluster_chain(), Limits::DEFAULT);
            assert!(
                matches!(result, Err(ExFatDirectoryError::LinkOutOfRange { from: 10, to: t }) if t == to),
                "{to}: {result:?}"
            );
        }
    }

    #[test]
    fn a_subdirectory_with_a_cluster_and_no_data_is_a_fault() {
        for no_fat_chain in [false, true] {
            let mut vol = Vol::new();
            vol.chain(&[10]);
            vol.put(10, 0, &file("hidden", 0, 0));
            let source = DirectorySource::Subdirectory {
                first_cluster: 10,
                data_length: 0,
                no_fat_chain,
            };

            let (result, seen) = walk(&mut vol, source, Limits::DEFAULT);
            assert!(
                matches!(
                    result,
                    Err(ExFatDirectoryError::ClusterWithoutData { cluster: 10 })
                ),
                "{no_fat_chain}: {result:?}"
            );
            assert!(seen.is_empty(), "{no_fat_chain}: {seen:?}");
        }
    }

    #[test]
    fn a_contiguous_directory_ignores_the_fat_after_its_last_cluster() {
        let mut vol = Vol::new();
        // A FAT entry that would continue a chain: a contiguous run must
        // not consult it.
        vol.fat(10, 11);
        vol.fill_unused(10);
        let source = DirectorySource::Subdirectory {
            first_cluster: 10,
            data_length: 512,
            no_fat_chain: true,
        };

        let (result, _) = walk(&mut vol, source, Limits::DEFAULT);
        assert_eq!(result.expect("the directory reads").clusters_read, 1);
    }

    #[test]
    fn a_cluster_already_read_by_another_directory_is_not_read_again() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.fill_unused(2);
        let boot = vol.boot.clone();
        let mut reader = ExFatReader::new(&mut vol.image, &boot, extent(), Limits::DEFAULT);

        let (first, _) = walk_with(&mut reader, root(2));
        assert!(first.is_ok());
        let again = DirectorySource::Subdirectory {
            first_cluster: 2,
            data_length: 512,
            no_fat_chain: true,
        };
        let (second, _) = walk_with(&mut reader, again);
        assert!(matches!(
            second,
            Err(ExFatDirectoryError::ClusterAlreadyRead { cluster: 2 })
        ));
    }

    #[test]
    fn an_evidence_failure_is_reported_as_one() {
        // The image ends before the cluster.
        let mut vol = Vol::new();
        vol.image = MemoryImage::new(64 * 512);
        vol.fat(2, FAT_END_OF_CHAIN);

        let (result, _) = walk(&mut vol, root(2), Limits::DEFAULT);
        assert!(matches!(result, Err(ExFatDirectoryError::Evidence(_))));
    }

    // --- entry set faults ---------------------------------------------------

    #[test]
    fn a_secondary_count_below_two_is_refused() {
        for count in [0usize, 1] {
            let set = mutated(file("x", 0, 0), |s| {
                s.truncate(1);
                s[0][1] = count as u8;
            });
            assert_eq!(
                refusal_of(set),
                SetRefusal::SecondaryCountTooSmall { count: count as u8 }
            );
        }
    }

    #[test]
    fn a_wrong_checksum_is_refused_and_the_set_is_not_used() {
        let mut set = file("x", 5, 100);
        set[0][2] ^= 1;

        assert!(matches!(
            refusal_of(set),
            SetRefusal::ChecksumMismatch { .. }
        ));
    }

    #[test]
    fn a_set_cut_short_by_the_terminator_is_refused() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        let mut set = file("x", 0, 0);
        set.truncate(2);
        set[0][1] = 3;
        seal(&mut set);
        vol.put(2, 0, &set);

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        let summary = result.expect("the directory reads");
        assert_eq!(
            seen,
            vec![Seen::Refused(
                ExFatPosition {
                    cluster: 2,
                    slot: 0
                },
                SetRefusal::SetTruncated {
                    declared: 3,
                    found: 1
                }
            )]
        );
        assert!(summary.terminated);
    }

    /// The entry that broke the set is handed back, so a wrong count does not
    /// swallow the valid set after it.
    #[test]
    fn a_set_cut_short_by_the_next_set_does_not_swallow_it() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        let mut set = file("x", 0, 0);
        set.truncate(2);
        set[0][1] = 3;
        seal(&mut set);
        vol.put(2, 0, &set);
        vol.put(2, 2, &file("next", 0, 0));

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        result.expect("the directory reads");
        assert!(matches!(
            &seen[0],
            Seen::Refused(_, SetRefusal::SetTruncated { .. })
        ));
        assert!(matches!(&seen[1], Seen::File(_, f) if f.name == utf16("next")));
    }

    #[test]
    fn a_first_secondary_that_is_not_a_stream_is_refused() {
        let set = mutated(file("x", 0, 0), |s| s[1][0] = TYPE_FILE_NAME);

        assert_eq!(
            refusal_of(set),
            SetRefusal::StreamExtensionMissing {
                found: TYPE_FILE_NAME
            }
        );
    }

    #[test]
    fn a_stream_that_allows_no_allocation_is_refused() {
        let set = mutated(file("x", 0, 0), |s| s[1][1] = 0);

        assert_eq!(refusal_of(set), SetRefusal::AllocationNotPossible);
    }

    #[test]
    fn a_name_length_of_zero_is_refused() {
        let set = mutated(file("x", 0, 0), |s| s[1][3] = 0);

        assert_eq!(
            refusal_of(set),
            SetRefusal::NameLengthOutOfRange { length: 0 }
        );
    }

    /// The vulnerability class of CVE-2023-4273: a name length that the
    /// entries do not hold.
    #[test]
    fn a_name_length_larger_than_the_entries_hold_is_refused() {
        let long: String = std::iter::repeat_n('a', 45).collect();
        let set = mutated(file(&long, 0, 0), |s| s[1][3] = 255);

        assert_eq!(
            refusal_of(set),
            SetRefusal::NameEntries {
                expected: 17,
                found: 3
            }
        );
    }

    #[test]
    fn a_name_length_smaller_than_the_entries_need_is_refused() {
        let long: String = std::iter::repeat_n('a', 45).collect();
        let set = mutated(file(&long, 0, 0), |s| s[1][3] = 10);

        assert_eq!(
            refusal_of(set),
            SetRefusal::NameEntries {
                expected: 1,
                found: 3
            }
        );
    }

    /// 18 name entries, one more than the 17 a name can need.
    #[test]
    fn a_set_of_18_name_entries_is_refused_without_reading_past_17() {
        let mut set = vec![
            primary(0x20, 19),
            stream(FLAG_ALLOCATION_POSSIBLE, 255, 0, 0, 0),
        ];
        for _ in 0..18 {
            set.push(name_entry(&utf16("abcdefghijklmno")));
        }
        seal(&mut set);

        // 20 entries: the set runs into a second cluster.
        let mut vol = Vol::new();
        vol.chain(&[2, 3]);
        vol.put(2, 0, &set[..PER_CLUSTER]);
        vol.put(3, 0, &set[PER_CLUSTER..]);
        vol.put(3, set.len() - PER_CLUSTER, &file("OK", 0, 0));

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        result.expect("the directory reads");
        assert!(matches!(
            &seen[0],
            Seen::Refused(
                _,
                SetRefusal::NameEntries {
                    expected: 17,
                    found: 18
                }
            )
        ));
        assert!(matches!(&seen[1], Seen::File(_, f) if f.name == utf16("OK")));
    }

    /// The largest set the specification allows: 255 secondaries, which here
    /// are 254 name entries after the stream. It spans 17 clusters, so the
    /// entry window is also exercised at its limit.
    #[test]
    fn a_set_of_255_secondaries_is_refused_without_panic_or_a_wrong_read() {
        let mut vol = Vol::new();
        let clusters: Vec<u32> = (2..=18).collect();
        vol.chain(&clusters);

        let mut set = vec![
            primary(0x20, 255),
            stream(FLAG_ALLOCATION_POSSIBLE, 255, 0, 0, 0),
        ];
        for _ in 0..254 {
            set.push(name_entry(&utf16("abcdefghijklmno")));
        }
        seal(&mut set);
        assert_eq!(set.len(), 256);
        for (i, chunk) in set.chunks(PER_CLUSTER).enumerate() {
            vol.put(2 + i as u32, 0, chunk);
        }
        vol.put(18, 0, &file("after", 0, 0));

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        result.expect("the directory reads");

        assert!(matches!(
            &seen[0],
            Seen::Refused(
                _,
                SetRefusal::NameEntries {
                    expected: 17,
                    found: 254
                }
            )
        ));
        assert!(matches!(&seen[1], Seen::File(_, f) if f.name == utf16("after")));
    }

    #[test]
    fn a_critical_secondary_after_the_names_makes_the_set_unrecognised() {
        let set = mutated(file("x", 0, 0), |s| {
            let mut extra = [0u8; 32];
            extra[0] = 0xC2;
            s.push(extra);
            s[0][1] += 1;
        });

        assert_eq!(
            refusal_of(set),
            SetRefusal::UnrecognisedCriticalSecondary { entry_type: 0xC2 }
        );
    }

    #[test]
    fn a_benign_secondary_after_the_names_is_ignored() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        let set = mutated(file("vendor", 0, 0), |s| {
            let mut extra = [0u8; 32];
            extra[0] = 0xE0;
            s.push(extra);
            s[0][1] += 1;
        });
        vol.put(2, 0, &set);

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        result.expect("the directory reads");
        assert!(matches!(&seen[0], Seen::File(_, f) if f.name == utf16("vendor")));
        assert_eq!(seen.len(), 1);
    }

    #[test]
    fn a_valid_data_length_over_the_data_length_is_refused() {
        let set = file_set(&utf16("x"), 0x20, 5, 101, 100, false);

        assert_eq!(
            refusal_of(set),
            SetRefusal::ValidDataLengthExceedsDataLength {
                valid_data_length: 101,
                data_length: 100
            }
        );
    }

    #[test]
    fn a_valid_data_length_below_the_data_length_is_listed_as_it_is() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.put(2, 0, &file_set(&utf16("x"), 0x20, 5, 40, 100, false));

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        result.expect("the directory reads");
        assert!(matches!(&seen[0], Seen::File(_, f)
            if f.valid_data_length == 40 && f.data_length == 100));
    }

    #[test]
    fn a_directory_whose_valid_data_length_is_not_its_data_length_is_refused() {
        let set = file_set(&utf16("d"), ATTRIBUTE_DIRECTORY, 5, 512, 1024, false);

        assert_eq!(
            refusal_of(set),
            SetRefusal::DirectoryValidDataLength {
                valid_data_length: 512,
                data_length: 1024
            }
        );
    }

    #[test]
    fn a_directory_over_256_mb_is_refused() {
        let size = MAX_DIRECTORY_BYTES + 1;
        let set = file_set(&utf16("d"), ATTRIBUTE_DIRECTORY, 5, size, size, false);

        assert_eq!(
            refusal_of(set),
            SetRefusal::DirectoryTooLarge { data_length: size }
        );
    }

    #[test]
    fn a_data_length_beyond_the_whole_heap_is_refused() {
        // 200 clusters of 512 bytes.
        let set = file_set(&utf16("x"), 0x20, 5, 102_401, 102_401, false);

        assert_eq!(
            refusal_of(set),
            SetRefusal::DataLengthTooLarge {
                data_length: 102_401
            }
        );
    }

    #[test]
    fn data_with_no_first_cluster_is_refused() {
        let set = file_set(&utf16("x"), 0x20, 0, 10, 10, false);

        assert_eq!(
            refusal_of(set),
            SetRefusal::DataWithoutCluster { data_length: 10 }
        );
    }

    #[test]
    fn a_first_cluster_outside_the_heap_is_refused_in_a_set() {
        for first in [1u32, 202, u32::MAX] {
            let set = file_set(&utf16("x"), 0x20, first, 10, 10, false);

            assert_eq!(
                refusal_of(set),
                SetRefusal::FirstClusterOutOfRange { cluster: first }
            );
        }
    }

    #[test]
    fn a_no_fat_chain_flag_with_no_first_cluster_is_refused() {
        let set = file_set(&utf16("x"), 0x20, 0, 0, 0, true);

        assert_eq!(refusal_of(set), SetRefusal::NoFatChainWithoutCluster);
    }

    #[test]
    fn a_no_fat_chain_flag_with_no_data_is_refused() {
        let set = file_set(&utf16("x"), 0x20, 10, 0, 0, true);

        assert_eq!(refusal_of(set), SetRefusal::NoFatChainWithoutData);
    }

    #[test]
    fn a_contiguous_allocation_past_the_heap_is_refused() {
        // 200 and two more clusters from there: the last cluster is 201.
        let set = file_set(&utf16("x"), 0x20, 200, 1536, 1536, true);

        assert_eq!(
            refusal_of(set),
            SetRefusal::AllocationOutOfRange {
                first_cluster: 200,
                clusters: 3
            }
        );
    }

    #[test]
    fn a_contiguous_allocation_ending_at_the_last_cluster_is_accepted() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.put(2, 0, &file_set(&utf16("x"), 0x20, 200, 1024, 1024, true));

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        result.expect("the directory reads");
        assert!(matches!(&seen[0], Seen::File(..)));
    }

    #[test]
    fn an_empty_file_with_no_cluster_is_accepted() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.put(2, 0, &file("empty", 0, 0));

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        let summary = result.expect("the directory reads");
        assert_eq!(summary.files, 1);
        assert_eq!(seen.len(), 1);
    }

    // --- unused entries, markers and primaries ------------------------------

    #[test]
    fn an_unused_file_entry_is_counted_and_its_secondaries_are_not_interpreted() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        // A deleted set: the InUse bit of each entry cleared.
        let deleted = mutated(file("gone", 5, 100), |s| {
            for entry in s.iter_mut() {
                entry[0] &= 0x7F;
            }
        });
        vol.put(2, 0, &deleted);
        vol.put(2, 3, &file("kept", 0, 0));

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        let summary = result.expect("the directory reads");

        assert_eq!(summary.unused_file_sets, 1);
        assert_eq!(summary.refused, 0);
        assert_eq!(summary.files, 1);
        assert_eq!(
            seen[0],
            Seen::Unused(ExFatPosition {
                cluster: 2,
                slot: 0
            })
        );
        assert!(matches!(&seen[1], Seen::File(_, f) if f.name == utf16("kept")));
    }

    #[test]
    fn unused_entries_that_are_not_file_entries_are_not_counted() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.fill_unused(2);

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        let summary = result.expect("the directory reads");
        assert_eq!(summary.unused_file_sets, 0);
        assert!(seen.is_empty());
    }

    #[test]
    fn nothing_after_the_end_marker_is_listed_and_what_is_there_is_counted() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.put(2, 0, &file("before", 0, 0));
        vol.put(2, 4, &file("after", 0, 0));

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        let summary = result.expect("the directory reads");
        assert!(summary.terminated);
        // The one file before the marker, and the three entries of the set
        // after it, counted and not interpreted.
        assert_eq!(summary.files, 1);
        assert_eq!(summary.entries_after_end_marker, 3);
        assert_eq!(seen.len(), 2);
        assert!(matches!(&seen[0], Seen::File(_, f) if f.name == utf16("before")));
        assert_eq!(seen[1], Seen::AfterEndMarker(3));
    }

    #[test]
    fn a_directory_that_is_zero_after_its_end_marker_has_no_residue() {
        let mut vol = Vol::new();
        vol.chain(&[2, 3]);
        vol.put(2, 0, &file("only", 0, 0));

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        let summary = result.expect("the directory reads");
        assert_eq!(summary.entries_after_end_marker, 0);
        assert_eq!(summary.clusters_read, 2, "the tail is read to be checked");
        assert_eq!(seen.len(), 1);
    }

    #[test]
    fn entries_after_the_end_marker_are_found_in_a_later_cluster() {
        let mut vol = Vol::new();
        vol.chain(&[2, 3, 4]);
        vol.put(2, 0, &file("only", 0, 0));
        vol.put(4, 7, &[[0x41; 32], [0x00; 32], [0x05; 32]]);

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        let summary = result.expect("the directory reads");
        assert_eq!(summary.entries_after_end_marker, 2);
        assert_eq!(seen.last(), Some(&Seen::AfterEndMarker(2)));
    }

    /// Reading past the marker is held to the same limits as the rest, and a
    /// fault in the tail is a fault of the directory.
    #[test]
    fn a_fault_in_the_tail_after_the_end_marker_is_a_fault() {
        let mut vol = Vol::new();
        vol.fat(2, 9999);
        vol.put(2, 0, &file("only", 0, 0));

        let (result, _) = walk(&mut vol, root(2), Limits::DEFAULT);
        assert!(matches!(
            result,
            Err(ExFatDirectoryError::LinkOutOfRange { from: 2, to: 9999 })
        ));
    }

    #[test]
    fn the_tail_after_the_end_marker_is_held_to_the_cluster_cap() {
        let mut vol = Vol::new();
        vol.chain(&[2, 3, 4]);
        vol.put(2, 0, &file("only", 0, 0));
        let limits = Limits {
            directory_clusters: 2,
            ..Limits::DEFAULT
        };

        let (result, _) = walk(&mut vol, root(2), limits);
        assert!(matches!(
            result,
            Err(ExFatDirectoryError::ClusterCapReached { cap: 2 })
        ));
    }

    #[test]
    fn entry_type_80h_is_refused() {
        let mut entry = [0u8; 32];
        entry[0] = TYPE_INVALID;
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.put(2, 0, &[entry]);

        let (_, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        assert_eq!(
            seen,
            vec![Seen::Refused(
                ExFatPosition {
                    cluster: 2,
                    slot: 0
                },
                SetRefusal::InvalidEntryType { entry_type: 0x80 }
            )]
        );
    }

    #[test]
    fn a_secondary_entry_where_a_set_should_begin_is_refused() {
        let mut entry = [0u8; 32];
        entry[0] = TYPE_STREAM;
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.put(2, 0, &[entry]);

        let (_, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        assert!(matches!(
            &seen[0],
            Seen::Refused(_, SetRefusal::StraySecondary { entry_type: 0xC0 })
        ));
    }

    #[test]
    fn an_unrecognised_benign_primary_is_refused_with_its_secondaries_consumed() {
        let mut primary = [0u8; 32];
        primary[0] = 0xA1;
        primary[1] = 1;
        let mut secondary = [0u8; 32];
        secondary[0] = 0xE0;
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.put(2, 0, &[primary, secondary]);
        vol.put(2, 2, &file("next", 0, 0));

        let (_, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert!(matches!(
            &seen[0],
            Seen::Refused(
                _,
                SetRefusal::UnrecognisedBenignPrimary { entry_type: 0xA1 }
            )
        ));
        assert!(matches!(&seen[1], Seen::File(..)));
    }

    #[test]
    fn the_volume_guid_entry_is_recognised_and_ignored() {
        let mut guid = [0u8; 32];
        guid[0] = TYPE_VOLUME_GUID;
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.put(2, 0, &[guid]);

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        let summary = result.expect("the directory reads");
        assert!(seen.is_empty());
        assert_eq!(summary.refused, 0);
    }

    #[test]
    fn the_up_case_table_entry_is_recognised_and_ignored() {
        let mut upcase = [0u8; 32];
        upcase[0] = TYPE_UP_CASE;
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.put(2, 0, &[upcase]);

        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        let summary = result.expect("the directory reads");
        assert!(seen.is_empty());
        assert_eq!(summary.refused, 0);
    }

    #[test]
    fn an_unrecognised_critical_primary_in_the_root_invalidates_the_volume() {
        let mut entry = [0u8; 32];
        entry[0] = 0x84;
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.put(2, 0, &[entry]);

        let (result, _) = walk(&mut vol, root(2), Limits::DEFAULT);
        assert!(matches!(
            result,
            Err(ExFatDirectoryError::UnrecognisedCriticalPrimary { entry_type: 0x84 })
        ));
    }

    #[test]
    fn a_critical_primary_other_than_a_file_outside_the_root_invalidates_the_directory() {
        let mut bitmap = [0u8; 32];
        bitmap[0] = TYPE_BITMAP;
        for kind in [TYPE_BITMAP, TYPE_UP_CASE, TYPE_VOLUME_LABEL, 0x84] {
            let mut entry = bitmap;
            entry[0] = kind;
            let mut vol = Vol::new();
            vol.chain(&[10]);
            vol.put(10, 0, &[entry]);
            let source = DirectorySource::Subdirectory {
                first_cluster: 10,
                data_length: 512,
                no_fat_chain: false,
            };

            let (result, _) = walk(&mut vol, source, Limits::DEFAULT);
            assert!(
                matches!(result, Err(ExFatDirectoryError::CriticalPrimaryOutsideRoot { entry_type }) if entry_type == kind),
                "{kind:#04x}: {result:?}"
            );
        }
    }

    #[test]
    fn a_file_entry_is_valid_outside_the_root() {
        let mut vol = Vol::new();
        vol.chain(&[10]);
        vol.put(10, 0, &file("inner", 0, 0));
        let source = DirectorySource::Subdirectory {
            first_cluster: 10,
            data_length: 512,
            no_fat_chain: false,
        };

        let (result, seen) = walk(&mut vol, source, Limits::DEFAULT);
        result.expect("the directory reads");
        assert!(matches!(&seen[0], Seen::File(..)));
    }

    fn bitmap_entry(flags: u8, first: u32, length: u64) -> [u8; 32] {
        let mut entry = [0u8; 32];
        entry[0] = TYPE_BITMAP;
        entry[1] = flags;
        entry[20..24].copy_from_slice(&first.to_le_bytes());
        entry[24..32].copy_from_slice(&length.to_le_bytes());
        entry
    }

    fn only_seen(entries: &[[u8; 32]]) -> Vec<Seen> {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.put(2, 0, entries);
        let (result, seen) = walk(&mut vol, root(2), Limits::DEFAULT);
        result.expect("the directory reads");
        seen
    }

    #[test]
    fn a_bitmap_for_the_second_fat_is_refused() {
        assert!(matches!(
            &only_seen(&[bitmap_entry(1, 3, 25)])[0],
            Seen::Refused(_, SetRefusal::BitmapIdentifier { flags: 1 })
        ));
    }

    #[test]
    fn a_bitmap_first_cluster_outside_the_heap_is_refused() {
        for first in [0u32, 1, 202] {
            assert!(matches!(
                &only_seen(&[bitmap_entry(0, first, 25)])[0],
                Seen::Refused(_, SetRefusal::BitmapFirstCluster { cluster }) if *cluster == first
            ));
        }
    }

    #[test]
    fn a_bitmap_too_short_for_the_clusters_is_refused() {
        // 200 clusters need 25 bytes.
        assert!(matches!(
            &only_seen(&[bitmap_entry(0, 3, 24)])[0],
            Seen::Refused(
                _,
                SetRefusal::BitmapTooShort {
                    data_length: 24,
                    needed: 25
                }
            )
        ));
        assert!(matches!(
            &only_seen(&[bitmap_entry(0, 3, 25)])[0],
            Seen::Bitmap(_)
        ));
    }

    #[test]
    fn a_second_bitmap_entry_is_refused() {
        let seen = only_seen(&[bitmap_entry(0, 3, 25), bitmap_entry(0, 4, 25)]);

        assert!(matches!(&seen[0], Seen::Bitmap(_)));
        assert!(matches!(
            &seen[1],
            Seen::Refused(_, SetRefusal::DuplicateBitmap)
        ));
    }

    #[test]
    fn a_label_over_11_characters_is_refused() {
        let mut label = [0u8; 32];
        label[0] = TYPE_VOLUME_LABEL;
        label[1] = 12;

        assert!(matches!(
            &only_seen(&[label])[0],
            Seen::Refused(_, SetRefusal::LabelLength { count: 12 })
        ));
    }

    /// Whatever the bytes, a walk returns: no panic, no hang, no read outside
    /// the evidence. Deterministic, so a failure repeats.
    #[test]
    fn arbitrary_directory_bytes_never_panic() {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u32
        };

        for round in 0..300 {
            let mut vol = Vol::new();
            let clusters: Vec<u32> = (2..=6).collect();
            vol.chain(&clusters);

            // Random bytes, with a bias toward real entry types so that the
            // set parser is reached and not only the unused-entry skip.
            let types = [0x85u8, 0xC0, 0xC1, 0xE0, 0x05, 0x40, 0x41, 0x81, 0x83, 0x80];
            for &c in &clusters {
                let mut entries = [[0u8; 32]; PER_CLUSTER];
                for entry in entries.iter_mut() {
                    for byte in entry.iter_mut() {
                        *byte = next() as u8;
                    }
                    if next() % 3 != 0 {
                        entry[0] = types[next() as usize % types.len()];
                    }
                    if next() % 4 == 0 {
                        entry[1] = (next() % 4) as u8;
                    }
                }
                vol.put(c, 0, &entries);
            }
            if round % 3 == 0 {
                vol.fat(3, next());
            }

            let _ = walk(&mut vol, root(2), Limits::DEFAULT);
        }
    }

    #[test]
    fn every_refusal_and_error_has_a_message() {
        for refusal in [
            SetRefusal::SecondaryCountTooSmall { count: 1 },
            SetRefusal::SetTruncated {
                declared: 3,
                found: 1,
            },
            SetRefusal::DuplicateBitmap,
            SetRefusal::NoFatChainWithoutData,
        ] {
            assert!(!refusal.to_string().is_empty());
        }
        assert!(
            !ExFatDirectoryError::ClusterCapReached { cap: 5 }
                .to_string()
                .is_empty()
        );
    }

    // --- content ----------------------------------------------------------------

    /// Where the synthetic volume's cluster heap begins, in bytes.
    const HEAP_BYTE_OFFSET: u64 = 64 * 512;

    /// Evidence that records the offset and length of every read made of it.
    struct Recording<'a> {
        image: &'a mut MemoryImage,
        reads: Vec<(u64, usize)>,
    }

    impl EvidenceReader for Recording<'_> {
        fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
            self.reads.push((offset, buf.len()));
            self.image.read_exact_at(offset, buf)
        }
    }

    /// What reading one file's content did.
    struct ContentRead {
        result: Result<(), ExFatContentError>,
        chunks: Vec<Vec<u8>>,
        reads: Vec<(u64, usize)>,
    }

    impl ContentRead {
        fn bytes(&self) -> Vec<u8> {
            self.chunks.concat()
        }

        /// Reads that reached the cluster heap, as `(offset, length)`.
        fn data_reads(&self) -> Vec<(u64, usize)> {
            self.reads
                .iter()
                .copied()
                .filter(|&(offset, _)| offset >= HEAP_BYTE_OFFSET)
                .collect()
        }
    }

    fn read_content_of(
        image: &mut MemoryImage,
        boot: &ExFatBootSector,
        file: &ExFatFile,
    ) -> ContentRead {
        let mut recording = Recording {
            image,
            reads: Vec::new(),
        };
        let mut chunks = Vec::new();
        let result = ExFatReader::new(&mut recording, boot, extent(), Limits::DEFAULT)
            .read_content(file, &mut |chunk| chunks.push(chunk.to_vec()));
        ContentRead {
            result,
            chunks,
            reads: recording.reads,
        }
    }

    fn read_file(vol: &mut Vol, file: &ExFatFile) -> ContentRead {
        let boot = vol.boot.clone();
        read_content_of(&mut vol.image, &boot, file)
    }

    /// A live regular file, built directly rather than read from an entry.
    fn content_file(first: u32, valid: u64, length: u64, no_fat_chain: bool) -> ExFatFile {
        ExFatFile {
            name: utf16("f"),
            attributes: 0x20,
            first_cluster: first,
            data_length: length,
            valid_data_length: valid,
            no_fat_chain,
        }
    }

    /// Bytes that say which cluster they were read from, and are never zero.
    fn cluster_data(cluster: u32) -> Vec<u8> {
        vec![cluster as u8; 512]
    }

    fn fill_clusters(vol: &mut Vol, clusters: &[u32]) {
        for &cluster in clusters {
            vol.image
                .write(Vol::cluster_offset(cluster), &cluster_data(cluster));
        }
    }

    fn data_of(clusters: &[u32]) -> Vec<u8> {
        clusters.iter().flat_map(|&c| cluster_data(c)).collect()
    }

    /// Asserts every read of the heap lies inside one of `allowed`, each a
    /// cluster and the bytes of it that may be read.
    fn assert_reads_within(read: &ContentRead, allowed: &[(u32, u64)]) {
        for &(offset, length) in &read.data_reads() {
            let end = offset + length as u64;
            assert!(
                allowed.iter().any(|&(cluster, bytes)| {
                    let start = Vol::cluster_offset(cluster);
                    offset >= start && end <= start + bytes
                }),
                "read of {length} bytes at {offset} is outside what may be read"
            );
        }
    }

    #[test]
    fn a_contiguous_file_is_read_as_one_run_and_its_fat_entries_are_ignored() {
        let mut vol = Vol::new();
        let clusters = [10, 11, 12, 13];
        fill_clusters(&mut vol, &clusters);
        // Entries that would refuse the file if they were consulted.
        for &cluster in &clusters {
            vol.fat(cluster, FAT_BAD_CLUSTER);
        }

        let read = read_file(&mut vol, &content_file(10, 2048, 2048, true));
        read.result.as_ref().expect("the run is read");
        assert_eq!(read.bytes(), data_of(&clusters));
        assert_eq!(
            read.reads.len(),
            read.data_reads().len(),
            "the FAT was read"
        );
    }

    #[test]
    fn a_chain_in_four_out_of_order_runs_is_read_in_chain_order() {
        let mut vol = Vol::new();
        let chain = [20, 21, 5, 6, 40, 12, 13];
        vol.chain(&chain);
        fill_clusters(&mut vol, &chain);

        let read = read_file(&mut vol, &content_file(20, 7 * 512, 7 * 512, false));
        read.result.as_ref().expect("the chain is read");
        assert_eq!(read.bytes(), data_of(&chain));
        let allowed: Vec<(u32, u64)> = chain.iter().map(|&c| (c, 512)).collect();
        assert_reads_within(&read, &allowed);
    }

    #[test]
    fn a_data_length_that_is_not_whole_clusters_reads_only_its_bytes() {
        let mut vol = Vol::new();
        let chain = [7, 8, 9];
        vol.chain(&chain);
        fill_clusters(&mut vol, &chain);

        let read = read_file(&mut vol, &content_file(7, 1124, 1124, false));
        read.result.as_ref().expect("the chain is read");
        assert_eq!(read.bytes(), data_of(&chain)[..1124]);
        assert_reads_within(&read, &[(7, 512), (8, 512), (9, 100)]);
    }

    /// Valid data ends 200 bytes into the second of three clusters. Every
    /// byte past it holds a non-zero sentinel, so a byte read past it would
    /// show in the output as well as in the reads.
    #[test]
    fn bytes_past_valid_data_length_are_zeros_and_never_read() {
        let mut vol = Vol::new();
        let chain = [30, 31, 32];
        vol.chain(&chain);
        fill_clusters(&mut vol, &chain);

        let read = read_file(&mut vol, &content_file(30, 712, 1536, false));
        read.result.as_ref().expect("the chain is read");

        let valid_end = Vol::cluster_offset(31) + 200;
        for &(offset, length) in &read.data_reads() {
            assert!(
                offset + length as u64 <= valid_end || offset >= Vol::cluster_offset(32) + 512,
                "read of {length} bytes at {offset} reaches valid data length"
            );
        }
        assert_reads_within(&read, &[(30, 512), (31, 200)]);

        let bytes = read.bytes();
        assert_eq!(bytes.len(), 1536);
        assert_eq!(bytes[..512], cluster_data(30));
        assert_eq!(bytes[512..712], cluster_data(31)[..200]);
        assert!(bytes[712..].iter().all(|&b| b == 0));
    }

    /// The chain is still checked whole, and the zeros come in chunks no
    /// larger than one: 200 clusters is more than one chunk.
    #[test]
    fn a_valid_data_length_of_zero_reads_no_data_and_hands_over_zeros_in_chunks() {
        let mut vol = Vol::new();
        let chain: Vec<u32> = (2..=201).collect();
        vol.chain(&chain);
        fill_clusters(&mut vol, &chain);

        let read = read_file(&mut vol, &content_file(2, 0, 200 * 512, false));
        read.result.as_ref().expect("the chain is well formed");
        assert!(read.data_reads().is_empty(), "{:?}", read.data_reads());
        assert!(!read.reads.is_empty(), "the chain was not followed");

        assert_eq!(read.chunks.len(), 2);
        assert!(read.chunks.iter().all(|c| c.len() <= CONTENT_CHUNK_BYTES));
        let bytes = read.bytes();
        assert_eq!(bytes.len(), 200 * 512);
        assert!(bytes.iter().all(|&b| b == 0));
    }

    #[test]
    fn a_data_length_of_zero_touches_nothing() {
        for first in [0, 5] {
            let mut vol = Vol::new();
            let read = read_file(&mut vol, &content_file(first, 0, 0, false));

            read.result.as_ref().expect("an empty file reads");
            assert!(read.reads.is_empty(), "first cluster {first}");
            assert!(read.chunks.is_empty(), "first cluster {first}");
        }
    }

    /// Reads a chain file of `clusters` clusters from `first` and returns
    /// its error, asserting no data was read and nothing handed over.
    fn chain_error(vol: &mut Vol, first: u32, clusters: u64) -> ExFatContentError {
        let read = read_file(
            vol,
            &content_file(first, clusters * 512, clusters * 512, false),
        );
        assert!(read.chunks.is_empty(), "bytes were handed over");
        assert!(read.data_reads().is_empty(), "data was read");
        read.result.expect_err("the chain should be refused")
    }

    #[test]
    fn a_chain_that_ends_early_is_refused_at_its_end() {
        let mut vol = Vol::new();
        vol.chain(&[50, 51]);
        fill_clusters(&mut vol, &[50, 51]);

        assert!(matches!(
            chain_error(&mut vol, 50, 3),
            ExFatContentError::ChainEndsEarly {
                cluster: 51,
                needed: 3
            }
        ));
    }

    #[test]
    fn a_chain_that_goes_on_past_the_data_is_refused_at_its_last_needed_cluster() {
        let mut vol = Vol::new();
        vol.chain(&[50, 51, 52]);
        fill_clusters(&mut vol, &[50, 51, 52]);

        assert!(matches!(
            chain_error(&mut vol, 50, 2),
            ExFatContentError::ChainLongerThanData {
                cluster: 51,
                needed: 2
            }
        ));
    }

    #[test]
    fn a_cycle_is_refused_and_nothing_is_handed_over() {
        let mut vol = Vol::new();
        vol.fat(2, 3);
        vol.fat(3, 2);
        fill_clusters(&mut vol, &[2, 3]);

        assert!(matches!(
            chain_error(&mut vol, 2, 2),
            ExFatContentError::ChainLongerThanData {
                cluster: 3,
                needed: 2
            }
        ));
        // However many clusters the length claims, the end is never reached.
        assert!(matches!(
            chain_error(&mut vol, 2, 5),
            ExFatContentError::ChainLongerThanData {
                cluster: 2,
                needed: 5
            }
        ));
    }

    #[test]
    fn a_link_to_a_bad_cluster_is_refused() {
        let mut vol = Vol::new();
        vol.fat(60, 61);
        vol.fat(61, FAT_BAD_CLUSTER);
        fill_clusters(&mut vol, &[60, 61]);

        assert!(matches!(
            chain_error(&mut vol, 60, 2),
            ExFatContentError::BadCluster { cluster: 61 }
        ));
    }

    #[test]
    fn a_link_outside_the_cluster_heap_is_refused() {
        let mut vol = Vol::new();
        vol.fat(60, CLUSTER_COUNT + 2);
        fill_clusters(&mut vol, &[60]);

        assert!(matches!(
            chain_error(&mut vol, 60, 2),
            ExFatContentError::LinkOutOfRange { from: 60, to } if to == CLUSTER_COUNT + 2
        ));
    }

    /// A FAT of one sector holds entries for clusters 0 to 127 only.
    #[test]
    fn a_fat_entry_outside_the_fat_is_refused() {
        let mut vol = Vol::new();
        vol.boot.fat_length = 1;
        vol.chain(&[100, 150]);
        fill_clusters(&mut vol, &[100, 150]);

        assert!(matches!(
            chain_error(&mut vol, 100, 2),
            ExFatContentError::FatOutOfRange { cluster: 150 }
        ));
    }

    #[test]
    fn a_directory_is_refused() {
        let mut vol = Vol::new();
        vol.chain(&[70]);
        let mut file = content_file(70, 512, 512, false);
        file.attributes = ATTRIBUTE_DIRECTORY;

        let read = read_file(&mut vol, &file);
        assert!(matches!(read.result, Err(ExFatContentError::Directory)));
        assert!(read.reads.is_empty());
        assert!(read.chunks.is_empty());
    }

    /// A caller can build a file the walk would have refused; it is refused
    /// here too, before a cluster number outside the heap is used.
    #[test]
    fn an_allocation_a_listing_would_refuse_is_refused() {
        let mut vol = Vol::new();
        for file in [
            content_file(1, 512, 512, false),
            content_file(201, 1024, 1024, true),
            content_file(10, 1024, 512, false),
        ] {
            let read = read_file(&mut vol, &file);
            assert!(
                matches!(read.result, Err(ExFatContentError::Allocation(_))),
                "{file:?}"
            );
            assert!(read.reads.is_empty());
        }
    }

    /// Clusters of 128 KiB, the default above 32 GB: each is read in pieces
    /// no larger than a chunk, and the zeros after valid data likewise.
    #[test]
    fn a_cluster_larger_than_a_chunk_is_read_in_chunks() {
        let cluster_bytes: u64 = 128 * 1024;
        let mut boot = boot();
        boot.sectors_per_cluster_shift = 8;
        boot.cluster_count = 4;
        boot.fat_length = 1;
        boot.volume_length = 64 + 4 * 256;
        let heap = HEAP_BYTE_OFFSET;
        let mut image = MemoryImage::new(heap + 4 * cluster_bytes);
        let pattern: Vec<u8> = (0..2 * cluster_bytes)
            .map(|i| (i % 251) as u8 + 1)
            .collect();
        image.write(heap, &pattern);

        let valid = cluster_bytes + 1000;
        let read = read_content_of(
            &mut image,
            &boot,
            &content_file(2, valid, 2 * cluster_bytes, true),
        );
        read.result.as_ref().expect("the run is read");

        assert_eq!(
            read.reads,
            [
                (heap, CONTENT_CHUNK_BYTES),
                (heap + CONTENT_CHUNK_BYTES as u64, CONTENT_CHUNK_BYTES),
                (heap + cluster_bytes, 1000),
            ]
        );
        assert!(read.chunks.iter().all(|c| c.len() <= CONTENT_CHUNK_BYTES));
        let bytes = read.bytes();
        assert_eq!(bytes.len() as u64, 2 * cluster_bytes);
        assert_eq!(bytes[..valid as usize], pattern[..valid as usize]);
        assert!(bytes[valid as usize..].iter().all(|&b| b == 0));
    }

    #[test]
    fn every_content_error_names_what_failed() {
        assert!(
            ExFatContentError::ChainLongerThanData {
                cluster: 77,
                needed: 2
            }
            .to_string()
            .contains("77")
        );
        assert!(
            ExFatContentError::ChainEndsEarly {
                cluster: 78,
                needed: 3
            }
            .to_string()
            .contains("78")
        );
        assert!(!ExFatContentError::Directory.to_string().is_empty());
    }
}
