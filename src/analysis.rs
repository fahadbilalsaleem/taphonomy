//! Runs the analysis of an opened FAT32 image and reports it as events.
//!
//! `ADR-0018`. An interface hashes the evidence and opens it, then hands
//! this module the image's length and an [`Options`] the operator chose.
//! [`run`] walks the partition table, the filesystems it finds, and every
//! directory reachable from a root or by the orphan search, and streams what
//! it finds to a [`Sink`] as one [`Event`] per finding. It returns the run's
//! [`RunCounts`] when it ends.
//!
//! Every line an interface prints from this run is rendered from one
//! event's fields. No event carries text already formatted for display; a
//! sink decides how to say what an event reports, not what to say.
//!
//! Hashing stays outside this module. `ADR-0018` Decision D: the caller
//! passes the image's length, measured however it measured it, so a test
//! that needs no digest can skip the whole-image read entirely.

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::path::Path;

use crate::Error;
use crate::evidence::EvidenceReader;
use crate::exfat_boot::{BootRegion, ExFatBootFailure, ExFatBootSector};
use crate::exfat_directory::{
    DirectorySummary, ExFatBitmapEntry, ExFatDirectoryError, ExFatFile, ExFatPosition, SetRefusal,
};
use crate::fat_directory::{
    DeletedDirectory, DeletedDirectoryRefusal, DeletedKind, Directory, DirectoryError, Entry,
    EntryKind, FirstByte, associate, begins_directory, enumerate_deleted_directory,
    enumerate_directory, enumerate_root, is_dot_entry, recovered_name,
};
use crate::fat_recovery::{
    Assessment, ClusterRun, Destination, Extraction, Ineligible, Listing, Output, RecoveryError,
    assess_listed, extract,
};
use crate::fat32::{Fat32BootSector, Fat32Error, parse_boot_sector};
use crate::filesystem::{
    Filesystem, Identification, UnknownReason, VBR_SIZE, VolumeExtent, declared_type_matches,
    identify,
};
use crate::hash::Sha256Digest;
use crate::partition::{Anomaly, MbrPartition, ParseError, PartitionTable, SECTOR_SIZE, parse_mbr};
use crate::validation::{Outcome, Validation, validate};

pub mod exfat;

use exfat::{ExFatNotRead, ExFatReached};

/// What the operator asked for.
///
/// The analysis functions take this rather than a widening list of flags.
/// `ADR-0013` section 13.
#[derive(Clone, Copy)]
pub struct Options<'a> {
    /// Read and hash the content of a deleted file whose run is free.
    ///
    /// `ADR-0010` Decision B: opt-in, because the default invocation
    /// reports what the volume states without reading any content.
    pub recover: bool,

    /// Digest of the file the operator is looking for, where they gave one.
    ///
    /// `ADR-0013` Decision A: the tool never discovers a reference, and
    /// this is the only way one enters. Decision H: it is an argument
    /// error without `recover`, because comparing needs content read.
    pub reference: Option<Sha256Digest>,

    /// Directory each recovered artifact is written to.
    ///
    /// `ADR-0015` Decision A: opt-in, and an argument error without
    /// `recover`, on the same grounds as `reference`. Decision B, as
    /// Appendix A.3 corrects it: checked against the evidence before the
    /// evidence is opened, so a destination that is the evidence never
    /// reaches a write.
    pub output: Option<&'a Path>,
}

/// How a reported directory was reached.
///
/// `ADR-0018`. A directory event names its own origin, so a sink can render
/// the heading a listing used to carry as literal text without being handed
/// that text.
#[derive(Clone, Copy, Debug)]
pub enum Reached<'a> {
    /// The volume's root directory.
    Root,

    /// Named by a listing the walk reached, at `path` from the root.
    ///
    /// `deleted` is whether the entry naming it is itself deleted.
    Walked { path: &'a str, deleted: bool },

    /// Found by the orphan search at `cluster`, whose `..` entry names
    /// `parent`, where one could be recovered.
    Orphaned { cluster: u32, parent: Option<u32> },
}

/// Which part of a directory an entry was read from.
///
/// `ADR-0018` follow-up. An entry past the terminator is residue: the
/// volume's current listing does not claim it, and it is kept rather
/// than discarded under `ADR-0009` Decision B. An examiner reads the
/// two differently, so this is a finding carried on every content
/// event, not a rendering choice left to a sink to reconstruct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectoryPart {
    /// Before the terminator, in the directory's current listing.
    Entries,

    /// Past the terminator: content the terminator does not claim.
    Residue,
}

/// Why a named directory's contents were not read.
#[derive(Clone, Copy, Debug)]
pub enum NotReadReason<'a> {
    /// Deeper below the root than [`MAX_DEPTH`].
    TooDeep,

    /// `cluster` was already read as a directory.
    AlreadyRead { cluster: u32 },

    /// `ADR-0016` Decision C refused it.
    Refused(&'a DeletedDirectoryRefusal),

    /// Reading or enumerating it failed.
    Enumeration(&'a DirectoryError),
}

/// One finding of a run, as it happened.
///
/// `ADR-0018` Decision B. Every line a CLI prints from `inspect` down to
/// `report_output` today is renderable from exactly one event's fields.
/// Events borrow library types rather than copy them, so a sink that only
/// counts what it saw allocates nothing.
pub enum Event<'a> {
    /// Sector 0 could not be read; the partition table was never seen.
    TableUnread(&'a Error),

    /// The partition table was read and parsing refused it.
    TableRejected(&'a ParseError),

    /// The disk signature of a conventional MBR.
    DiskSignature(u32),

    /// One partition the parsed table lists, in table order.
    PartitionListed(&'a MbrPartition),

    /// Every partition the parsed table lists has been listed. An entry
    /// left out as an anomaly is not among them.
    PartitionsListed,

    /// A GPT protective MBR, which this tool does not analyse.
    GptDetected,

    /// Interpretable but unusual observations on the partition table.
    /// Never empty.
    Anomalies(&'a [Anomaly]),

    /// A partition's first sector could not be read.
    PartitionUnread {
        partition: &'a MbrPartition,
        error: &'a Error,
    },

    /// What a partition's volume boot record identified.
    ///
    /// `type_match` is [`declared_type_matches`] of the partition's declared
    /// type against the identified filesystem: `Some(true)` or
    /// `Some(false)` where one was identified, `None` where none was, or
    /// where the declared type carries no expectation to compare against.
    PartitionIdentified {
        partition: &'a MbrPartition,
        identification: &'a Identification,
        type_match: Option<bool>,
    },

    /// A partition whose first sector identified nothing, identified as
    /// exFAT from its backup boot region.
    ///
    /// Recorded in place of [`Event::PartitionIdentified`], and followed by
    /// the volume's analysis, or by why the region found is not analysed.
    /// `sector_zero` is why the volume's first sector identified nothing.
    ExFatIdentifiedFromBackup {
        partition: &'a MbrPartition,
        sector_zero: &'a UnknownReason,
    },

    /// A FAT32 volume's boot sector was refused.
    BootSectorRejected(&'a Fat32Error),

    /// A FAT32 volume's validated boot sector.
    BootSector(&'a Fat32BootSector),

    /// A volume's root directory could not be enumerated.
    RootDirectoryUnread(&'a DirectoryError),

    /// A directory that was read, however it was reached.
    Directory {
        reached: Reached<'a>,
        directory: &'a Directory,
    },

    /// A named directory whose contents were not read.
    DirectoryNotRead {
        reached: Reached<'a>,
        reason: NotReadReason<'a>,
    },

    /// A directory's first cluster held no terminator, so its listing may
    /// continue where the evidence does not say.
    ListingMayContinue(u32),

    /// An evidence error stopped the orphan search at `cluster`; the
    /// clusters after it were never visited.
    OrphanSearchStopped {
        cluster: u32,
        error: &'a DirectoryError,
    },

    /// A directory an orphaned listing named was never found by the search.
    OrphanedListingUnread {
        holder: u32,
        name: &'a str,
        first_cluster: u32,
    },

    /// A deleted entry's assessment failed.
    ///
    /// `listing` and `part` are always present: which listing the entry
    /// was offered under, and which part of the directory it came from.
    /// Neither is a rendering choice; `ADR-0018` follow-up.
    NotAssessed {
        listing: Listing,
        part: DirectoryPart,
        entry: &'a Entry,
        error: &'a RecoveryError,
    },

    /// A deleted entry whose fields locate no content.
    Ineligible {
        listing: Listing,
        part: DirectoryPart,
        entry: &'a Entry,
        reason: Ineligible,
    },

    /// A deleted entry whose implied run holds an allocated cluster.
    RunBroken {
        listing: Listing,
        part: DirectoryPart,
        entry: &'a Entry,
        run: ClusterRun,
        first_allocated: u32,
    },

    /// A deleted entry whose implied run is free.
    Recoverable {
        listing: Listing,
        part: DirectoryPart,
        entry: &'a Entry,
        run: ClusterRun,
    },

    /// A free run that was read, and what became of it.
    ///
    /// `readback_matches` is `Some` exactly where `extraction.output` is
    /// [`Output::Written`], and then states whether the bytes read back
    /// from the destination equal `extraction.digest`. `None` for every
    /// other outcome, including no destination at all. `listing` and
    /// `part` are as on [`Event::Recoverable`], the event this always
    /// follows.
    Extracted {
        listing: Listing,
        part: DirectoryPart,
        entry: &'a Entry,
        extraction: &'a Extraction,
        validation: Validation,
        readback_matches: Option<bool>,
    },

    /// A free run whose extraction failed. `listing` and `part` are as on
    /// [`Event::Recoverable`], the event this always follows.
    NotExtracted {
        listing: Listing,
        part: DirectoryPart,
        entry: &'a Entry,
        error: &'a RecoveryError,
    },

    /// An exFAT volume's validated boot region, and which region it came
    /// from. `ADR-0019` section 3.
    ExFatBootRegion(&'a ExFatBootSector),

    /// An exFAT volume's boot regions were refused, or a valid backup was
    /// found at more than one sector size.
    ExFatBootRejected(&'a ExFatBootFailure),

    /// A valid exFAT boot region declaring two FATs: TexFAT, which is
    /// identified and not analysed. The region it was read from.
    ExFatTexFat(BootRegion),

    /// An exFAT volume's root directory could not be read, wholly or in part.
    ExFatRootDirectoryUnread(&'a ExFatDirectoryError),

    /// An exFAT directory is about to be read. Its entries follow, one event
    /// each, then [`Event::ExFatDirectoryEnd`] or
    /// [`Event::ExFatRootDirectoryUnread`] or [`Event::ExFatDirectoryNotRead`].
    ExFatDirectoryStart { reached: ExFatReached<'a> },

    /// A live file or directory in an exFAT directory. The name is UTF-16
    /// code units as stored.
    ExFatEntry {
        position: ExFatPosition,
        file: &'a ExFatFile,
    },

    /// An exFAT volume's label, as UTF-16 code units.
    ExFatVolumeLabel {
        position: ExFatPosition,
        label: &'a [u16],
    },

    /// An exFAT volume's allocation bitmap entry. Its contents are not read.
    ExFatBitmap {
        position: ExFatPosition,
        bitmap: &'a ExFatBitmapEntry,
    },

    /// An exFAT entry or entry set that was not used, and why. Counted as a
    /// gap.
    ExFatSetRefused {
        position: ExFatPosition,
        refusal: &'a SetRefusal,
    },

    /// An exFAT directory was read to its end.
    ExFatDirectoryEnd {
        reached: ExFatReached<'a>,
        summary: &'a DirectorySummary,
    },

    /// A named exFAT directory whose contents were not read, wholly or in
    /// part. Anything printed since its [`Event::ExFatDirectoryStart`] was
    /// read before the failure.
    ExFatDirectoryNotRead {
        reached: ExFatReached<'a>,
        reason: ExFatNotRead<'a>,
    },

    /// The volume held more directory clusters than the cap allows reading.
    /// `directories_unread` directories were counted and not read.
    ExFatDirectoryCapReached { cap: u32, directories_unread: usize },

    /// More exFAT directories were named than the queue may hold. The
    /// `directories_unread` that would have exceeded `bound` were counted as
    /// not read and never queued.
    ExFatQueueBoundReached {
        bound: usize,
        directories_unread: usize,
    },
}

/// Receives the events of one run, in the order they happened.
///
/// `ADR-0018` Decision B. The library reports; a sink decides how, or
/// whether, to render what it is given. The CLI's sink prints. A sink that
/// only counts, or that collects events into a tree, is a few lines.
pub trait Sink {
    /// Records one event.
    fn record(&mut self, event: Event<'_>);
}

/// Deepest level below the root at which a directory is read.
///
/// `ADR-0016` Decision E. Termination is guaranteed by reading no cluster
/// twice; this bounds output, which a chain of nested directories each
/// reported under its full path would otherwise grow quadratically. The
/// Sleuth Kit's directory walk uses the same number. A DCF card nests two.
pub const MAX_DEPTH: usize = 128;

/// The MBR partition type NTFS and exFAT both use. `ADR-0019` section 3: the
/// only declared type for which a partition nothing identified is tried as
/// exFAT.
const EXFAT_OR_NTFS_TYPE: u8 = 0x07;

/// A directory named by an entry and not yet read.
struct Pending {
    /// First cluster, from the entry that names it.
    first_cluster: u32,

    /// Path of recovered names from the root, for the report only.
    path: String,

    /// Levels below the root.
    depth: usize,

    /// Whether the entry naming it is deleted.
    deleted: bool,
}

/// What one run covered, produced, and left unanalysed.
///
/// One value accumulates for the whole run, because `ADR-0003` section 4.7
/// requires a statement about a session rather than about a volume. Its
/// fields are private: nothing outside this module can change a count, only
/// read one back through the methods below. `ADR-0018` Decision C.
///
/// Only independently observed facts are stored. Everything that follows
/// from them, including the caveats `main.rs` used to carry as three
/// booleans, is derived below. `ADR-0014` Appendix B.8.
#[derive(Default, Debug)]
pub struct RunCounts {
    /// Volumes whose root directory was enumerated.
    volumes_analysed: usize,

    /// Sector 0 could not be read, so no table was seen at all.
    table_unread: usize,

    /// The partition table was read and refused.
    table_rejected: usize,

    /// A partition entry rejected as malformed, zero length or beyond the
    /// image, and not analysed. The table's other entries were.
    partitions_anomalous: usize,

    /// A GPT disk, which this tool does not analyse.
    gpt: usize,

    /// A partition whose first sector could not be read.
    partition_unread: usize,

    /// A partition holding no filesystem this tool could identify.
    unidentified: usize,

    /// A partition whose filesystem was identified and is neither FAT32 nor
    /// exFAT, or is TexFAT, which is exFAT with two FATs.
    unsupported: usize,

    /// A FAT32 or exFAT volume whose boot sector or boot regions were
    /// refused.
    boot_rejected: usize,

    /// A FAT32 or exFAT volume whose root directory could not be enumerated.
    root_unread: usize,

    /// A directory that was named and whose contents were not read.
    ///
    /// `ADR-0016` narrows it to a deleted directory whose first cluster did
    /// not identify itself, a live chain that could not be walked, and a
    /// directory beyond the depth bound or already read.
    directories_unread: usize,

    /// A deleted directory whose first cluster was read and holds no
    /// terminator, so its listing may continue where the evidence does not
    /// say. `ADR-0016` Decision D.
    listings_may_continue: usize,

    /// A volume whose orphan search an evidence error stopped, so the
    /// clusters after it were never visited. `ADR-0017` Decision D.
    orphan_search_stopped: usize,

    /// A deleted entry whose assessment failed.
    not_assessed: usize,

    /// A deleted entry whose run was free and whose extraction failed.
    not_extracted: usize,

    /// An artifact that was read and could not be handed to the operator.
    ///
    /// `ADR-0015` Decision G and section 9: the digest stands, because the
    /// destination has no say in what the evidence holds, but the operator
    /// asked for a file and has none. A written file that could not be read
    /// back is not counted here, because the file is there.
    not_delivered: usize,

    /// A deleted entry whose implied run was free in the FAT.
    free_runs: usize,

    /// A deleted entry whose run holds a cluster still in use.
    refused: usize,

    /// An extraction whose digest equalled the operator's reference.
    matched: usize,

    /// A deleted entry stating a size of zero.
    empty: usize,

    /// A deleted entry naming cluster 0 or 1.
    reserved_cluster: usize,

    /// A deleted entry whose run would pass the last data cluster.
    run_out_of_range: usize,

    /// An exFAT entry or entry set that was not used: its checksum was
    /// wrong, it was malformed, or it was not recognised. `ADR-0019`
    /// Decision C.
    exfat_sets_refused: usize,

    /// An unused exFAT File entry: a deleted entry set, which this tool does
    /// not examine. One per set, so a run cannot report an exFAT volume
    /// completely analysed while skipping them. `SAFETY.md` section 12.
    exfat_deleted_sets_not_examined: usize,

    /// An exFAT directory with entries of a type other than zero after its
    /// end marker, which the specification says it does not have. One per
    /// directory. The entries are counted and not interpreted.
    exfat_end_marker_residue: usize,
}

impl RunCounts {
    /// Volumes whose root directory was enumerated.
    pub const fn volumes_analysed(&self) -> usize {
        self.volumes_analysed
    }

    /// Sector 0 could not be read, so no table was seen at all.
    pub const fn table_unread(&self) -> usize {
        self.table_unread
    }

    /// The partition table was read and refused.
    pub const fn table_rejected(&self) -> usize {
        self.table_rejected
    }

    /// A partition entry rejected as malformed, zero length or beyond the
    /// image, and not analysed. The table's other entries were.
    pub const fn partitions_anomalous(&self) -> usize {
        self.partitions_anomalous
    }

    /// A GPT disk, which this tool does not analyse.
    pub const fn gpt(&self) -> usize {
        self.gpt
    }

    /// A partition whose first sector could not be read.
    pub const fn partition_unread(&self) -> usize {
        self.partition_unread
    }

    /// A partition holding no filesystem this tool could identify.
    pub const fn unidentified(&self) -> usize {
        self.unidentified
    }

    /// A partition whose filesystem was identified and is neither FAT32 nor
    /// exFAT, or is TexFAT, which is exFAT with two FATs.
    pub const fn unsupported(&self) -> usize {
        self.unsupported
    }

    /// A FAT32 or exFAT volume whose boot sector or boot regions were
    /// refused.
    pub const fn boot_rejected(&self) -> usize {
        self.boot_rejected
    }

    /// A FAT32 or exFAT volume whose root directory could not be enumerated.
    pub const fn root_unread(&self) -> usize {
        self.root_unread
    }

    /// A directory that was named and whose contents were not read.
    pub const fn directories_unread(&self) -> usize {
        self.directories_unread
    }

    /// A deleted directory read with no terminator in its first cluster.
    pub const fn listings_may_continue(&self) -> usize {
        self.listings_may_continue
    }

    /// A volume whose orphan search an evidence error stopped.
    pub const fn orphan_search_stopped(&self) -> usize {
        self.orphan_search_stopped
    }

    /// A deleted entry whose assessment failed.
    pub const fn not_assessed(&self) -> usize {
        self.not_assessed
    }

    /// A deleted entry whose run was free and whose extraction failed.
    pub const fn not_extracted(&self) -> usize {
        self.not_extracted
    }

    /// An artifact that was read and could not be handed to the operator.
    pub const fn not_delivered(&self) -> usize {
        self.not_delivered
    }

    /// A deleted entry whose run holds a cluster still in use.
    pub const fn refused(&self) -> usize {
        self.refused
    }

    /// An extraction whose digest equalled the operator's reference.
    pub const fn matched(&self) -> usize {
        self.matched
    }

    /// A deleted entry stating a size of zero.
    pub const fn empty(&self) -> usize {
        self.empty
    }

    /// A deleted entry naming cluster 0 or 1.
    pub const fn reserved_cluster(&self) -> usize {
        self.reserved_cluster
    }

    /// A deleted entry whose run would pass the last data cluster.
    pub const fn run_out_of_range(&self) -> usize {
        self.run_out_of_range
    }

    /// A deleted entry whose implied run was free in the FAT.
    pub const fn free_runs(&self) -> usize {
        self.free_runs
    }

    /// An exFAT entry or entry set that was not used.
    pub const fn exfat_sets_refused(&self) -> usize {
        self.exfat_sets_refused
    }

    /// An unused exFAT File entry, a deleted set this tool did not examine.
    pub const fn exfat_deleted_sets_not_examined(&self) -> usize {
        self.exfat_deleted_sets_not_examined
    }

    /// An exFAT directory holding entries after its end marker.
    pub const fn exfat_end_marker_residue(&self) -> usize {
        self.exfat_end_marker_residue
    }

    /// Everything the run did not analyse, counted once per occurrence.
    ///
    /// The eleven kinds `ADR-0014` Appendix B.4 enumerates, the twelfth
    /// `ADR-0015` section 9 adds, the thirteenth `ADR-0016` Decision D adds,
    /// the fourteenth `ADR-0017` Decision D adds, a partition entry dropped
    /// as malformed (`ADR-0014` Appendix D), and the three exFAT kinds
    /// `ADR-0014` Appendix E adds. Not every kind is reached by a fixture,
    /// and Appendices B.4 and E record which of theirs are; several are
    /// reached only by unit tests, which is why the derivation below is unit
    /// tested rather than measured alone.
    const fn gaps(&self) -> usize {
        self.table_unread
            + self.table_rejected
            + self.partitions_anomalous
            + self.gpt
            + self.partition_unread
            + self.unidentified
            + self.unsupported
            + self.boot_rejected
            + self.root_unread
            + self.not_assessed
            + self.not_extracted
            + self.not_delivered
            + self.directories_unread
            + self.listings_may_continue
            + self.orphan_search_stopped
            + self.exfat_sets_refused
            + self.exfat_deleted_sets_not_examined
            + self.exfat_end_marker_residue
    }

    /// How much of the evidence the run covered.
    ///
    /// `ADR-0014` Appendix B.5. A run that analysed nothing is kept apart
    /// from one that analysed part of the evidence, because `SAFETY.md`
    /// section 12 forbids a failure being converted silently into a partial
    /// success. An image declaring no partition reaches `Complete` with no
    /// volume analysed, which is the control Appendix A.6 names.
    pub const fn coverage(&self) -> Coverage {
        if self.gaps() == 0 {
            Coverage::Complete
        } else if self.volumes_analysed == 0 {
            Coverage::NothingAnalysed
        } else {
            Coverage::Incomplete
        }
    }

    /// Artifacts produced, which only `--recover` can produce.
    ///
    /// Every free run is either extracted or counted as not extracted, so
    /// this follows from the two and is not tracked beside them. Saturating
    /// because a count that disagrees with that invariant must not panic in
    /// the middle of reporting evidence.
    pub const fn artifacts(&self, options: Options<'_>) -> usize {
        if options.recover {
            self.free_runs.saturating_sub(self.not_extracted)
        } else {
            0
        }
    }

    /// Free runs left unread because the operator did not ask to read them.
    pub const fn not_read(&self, options: Options<'_>) -> usize {
        if options.recover {
            return 0;
        }

        self.free_runs
    }

    /// Extractions compared against a reference that did not equal it.
    pub const fn differed(&self, options: Options<'_>) -> usize {
        if options.reference.is_some() {
            self.artifacts(options).saturating_sub(self.matched)
        } else {
            0
        }
    }
}

/// How much of the evidence a run analysed.
///
/// Three values rather than two: `ADR-0014` Appendix B.5 and B.6. The word
/// names what it measures, because a run can cover everything and still
/// recover content that is not the file's, which EXP-0004 measured.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Coverage {
    /// Nothing went unanalysed.
    Complete,

    /// Part of the evidence was analysed and part was not.
    Incomplete,

    /// Nothing past the evidence digest was analysed.
    ///
    /// Named for what it says rather than `None`, which would shadow
    /// `Option::None` wherever this is matched.
    NothingAnalysed,
}

impl fmt::Display for Coverage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Complete => f.write_str("complete"),
            Self::Incomplete => f.write_str("incomplete"),
            Self::NothingAnalysed => f.write_str("none"),
        }
    }
}

/// Runs the analysis of an opened image and reports it to `sink`.
///
/// `length` is the image's size in bytes, as the caller measured it.
/// `ADR-0018` Decision D: hashing stays outside this function, so a test
/// that needs no digest can pass the file's reported size and skip the
/// whole-image read.
pub fn run<R: EvidenceReader>(
    evidence: &mut R,
    length: u64,
    options: Options<'_>,
    sink: &mut dyn Sink,
) -> RunCounts {
    let mut counts = RunCounts::default();
    let mut sector = [0u8; SECTOR_SIZE];

    match evidence.read_exact_at(0, &mut sector) {
        Ok(()) => {
            let image_sectors = length / SECTOR_SIZE as u64;

            match parse_mbr(&sector, image_sectors) {
                Ok(outcome) => {
                    match &outcome.table {
                        PartitionTable::Mbr {
                            disk_signature,
                            partitions,
                        } => {
                            sink.record(Event::DiskSignature(*disk_signature));
                            for p in partitions {
                                sink.record(Event::PartitionListed(p));
                            }
                            sink.record(Event::PartitionsListed);

                            for p in partitions {
                                report_partition(evidence, p, options, &mut counts, sink);
                            }
                        }
                        PartitionTable::GptProtective => {
                            sink.record(Event::GptDetected);
                            counts.gpt += 1;
                        }
                    }

                    // A dropped entry is an anomaly of the table and a gap
                    // in the run: a partition was declared and not analysed.
                    counts.partitions_anomalous += outcome
                        .anomalies
                        .iter()
                        .filter(|a| {
                            matches!(
                                a,
                                Anomaly::PartitionBeyondEnd { .. }
                                    | Anomaly::ZeroLengthPartition { .. }
                            )
                        })
                        .count();

                    if !outcome.anomalies.is_empty() {
                        sink.record(Event::Anomalies(&outcome.anomalies));
                    }
                }
                Err(e) => {
                    sink.record(Event::TableRejected(&e));
                    counts.table_rejected += 1;
                }
            }
        }
        Err(e) => {
            sink.record(Event::TableUnread(&e));
            counts.table_unread += 1;
        }
    }

    counts
}

/// Reports the filesystem found in one partition.
///
/// A read or identification failure for one partition is reported and does
/// not stop the others: hashing already succeeded and that result stands on
/// its own. Each is counted, so that the coverage line states what the run
/// did not analyse.
fn report_partition<R: EvidenceReader>(
    evidence: &mut R,
    p: &MbrPartition,
    options: Options<'_>,
    counts: &mut RunCounts,
    sink: &mut dyn Sink,
) {
    let mut vbr = [0u8; VBR_SIZE];
    if let Err(e) = evidence.read_exact_at(p.start_byte(), &mut vbr) {
        sink.record(Event::PartitionUnread {
            partition: p,
            error: &e,
        });
        counts.partition_unread += 1;
        return;
    }

    let id = identify(&vbr);
    let extent = VolumeExtent {
        start_lba: p.start_lba,
        sector_count: p.sector_count,
    };

    // `ADR-0019` section 3. An exFAT volume keeps a second copy of its boot
    // region, so a damaged first sector can hide one. Only a partition the
    // table declares as 0x07, which NTFS also uses, is tried, and a real
    // NTFS volume never gets here because its first sector identifies it.
    // It is tried before the partition's heading is recorded, because what
    // it finds decides what the heading says.
    if let Identification::Unknown { reason } = &id
        && p.partition_type == EXFAT_OR_NTFS_TYPE
        && exfat::report_unidentified_0x07(evidence, p, reason, extent, counts, sink)
    {
        return;
    }

    let type_match = id
        .filesystem()
        .and_then(|filesystem| declared_type_matches(p.partition_type, filesystem));
    sink.record(Event::PartitionIdentified {
        partition: p,
        identification: &id,
        type_match,
    });

    // Two different gaps, separated because `CLAUDE.md` section 14 requires
    // an unsupported format and unreadable metadata to be told apart: a
    // volume whose filesystem was identified and not analysed is not the
    // same finding as one nothing could identify.
    match id.filesystem() {
        Some(Filesystem::Fat32) => {}
        Some(Filesystem::ExFat) => {
            exfat::report_identified(evidence, p, extent, counts, sink);
            return;
        }
        Some(_) => {
            counts.unsupported += 1;
            return;
        }
        None => {
            counts.unidentified += 1;
            return;
        }
    }

    match parse_boot_sector(&vbr, extent) {
        Ok(boot) => {
            sink.record(Event::BootSector(&boot));
            report_root_directory(evidence, &boot, extent, p.index, options, counts, sink);
        }
        Err(e) => {
            sink.record(Event::BootSectorRejected(&e));
            counts.boot_rejected += 1;
        }
    }
}

/// Reports the root directory of a FAT32 volume.
///
/// An enumeration failure is reported and does not stop the other
/// partitions, for the same reason a read failure does not: hashing already
/// succeeded and that result stands on its own. It is counted, and where no
/// volume was analysed it leaves the run's coverage `none`.
fn report_root_directory<R: EvidenceReader>(
    evidence: &mut R,
    boot: &Fat32BootSector,
    extent: VolumeExtent,
    partition: u8,
    options: Options<'_>,
    counts: &mut RunCounts,
    sink: &mut dyn Sink,
) {
    let root = match enumerate_root(evidence, boot, extent) {
        Ok(root) => root,
        Err(e) => {
            sink.record(Event::RootDirectoryUnread(&e));
            counts.root_unread += 1;
            return;
        }
    };

    counts.volumes_analysed += 1;

    // `ADR-0016` Decision E. Every cluster read as a directory, for the
    // whole volume, so no directory is read twice and the walk terminates.
    let mut seen: HashSet<u32> = root.clusters.iter().copied().collect();
    let mut pending: VecDeque<Pending> = VecDeque::new();

    // `ADR-0017` Decision A. Every first cluster the walk was given, read or
    // declined, so the search never reverses one of its refusals.
    let mut named: HashSet<u32> = HashSet::new();

    sink.record(Event::Directory {
        reached: Reached::Root,
        directory: &root,
    });
    queue_subdirectories(&root, "", 0, &mut pending);
    report_recovery(
        evidence,
        boot,
        extent,
        partition,
        &root.entries,
        Listing::Walked,
        DirectoryPart::Entries,
        options,
        counts,
        sink,
    );
    report_recovery(
        evidence,
        boot,
        extent,
        partition,
        &root.residue,
        Listing::Walked,
        DirectoryPart::Residue,
        options,
        counts,
        sink,
    );

    while let Some(next) = pending.pop_front() {
        named.insert(next.first_cluster);
        report_subdirectory(
            evidence,
            boot,
            extent,
            partition,
            next,
            &mut seen,
            &mut pending,
            options,
            counts,
            sink,
        );
    }

    search_orphaned_directories(
        evidence, boot, extent, partition, &mut seen, &named, options, counts, sink,
    );
}

/// Searches every data cluster the walk did not name for a directory, and
/// reads and reports each one found.
///
/// `ADR-0017` Decisions A, B and D. A cluster is read only if it begins with
/// `.` naming itself and `..`, and then only under `ADR-0016` Decision C, so
/// a cluster the FAT marks in use is passed over. Each directory found is
/// reported on its own and never followed: every directory the search can
/// find carries its own signature, so the search visits it whatever names
/// it. A directory an orphaned listing names and the run never read is a
/// gap, checked once the search has visited every cluster.
#[allow(clippy::too_many_arguments)]
fn search_orphaned_directories<R: EvidenceReader>(
    evidence: &mut R,
    boot: &Fat32BootSector,
    extent: VolumeExtent,
    partition: u8,
    seen: &mut HashSet<u32>,
    named: &HashSet<u32>,
    options: Options<'_>,
    counts: &mut RunCounts,
    sink: &mut dyn Sink,
) {
    // Directories the orphaned listings name: the cluster of the listing
    // that holds the entry, the name, and the first cluster it gives.
    let mut listed: Vec<(u32, String, u32)> = Vec::new();

    for cluster in boot.geometry.data_clusters() {
        if seen.contains(&cluster) || named.contains(&cluster) {
            continue;
        }

        let read = match begins_directory(evidence, boot, extent, cluster) {
            Ok(false) => continue,
            Ok(true) => enumerate_deleted_directory(evidence, boot, extent, cluster),
            Err(e) => Err(e),
        };

        let (directory, may_continue) = match read {
            Ok(DeletedDirectory::Read {
                listing,
                may_continue,
            }) => (listing, may_continue),
            // Decision C refused what the first test passed: in use, most
            // likely. It is not a directory the run was given, so not a gap.
            Ok(DeletedDirectory::Refused(_)) => continue,
            Err(e) => {
                // `ADR-0017` Decision D. The clusters after this one were
                // never visited, so nothing about them can be stated.
                sink.record(Event::OrphanSearchStopped { cluster, error: &e });
                counts.orphan_search_stopped += 1;
                return;
            }
        };

        seen.insert(cluster);

        let parent = match directory.entries.get(1).map(|e| &e.kind) {
            Some(EntryKind::ShortName { first_cluster, .. }) => Some(*first_cluster),
            _ => None,
        };

        sink.record(Event::Directory {
            reached: Reached::Orphaned { cluster, parent },
            directory: &directory,
        });

        if may_continue {
            sink.record(Event::ListingMayContinue(cluster));
            counts.listings_may_continue += 1;
        }

        for slots in [&directory.entries, &directory.residue] {
            for (index, entry) in slots.iter().enumerate() {
                if is_dot_entry(&entry.kind) {
                    continue;
                }
                let (first_cluster, name) = match &entry.kind {
                    EntryKind::ShortName {
                        name,
                        directory: true,
                        first_cluster,
                        ..
                    } => (*first_cluster, name.clone()),
                    EntryKind::Deleted {
                        was:
                            DeletedKind::ShortName {
                                surviving_name,
                                directory: true,
                                first_cluster,
                                ..
                            },
                    } => (
                        *first_cluster,
                        deleted_segment(slots, index, surviving_name),
                    ),
                    _ => continue,
                };
                let name = name.unwrap_or_else(|| format!("c{first_cluster}"));
                listed.push((cluster, name, first_cluster));
            }
        }

        report_recovery(
            evidence,
            boot,
            extent,
            partition,
            &directory.entries,
            Listing::Orphaned,
            DirectoryPart::Entries,
            options,
            counts,
            sink,
        );
        report_recovery(
            evidence,
            boot,
            extent,
            partition,
            &directory.residue,
            Listing::Orphaned,
            DirectoryPart::Residue,
            options,
            counts,
            sink,
        );
    }

    for (holder, name, first_cluster) in listed {
        if !seen.contains(&first_cluster) {
            sink.record(Event::OrphanedListingUnread {
                holder,
                name: &name,
                first_cluster,
            });
            counts.directories_unread += 1;
        }
    }
}

/// Queues every directory a listing names, except `.` and `..`.
///
/// `ADR-0016` Decision B: the dot entries are listed and never followed.
/// Entries past the terminator are followed as well, because a deleted
/// directory named there is read under the same checks as one named before
/// it, and `ADR-0014` Appendix B.2 counted both.
fn queue_subdirectories(
    directory: &Directory,
    parent: &str,
    depth: usize,
    pending: &mut VecDeque<Pending>,
) {
    for slots in [&directory.entries, &directory.residue] {
        for (index, entry) in slots.iter().enumerate() {
            if is_dot_entry(&entry.kind) {
                continue;
            }

            let (first_cluster, segment, deleted) = match &entry.kind {
                EntryKind::ShortName {
                    name,
                    directory: true,
                    first_cluster,
                    ..
                } => (*first_cluster, name.clone(), false),
                EntryKind::Deleted {
                    was:
                        DeletedKind::ShortName {
                            surviving_name,
                            directory: true,
                            first_cluster,
                            ..
                        },
                } => (
                    *first_cluster,
                    deleted_segment(slots, index, surviving_name),
                    true,
                ),
                _ => continue,
            };

            // `ADR-0016` Decision G. A name that cannot be rendered is not
            // guessed at; the directory is named by its first cluster instead.
            let segment = segment.unwrap_or_else(|| format!("c{first_cluster}"));

            pending.push_back(Pending {
                first_cluster,
                path: format!("{parent}/{segment}"),
                depth: depth + 1,
                deleted,
            });
        }
    }
}

/// The path segment a deleted directory's short entry names.
///
/// Its destroyed first byte is recovered as the listing recovers it, from
/// the long-name components before it, so a heading and the entry it came
/// from name the directory the same way; EXP-0008 found them disagreeing.
/// Where nothing recovers the byte it is shown as `?`, as before.
fn deleted_segment(slots: &[Entry], index: usize, surviving: &[u8; 10]) -> Option<String> {
    let first = match associate(slots, index) {
        Some(FirstByte::Recovered(byte)) => byte,
        _ => b'?',
    };
    recovered_name(first, surviving)
}

/// Reads one directory below the root, reports it, and queues what it names.
///
/// `ADR-0016` Decisions A, C, D and E. A directory that is not read is a
/// coverage gap with its reason reported, and one that was read but may
/// continue is a gap of its own kind.
#[allow(clippy::too_many_arguments)]
fn report_subdirectory<R: EvidenceReader>(
    evidence: &mut R,
    boot: &Fat32BootSector,
    extent: VolumeExtent,
    partition: u8,
    next: Pending,
    seen: &mut HashSet<u32>,
    pending: &mut VecDeque<Pending>,
    options: Options<'_>,
    counts: &mut RunCounts,
    sink: &mut dyn Sink,
) {
    let reached = Reached::Walked {
        path: &next.path,
        deleted: next.deleted,
    };

    if next.depth > MAX_DEPTH {
        sink.record(Event::DirectoryNotRead {
            reached,
            reason: NotReadReason::TooDeep,
        });
        counts.directories_unread += 1;
        return;
    }
    if seen.contains(&next.first_cluster) {
        sink.record(Event::DirectoryNotRead {
            reached,
            reason: NotReadReason::AlreadyRead {
                cluster: next.first_cluster,
            },
        });
        counts.directories_unread += 1;
        return;
    }

    let (directory, may_continue) = if next.deleted {
        match enumerate_deleted_directory(evidence, boot, extent, next.first_cluster) {
            Ok(DeletedDirectory::Read {
                listing,
                may_continue,
            }) => (listing, may_continue),
            Ok(DeletedDirectory::Refused(reason)) => {
                sink.record(Event::DirectoryNotRead {
                    reached,
                    reason: NotReadReason::Refused(&reason),
                });
                counts.directories_unread += 1;
                return;
            }
            Err(e) => {
                sink.record(Event::DirectoryNotRead {
                    reached,
                    reason: NotReadReason::Enumeration(&e),
                });
                counts.directories_unread += 1;
                return;
            }
        }
    } else {
        match enumerate_directory(evidence, boot, extent, next.first_cluster) {
            Ok(listing) => (listing, false),
            Err(e) => {
                sink.record(Event::DirectoryNotRead {
                    reached,
                    reason: NotReadReason::Enumeration(&e),
                });
                counts.directories_unread += 1;
                return;
            }
        }
    };

    // A live chain that runs into a cluster another directory already
    // occupies is cross-linked, and its listing would repeat that one.
    if let Some(shared) = directory.clusters.iter().find(|c| seen.contains(*c)) {
        sink.record(Event::DirectoryNotRead {
            reached,
            reason: NotReadReason::AlreadyRead { cluster: *shared },
        });
        counts.directories_unread += 1;
        return;
    }
    seen.extend(directory.clusters.iter().copied());

    sink.record(Event::Directory {
        reached,
        directory: &directory,
    });

    if may_continue {
        // `ADR-0016` Decision D. The first cluster holds no terminator, and
        // no later cluster can be located, so nothing else is read.
        sink.record(Event::ListingMayContinue(next.first_cluster));
        counts.listings_may_continue += 1;
    }

    queue_subdirectories(&directory, &next.path, next.depth, pending);
    report_recovery(
        evidence,
        boot,
        extent,
        partition,
        &directory.entries,
        Listing::Walked,
        DirectoryPart::Entries,
        options,
        counts,
        sink,
    );
    report_recovery(
        evidence,
        boot,
        extent,
        partition,
        &directory.residue,
        Listing::Walked,
        DirectoryPart::Residue,
        options,
        counts,
        sink,
    );
}

/// Reports what the volume says about each deleted entry's content.
///
/// `ADR-0010` Decision B. Without `--recover` this reads the FAT and reports
/// the run each deleted entry implies; with it, the runs that pass are read
/// and hashed. The first is what the volume states, the second is derived
/// from an assumption the evidence cannot confirm.
///
/// An assessment failure is reported and does not stop the others, for the
/// same reason a read failure does not: what already succeeded stands.
#[allow(clippy::too_many_arguments)]
fn report_recovery<R: EvidenceReader>(
    evidence: &mut R,
    boot: &Fat32BootSector,
    extent: VolumeExtent,
    partition: u8,
    entries: &[Entry],
    listing: Listing,
    part: DirectoryPart,
    options: Options<'_>,
    counts: &mut RunCounts,
    sink: &mut dyn Sink,
) {
    // `ADR-0017` Decision C. An orphaned listing's files are not deleted;
    // `listing` says so. `part` says whether `entries` is the directory's
    // current listing or its residue past the terminator. Both are
    // findings carried on every event; whether and when to head them is
    // a rendering choice left entirely to the sink.
    for entry in entries {
        let assessment = match assess_listed(entry, listing, boot, extent, evidence) {
            Ok(None) => continue,
            Ok(Some(assessment)) => assessment,
            Err(e) => {
                sink.record(Event::NotAssessed {
                    listing,
                    part,
                    entry,
                    error: &e,
                });
                counts.not_assessed += 1;
                continue;
            }
        };

        match assessment {
            Assessment::Ineligible(reason) => {
                sink.record(Event::Ineligible {
                    listing,
                    part,
                    entry,
                    reason,
                });

                match reason {
                    // Entries that describe a file which yielded nothing.
                    Ineligible::EmptyFile => counts.empty += 1,
                    Ineligible::ReservedFirstCluster { .. } => counts.reserved_cluster += 1,
                    Ineligible::RunOutOfRange { .. } => counts.run_out_of_range += 1,
                    // A deleted directory is read as a directory, and any
                    // gap it leaves is counted where it is read, so that one
                    // entry is never two findings. `ADR-0016` Decisions C and
                    // D. The rest describe no file:
                    // a long-name component, a volume label and an entry
                    // with impossible attributes locate no content, so
                    // counting them as files that went unrecovered would
                    // answer `ADR-0013` Appendix C.4's question wrongly.
                    Ineligible::Directory
                    | Ineligible::VolumeLabel
                    | Ineligible::LongNameComponent
                    | Ineligible::InvalidEntry { .. } => {}
                }
            }
            Assessment::RunBroken {
                run,
                first_allocated,
            } => {
                sink.record(Event::RunBroken {
                    listing,
                    part,
                    entry,
                    run,
                    first_allocated,
                });
                counts.refused += 1;
            }
            Assessment::Recoverable(found) => {
                counts.free_runs += 1;
                sink.record(Event::Recoverable {
                    listing,
                    part,
                    entry,
                    run: *found.run(),
                });

                if options.recover {
                    // `ADR-0015` Decision D. The partition, the entry's
                    // location and the first cluster are facts the run
                    // established; no byte of evidence reaches the path.
                    let destination = options.output.map(|directory| Destination {
                        directory,
                        partition,
                        cluster: entry.cluster,
                        slot: entry.slot,
                    });

                    match extract(&found, boot, extent, evidence, destination) {
                        Ok(extracted) => {
                            // `ADR-0015` Decision G and section 9: the digest
                            // stands even where the destination has no say
                            // in what the evidence holds.
                            let mut readback_matches = None;
                            if let Some(output) = &extracted.output {
                                match output {
                                    Output::Exists { .. }
                                    | Output::NotCreated { .. }
                                    | Output::Failed { .. } => {
                                        counts.not_delivered += 1;
                                    }
                                    Output::Written { readback, .. } => {
                                        readback_matches = Some(*readback == extracted.digest);
                                    }
                                    Output::Unverified { .. } => {}
                                }
                            }

                            let validation = validate(
                                extracted.digest,
                                extracted.bytes_hashed,
                                options.reference,
                            );
                            if validation.outcome == Outcome::Match {
                                counts.matched += 1;
                            }

                            sink.record(Event::Extracted {
                                listing,
                                part,
                                entry,
                                extraction: &extracted,
                                validation,
                                readback_matches,
                            });
                        }
                        Err(e) => {
                            sink.record(Event::NotExtracted {
                                listing,
                                part,
                                entry,
                                error: &e,
                            });
                            counts.not_extracted += 1;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reference digest, for the invocations that supply one. Its value
    /// never reaches a comparison here: these tests exercise the counts,
    /// and `validation` owns what a comparison decides.
    fn reference() -> Sha256Digest {
        Sha256Digest::from_hex("0000000000000000000000000000000000000000000000000000000000000000")
            .expect("64 hexadecimal characters")
    }

    fn recovering(reference: Option<Sha256Digest>) -> Options<'static> {
        Options {
            recover: true,
            reference,
            output: None,
        }
    }

    fn reporting() -> Options<'static> {
        Options {
            recover: false,
            reference: None,
            output: None,
        }
    }

    /// EXP-0008. On `dfr-11-fat.dd` a deleted directory's entry read `name
    /// SAGITT~1 recovered` while its heading read `/?AGITT~1`. The heading now
    /// recovers the first byte as the entry does, from the long-name
    /// component before it, whose checksum here is the one that image holds.
    #[test]
    fn a_deleted_directory_is_named_as_its_entry_recovers_it() {
        let named = Directory {
            entries: vec![
                Entry {
                    cluster: 2,
                    slot: 3,
                    kind: EntryKind::Deleted {
                        was: DeletedKind::LongName { checksum: 0x35 },
                    },
                },
                Entry {
                    cluster: 2,
                    slot: 4,
                    kind: EntryKind::Deleted {
                        was: DeletedKind::ShortName {
                            surviving_name: *b"AGITT~1   ",
                            directory: true,
                            first_cluster: 5,
                            file_size: 0,
                            nt_res: 0,
                        },
                    },
                },
            ],
            residue: Vec::new(),
            clusters: vec![2],
            observations: Vec::new(),
        };
        let mut pending = VecDeque::new();
        queue_subdirectories(&named, "", 0, &mut pending);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].path, "/SAGITT~1");
        assert!(pending[0].deleted);

        // With no long-name component before it, nothing recovers the byte.
        let bare = Directory {
            entries: named.entries[1..].to_vec(),
            ..named.clone()
        };
        let mut pending = VecDeque::new();
        queue_subdirectories(&bare, "", 0, &mut pending);
        assert_eq!(pending[0].path, "/?AGITT~1");
    }

    #[test]
    fn an_image_declaring_no_partition_is_covered_completely() {
        // `mbr-empty.img`: nothing was analysed because the table declares
        // nothing to analyse, which `ADR-0014` Appendix A.6 names as the
        // control a coverage statement must not report as a gap.
        let counts = RunCounts::default();

        assert_eq!(counts.coverage(), Coverage::Complete);
    }

    #[test]
    fn a_volume_analysed_with_nothing_missed_is_covered_completely() {
        let counts = RunCounts {
            volumes_analysed: 1,
            ..RunCounts::default()
        };

        assert_eq!(counts.coverage(), Coverage::Complete);
    }

    #[test]
    fn a_rejected_partition_table_leaves_nothing_analysed() {
        // `bad-signature.img`. Appendix A.8's rule reported this as a
        // success, because it counted uncovered partitions and this run
        // parsed none.
        let counts = RunCounts {
            table_rejected: 1,
            ..RunCounts::default()
        };

        assert_eq!(counts.coverage(), Coverage::NothingAnalysed);
    }

    #[test]
    fn a_directory_not_read_leaves_a_volume_incompletely_covered() {
        // A deleted directory whose first cluster failed `ADR-0016`
        // Decision C's checks. No fixture holds one.
        let counts = RunCounts {
            volumes_analysed: 1,
            directories_unread: 1,
            ..RunCounts::default()
        };

        assert_eq!(counts.coverage(), Coverage::Incomplete);
    }

    #[test]
    fn a_volume_lost_beside_one_analysed_is_incomplete() {
        // No fixture holds two volumes of which one is analysed and one is
        // not, which `ADR-0014` Appendix B.5 records. This is that case.
        let counts = RunCounts {
            volumes_analysed: 1,
            boot_rejected: 1,
            ..RunCounts::default()
        };

        assert_eq!(counts.coverage(), Coverage::Incomplete);
    }

    #[test]
    fn every_kind_of_gap_is_counted_as_one() {
        // The eleven kinds of Appendix B.4, the twelfth `ADR-0015` section 9
        // adds, the thirteenth `ADR-0016` Decision D adds, the fourteenth
        // `ADR-0017` Decision D adds, and a partition entry dropped as
        // malformed. Five are unreachable from any fixture, so this is the
        // only place they are exercised.
        let kinds: [fn(&mut RunCounts); 15] = [
            |counts| counts.table_unread += 1,
            |counts| counts.table_rejected += 1,
            |counts| counts.partitions_anomalous += 1,
            |counts| counts.gpt += 1,
            |counts| counts.partition_unread += 1,
            |counts| counts.unidentified += 1,
            |counts| counts.unsupported += 1,
            |counts| counts.boot_rejected += 1,
            |counts| counts.root_unread += 1,
            |counts| counts.not_assessed += 1,
            |counts| counts.not_extracted += 1,
            |counts| counts.not_delivered += 1,
            |counts| counts.directories_unread += 1,
            |counts| counts.listings_may_continue += 1,
            |counts| counts.orphan_search_stopped += 1,
        ];

        for set in kinds {
            let mut counts = RunCounts::default();
            set(&mut counts);

            assert_eq!(counts.gaps(), 1);
            assert_eq!(counts.coverage(), Coverage::NothingAnalysed);

            counts.volumes_analysed = 1;

            assert_eq!(counts.coverage(), Coverage::Incomplete);
        }
    }

    #[test]
    fn an_artifact_is_a_free_run_that_was_read() {
        let counts = RunCounts {
            free_runs: 3,
            not_extracted: 1,
            ..RunCounts::default()
        };

        assert_eq!(counts.artifacts(recovering(None)), 2);
        assert_eq!(counts.not_read(recovering(None)), 0);
    }

    #[test]
    fn without_recover_every_free_run_is_unread_and_no_artifact_exists() {
        // The default invocation. `ADR-0010` Decision B: the run reports
        // what the volume states and reads no content.
        let counts = RunCounts {
            free_runs: 3,
            ..RunCounts::default()
        };

        assert_eq!(counts.artifacts(reporting()), 0);
        assert_eq!(counts.not_read(reporting()), 3);
    }

    #[test]
    fn a_difference_is_an_artifact_the_reference_did_not_equal() {
        let counts = RunCounts {
            free_runs: 3,
            matched: 1,
            ..RunCounts::default()
        };

        assert_eq!(counts.differed(recovering(Some(reference()))), 2);
    }

    #[test]
    fn nothing_differs_where_no_reference_was_supplied() {
        // `validate` returns `NotAttempted` for every extraction of such a
        // run, so no artifact is a difference rather than every one.
        let counts = RunCounts {
            free_runs: 3,
            ..RunCounts::default()
        };

        assert_eq!(counts.differed(recovering(None)), 0);
    }
}
