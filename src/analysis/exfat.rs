//! The analysis of an exFAT volume in a partition.
//!
//! `ADR-0019` section 3. A child of `analysis` so that it adds to the run's
//! counts, which that module keeps private, and reports through the same
//! [`Sink`]. Nothing here reads a directory itself: [`ExFatReader`] does, and
//! this module decides what each thing it finds means for the run.
//!
//! A volume is counted as analysed once its root directory has been read.
//! A volume whose boot region is valid and whose root cannot be read is a
//! gap, and is not counted, so a run cannot report it covered.

use std::collections::VecDeque;
use std::rc::Rc;

use crate::Error;
use crate::evidence::EvidenceReader;
use crate::exfat_boot::{ExFatBootFailure, ExFatBootSector, parse_boot_region};
use crate::exfat_directory::{
    DeletedDirectoriesRead, DeletedSetError, DeletedSetOutcome, DirItem, DirectorySource,
    DirectorySummary, ExFatBitmapEntry, ExFatBitmapError, ExFatDirectoryError, ExFatFile,
    ExFatPosition, ExFatReader, ExFatStructureError, Limits,
};
use crate::exfat_recovery;
use crate::extraction::Destination;
use crate::filesystem::{UnknownReason, VolumeExtent};
use crate::partition::MbrPartition;
use crate::validation::{Outcome, validate};

use super::{Event, MAX_DEPTH, Options, RunCounts, Sink, delivery};

/// How a directory was reached, for the heading its listing is printed under.
///
/// Names are UTF-16 code units exactly as the evidence holds them. A sink
/// decides how to show them; `exfat_directory::escaped_name` is how the CLI
/// does.
#[derive(Clone, Copy, Debug)]
pub enum ExFatReached<'a> {
    /// The volume's root directory.
    Root,

    /// Named by an entry, at `components` from the root, outermost first.
    Walked { components: &'a [Vec<u16>] },
}

/// Why a named directory's contents were not read.
#[derive(Clone, Copy, Debug)]
pub enum ExFatNotRead<'a> {
    /// Deeper below the root than [`MAX_DEPTH`].
    TooDeep,

    /// Reading it failed, wholly or part way.
    Error(&'a ExFatDirectoryError),
}

/// Why phase 2 did not run on a volume, or stopped part way (ADR-0022
/// section 5, S6-1 to S6-3). The deleted sets it had not decided are not
/// assessed.
#[derive(Clone, Copy, Debug)]
pub enum ExFatPhaseTwoStopped<'a> {
    /// The root held no allocation bitmap entry, so condition 1 cannot be
    /// shown for any set.
    NoBitmap,

    /// A live directory of the volume was not read whole, so the bitmap
    /// check would cover only part of what is in use (S6-1).
    DirectoriesNotReadWhole,

    /// The bitmap's chain could not be followed (ADR-0022 Decision E).
    BitmapNotOpened(&'a ExFatBitmapError),

    /// The bitmap check failed: the bitmap does not describe the volume
    /// (ADR-0022 Appendix A, S6-2).
    BitmapNotBelieved(&'a ExFatStructureError),

    /// Reading the deleted directories failed (S6-3).
    DeletedDirectories(&'a ExFatDirectoryError),

    /// Following the claims failed (S6-3).
    Claims(&'a Error),

    /// Deciding a set failed: it and every later one are left (S6-3).
    Decide(&'a DeletedSetError),
}

/// Where a volume lies and how it is to be analysed: its partition's extent
/// and MBR entry, the reader's bounds, and the operator's options.
#[derive(Clone, Copy)]
struct Volume<'a> {
    extent: VolumeExtent,
    limits: Limits,
    /// MBR entry, 1 to 4, which names the artifacts written from it.
    partition: u8,
    options: Options<'a>,
}

/// What the live walk of one volume gathered for phase 2.
#[derive(Default)]
struct VolumeWalk {
    /// The root's allocation bitmap entry.
    bitmap: Option<ExFatBitmapEntry>,

    /// Unused File entries the walk met: every deleted set of the volume
    /// it saw, usable or not.
    deleted_sets: usize,
}

/// A directory's name in the path of the directory being read.
///
/// A parent is shared, so a directory queued to be read costs its own name and
/// not its whole path.
struct PathNode {
    parent: Option<Rc<PathNode>>,
    name: Vec<u16>,
}

impl PathNode {
    /// The names from the root to this node, outermost first.
    fn components(node: &Rc<PathNode>) -> Vec<Vec<u16>> {
        let mut names = Vec::new();
        let mut current = Some(node);
        while let Some(n) = current {
            if n.parent.is_some() {
                names.push(n.name.clone());
            }
            current = n.parent.as_ref();
        }
        names.reverse();
        names
    }
}

/// A directory named by an entry and not yet read.
struct Pending {
    source: DirectorySource,
    path: Rc<PathNode>,
    depth: usize,
}

/// Reports a volume whose filesystem was identified as exFAT.
///
/// A boot region that cannot be used is a gap of its own kind and is
/// reported with its reasons. TexFAT is a valid exFAT volume this tool does
/// not analyse, so it is counted with the other filesystems identified and
/// not analysed.
pub(super) fn report_identified<R: EvidenceReader>(
    evidence: &mut R,
    partition: &MbrPartition,
    extent: VolumeExtent,
    options: Options<'_>,
    counts: &mut RunCounts,
    sink: &mut dyn Sink,
) {
    match parse_boot_region(evidence, extent) {
        Ok(boot) => {
            let volume = Volume {
                extent,
                limits: Limits::DEFAULT,
                partition: partition.index,
                options,
            };
            report_volume(evidence, &boot, volume, counts, sink);
        }
        Err(failure) => report_boot_failure(partition, failure, counts, sink),
    }
}

/// Tries a partition no sector-0 identification named as exFAT.
///
/// For a partition the table declares as type `0x07` whose first sector held
/// nothing identifiable, `sector_zero` being why: that sector may be the one
/// damaged, and an exFAT volume keeps a second copy of its boot region. If a
/// valid region is found, the partition is reported as exFAT identified
/// from it, followed by the volume or by why it is not analysed, and `true`
/// is returned. Where none validates, nothing is reported and `false` is
/// returned, so the partition is reported as unidentified exactly as it was
/// before.
pub(super) fn report_unidentified_0x07<R: EvidenceReader>(
    evidence: &mut R,
    partition: &MbrPartition,
    sector_zero: &UnknownReason,
    extent: VolumeExtent,
    options: Options<'_>,
    counts: &mut RunCounts,
    sink: &mut dyn Sink,
) -> bool {
    match parse_boot_region(evidence, extent) {
        Ok(boot) => {
            sink.record(Event::ExFatIdentifiedFromBackup {
                partition,
                sector_zero,
            });
            let volume = Volume {
                extent,
                limits: Limits::DEFAULT,
                partition: partition.index,
                options,
            };
            report_volume(evidence, &boot, volume, counts, sink);
            true
        }
        // A region that validates as exFAT was found, and what it says is
        // something to report.
        Err(failure @ (ExFatBootFailure::TexFat(_) | ExFatBootFailure::AmbiguousBackup { .. })) => {
            sink.record(Event::ExFatIdentifiedFromBackup {
                partition,
                sector_zero,
            });
            report_boot_failure(partition, failure, counts, sink);
            true
        }
        // No region validated, or the evidence could not be read: as before.
        Err(ExFatBootFailure::Rejected { .. } | ExFatBootFailure::Unreadable(_)) => false,
    }
}

fn report_boot_failure(
    partition: &MbrPartition,
    failure: ExFatBootFailure,
    counts: &mut RunCounts,
    sink: &mut dyn Sink,
) {
    match &failure {
        ExFatBootFailure::Unreadable(error) => {
            sink.record(Event::PartitionUnread { partition, error });
            counts.partition_unread += 1;
        }
        ExFatBootFailure::TexFat(region) => {
            sink.record(Event::ExFatTexFat(*region));
            counts.unsupported += 1;
        }
        ExFatBootFailure::Rejected { .. } | ExFatBootFailure::AmbiguousBackup { .. } => {
            sink.record(Event::ExFatBootRejected(&failure));
            counts.boot_rejected += 1;
        }
    }
}

/// Reads every directory reachable from the root of a volume, then decides
/// its deleted sets.
///
/// Breadth first, each directory once. `limits` bounds the work: a directory
/// beyond [`MAX_DEPTH`] is not read, and the volume's cluster cap, where it is
/// reached, leaves the directories not yet read counted and not read, never a
/// hang. Phase 2 follows the walk ([`report_deleted_sets`]).
fn report_volume<R: EvidenceReader>(
    evidence: &mut R,
    boot: &ExFatBootSector,
    volume: Volume<'_>,
    counts: &mut RunCounts,
    sink: &mut dyn Sink,
) {
    sink.record(Event::ExFatBootRegion(boot));

    let limits = volume.limits;
    let mut reader = ExFatReader::new(evidence, boot, volume.extent, limits);
    let mut pending: VecDeque<Pending> = VecDeque::new();

    // Directories named and never queued, because queueing them would hold
    // more directories than `limits.queued_directories` allows.
    let mut unqueued = 0usize;

    // What phase 2 needs from the walk, and whether every live directory of
    // this volume was read whole (S6-1): none was counted as unread.
    let mut walk = VolumeWalk::default();
    let unread_before = counts.directories_unread;

    let root = Rc::new(PathNode {
        parent: None,
        name: Vec::new(),
    });
    sink.record(Event::ExFatDirectoryStart {
        reached: ExFatReached::Root,
    });
    let root_source = DirectorySource::Root {
        first_cluster: boot.root_cluster,
    };
    match read_listing(
        &mut reader,
        root_source,
        &root,
        0,
        &mut pending,
        &mut unqueued,
        &mut walk,
        counts,
        sink,
    ) {
        Ok(summary) => {
            counts.volumes_analysed += 1;
            sink.record(Event::ExFatDirectoryEnd {
                reached: ExFatReached::Root,
                summary: &summary,
            });
        }
        Err(error) => {
            // The listing already printed stands, and the volume is not
            // counted as analysed. The deleted sets the root showed before
            // it failed are not assessed: it was not read whole (S6-1).
            sink.record(Event::ExFatRootDirectoryUnread(&error));
            counts.root_unread += 1;
            leave_not_assessed(
                ExFatPhaseTwoStopped::DirectoriesNotReadWhole,
                walk.deleted_sets,
                counts,
                sink,
            );
            return;
        }
    }

    // Directories counted as not read because the cluster cap was reached:
    // the one being read, and every one still queued.
    let mut cap_unread = 0usize;
    while let Some(next) = pending.pop_front() {
        let components = PathNode::components(&next.path);
        let reached = ExFatReached::Walked {
            components: &components,
        };

        if next.depth > MAX_DEPTH {
            sink.record(Event::ExFatDirectoryNotRead {
                reached,
                reason: ExFatNotRead::TooDeep,
            });
            counts.directories_unread += 1;
            continue;
        }

        sink.record(Event::ExFatDirectoryStart { reached });
        match read_listing(
            &mut reader,
            next.source,
            &next.path,
            next.depth,
            &mut pending,
            &mut unqueued,
            &mut walk,
            counts,
            sink,
        ) {
            Ok(summary) => sink.record(Event::ExFatDirectoryEnd {
                reached,
                summary: &summary,
            }),
            Err(ExFatDirectoryError::ClusterCapReached { .. }) => {
                // This directory, and every one not yet read, is counted.
                // Reading on would only reach the same cap again.
                cap_unread = 1 + pending.len();
                counts.directories_unread += cap_unread;
                pending.clear();
                break;
            }
            Err(error) => {
                sink.record(Event::ExFatDirectoryNotRead {
                    reached,
                    reason: ExFatNotRead::Error(&error),
                });
                counts.directories_unread += 1;
            }
        }
    }

    if cap_unread > 0 {
        sink.record(Event::ExFatDirectoryCapReached {
            cap: limits.directory_clusters,
            directories_unread: cap_unread,
        });
    }
    if unqueued > 0 {
        sink.record(Event::ExFatQueueBoundReached {
            bound: limits.queued_directories,
            directories_unread: unqueued,
        });
    }

    let every_directory_read = counts.directories_unread == unread_before;
    report_deleted_sets(
        &mut reader,
        &walk,
        every_directory_read,
        volume,
        counts,
        sink,
    );
}

/// Phase 2 (ADR-0022 sections 5 and 6, Appendix E, S6-1 to S6-3): checks
/// the bitmap, reads the deleted directories, follows the claims, and
/// decides every recorded deleted set in the order the walk recorded them.
/// Where phase 2 does not run or stops, every set not yet decided is counted
/// not assessed, with one event saying why.
///
/// Content is read only under `--recover` (ADR-0022 Decision G): condition 3
/// is then decided before any file is opened, and each recoverable set is
/// extracted (Decision H). An extraction that fails is counted not extracted
/// and the next set is decided, as FAT32's loop does; only an error while
/// deciding stops phase 2 (S6-3).
fn report_deleted_sets<R: EvidenceReader>(
    reader: &mut ExFatReader<'_, R>,
    walk: &VolumeWalk,
    every_directory_read: bool,
    volume: Volume<'_>,
    counts: &mut RunCounts,
    sink: &mut dyn Sink,
) {
    use ExFatPhaseTwoStopped as Stopped;

    // Until the claims are followed, every set the walk met is left: the
    // records do not yet say which were unusable or not recorded.
    let all = walk.deleted_sets;
    let Some(entry) = walk.bitmap else {
        return leave_not_assessed(Stopped::NoBitmap, all, counts, sink);
    };
    if !every_directory_read {
        return leave_not_assessed(Stopped::DirectoriesNotReadWhole, all, counts, sink);
    }
    let mut bitmap = match reader.open_bitmap(&entry) {
        Ok(bitmap) => bitmap,
        Err(error) => {
            return leave_not_assessed(Stopped::BitmapNotOpened(&error), all, counts, sink);
        }
    };
    if let Err(error) = reader.check_bitmap(&mut bitmap) {
        return leave_not_assessed(Stopped::BitmapNotBelieved(&error), all, counts, sink);
    }

    match reader.read_deleted_directories(&mut bitmap) {
        Ok(read) => {
            let (unread, unseen) = deleted_directory_gaps(&read);
            counts.exfat_deleted_directories_unread += unread;
            counts.exfat_claimants_not_followed += unseen;
            sink.record(Event::ExFatDeletedDirectoriesRead(&read));
        }
        Err(error) => {
            return leave_not_assessed(Stopped::DeletedDirectories(&error), all, counts, sink);
        }
    }

    let claims = match reader.assess_claims() {
        Ok(claims) => claims,
        Err(error) => return leave_not_assessed(Stopped::Claims(&error), all, counts, sink),
    };
    counts.exfat_sets_unusable += claims.sets_unusable as usize;
    counts.exfat_not_assessed += claims.sets_unrecorded as usize;
    counts.exfat_claimants_not_followed += (claims.unrecorded + claims.orphans_unrecorded) as usize;
    sink.record(Event::ExFatClaimsAssessed(&claims));

    let recorded = reader.deleted_sets_recorded();
    for index in 0..recorded {
        let recover = volume.options.recover;
        match reader.decide_deleted_set(index, &mut bitmap, &claims, recover) {
            Ok(decision) => {
                count_outcome(&decision.outcome, counts);
                sink.record(Event::ExFatDeletedSetDecided {
                    position: decision.position,
                    outcome: &decision.outcome,
                });
                if let DeletedSetOutcome::Recoverable { file } = &decision.outcome {
                    report_extraction(reader, decision.position, file, volume, counts, sink);
                }
            }
            Err(error) => {
                let left = recorded - index;
                return leave_not_assessed(Stopped::Decide(&error), left, counts, sink);
            }
        }
    }
}

/// Extracts one recoverable set (ADR-0022 Decision H), validates its digest
/// against the operator's reference, and reports it, as `report_recovery`
/// does for a FAT32 run. A failure is counted and reported, and is not
/// returned: the caller goes on to the next set.
fn report_extraction<R: EvidenceReader>(
    reader: &mut ExFatReader<'_, R>,
    position: ExFatPosition,
    file: &ExFatFile,
    volume: Volume<'_>,
    counts: &mut RunCounts,
    sink: &mut dyn Sink,
) {
    // The path is composed from the partition, the File entry's position
    // and the first cluster; no byte of evidence reaches it.
    let destination = volume.options.output.map(|directory| Destination {
        directory,
        partition: volume.partition,
        cluster: position.cluster,
        slot: position.slot,
    });

    match exfat_recovery::extract(reader, position, file, destination) {
        Ok(extraction) => {
            let (delivered, readback_matches) = delivery(&extraction);
            if !delivered {
                counts.exfat_not_delivered += 1;
            }
            let validation = validate(
                extraction.digest,
                extraction.bytes_hashed,
                volume.options.reference,
            );
            if validation.outcome == Outcome::Match {
                counts.exfat_matched += 1;
            }
            sink.record(Event::ExFatExtracted {
                position,
                file,
                extraction: &extraction,
                validation,
                readback_matches,
            });
        }
        Err(error) => {
            counts.exfat_not_extracted += 1;
            sink.record(Event::ExFatNotExtracted {
                position,
                file,
                error: &error,
            });
        }
    }
}

/// The gaps a reading of the deleted directories leaves (ADR-0022 Decision
/// K, Appendix D): the deleted directories not read whole, each kind of
/// reason summed, and the sets inside whose claims were not recorded.
fn deleted_directory_gaps(read: &DeletedDirectoriesRead) -> (usize, usize) {
    let unread = read.not_read_out_of_range
        + read.not_read_malformed
        + read.not_read_in_use
        + read.not_read_before
        + read.not_read_bound
        + read.partly_read;
    let unseen = read.sets_inside_unusable + read.sets_inside_unrecorded;
    (unread as usize, unseen as usize)
}

/// Counts `sets` deleted sets not assessed for `reason`, and reports it
/// where there are any.
fn leave_not_assessed(
    reason: ExFatPhaseTwoStopped<'_>,
    sets: usize,
    counts: &mut RunCounts,
    sink: &mut dyn Sink,
) {
    if sets > 0 {
        counts.exfat_not_assessed += sets;
        sink.record(Event::ExFatDeletedSetsNotAssessed { reason, sets });
    }
}

/// Counts one decided set as a finding or a gap (ADR-0022 section 11).
fn count_outcome(outcome: &DeletedSetOutcome, counts: &mut RunCounts) {
    use DeletedSetOutcome as O;
    let count = match outcome {
        O::UnusableOnReread { .. } | O::LeftByBound { .. } | O::NotAssessed { .. } => {
            &mut counts.exfat_not_assessed
        }
        O::DeletedDirectory { .. } => &mut counts.exfat_deleted_directories,
        O::NoData { .. } => &mut counts.exfat_no_data,
        O::ConditionFour { .. } | O::ClusterWithoutData { .. } => {
            &mut counts.exfat_refused_condition_4
        }
        O::ConditionOne { .. } => &mut counts.exfat_refused_condition_1,
        O::ConditionTwo { .. } => &mut counts.exfat_refused_condition_2,
        O::NoValidData { .. } | O::ZerosOnly { .. } => &mut counts.exfat_refused_condition_3,
        // Passed without content read; with it, condition 3 decided too.
        O::ConditionThreeNotChecked { .. } => &mut counts.exfat_passed,
        O::Recoverable { .. } => &mut counts.exfat_recoverable,
    };
    *count += 1;
}

/// Reads one directory, reporting what it holds and queueing the directories
/// it names.
#[allow(clippy::too_many_arguments)]
fn read_listing<R: EvidenceReader>(
    reader: &mut ExFatReader<'_, R>,
    source: DirectorySource,
    path: &Rc<PathNode>,
    depth: usize,
    pending: &mut VecDeque<Pending>,
    dropped: &mut usize,
    walk: &mut VolumeWalk,
    counts: &mut RunCounts,
    sink: &mut dyn Sink,
) -> Result<DirectorySummary, ExFatDirectoryError> {
    let queue_bound = reader.limits().queued_directories;

    reader.read_directory(source, &mut |item| match item {
        DirItem::File { position, file } => {
            sink.record(Event::ExFatEntry { position, file });

            if file.is_directory() {
                // A directory past the bound is counted now and never held,
                // so a volume that names millions cannot fill memory.
                if pending.len() >= queue_bound {
                    counts.directories_unread += 1;
                    *dropped += 1;
                } else {
                    pending.push_back(Pending {
                        source: DirectorySource::Subdirectory {
                            first_cluster: file.first_cluster,
                            data_length: file.data_length,
                            no_fat_chain: file.no_fat_chain,
                        },
                        path: Rc::new(PathNode {
                            parent: Some(Rc::clone(path)),
                            name: file.name.clone(),
                        }),
                        depth: depth + 1,
                    });
                }
            }
        }
        DirItem::VolumeLabel { position, label } => {
            sink.record(Event::ExFatVolumeLabel { position, label });
        }
        DirItem::Bitmap { position, bitmap } => {
            // The root's is the volume's; the reader refuses a second.
            if depth == 0 {
                walk.bitmap = Some(*bitmap);
            }
            sink.record(Event::ExFatBitmap { position, bitmap });
        }
        DirItem::Refused { position, refusal } => {
            counts.exfat_sets_refused += 1;
            sink.record(Event::ExFatSetRefused { position, refusal });
        }
        DirItem::UnusedFileSet { .. } => walk.deleted_sets += 1,
        DirItem::AfterEndMarker { .. } => counts.exfat_end_marker_residue += 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Coverage;
    use crate::evidence::tests::MemoryImage;
    use crate::exfat_boot::tests as boot_tests;
    use crate::exfat_boot::{BootRegion, ExFatError};
    use crate::exfat_directory::escaped_name;
    use crate::exfat_directory::tests::{
        Vol, at_times, cluster_data, deleted, directory, extent, file, file_set, fill_clusters,
        mark, orphan_stream, seal, standard_root, structured, utf16, with_root,
    };
    use crate::exfat_recovery::tests::scratch;
    use crate::hash::Sha256Digest;
    use std::fs;
    use std::path::Path;

    /// Each event as one line of text, in order, so a test reads as a
    /// transcript. Only the exFAT events say anything.
    struct Transcript(Vec<String>);

    impl Sink for Transcript {
        fn record(&mut self, event: Event<'_>) {
            let line = match event {
                Event::ExFatDirectoryStart { reached } => format!("start {}", heading(reached)),
                Event::ExFatEntry { file, .. } => format!("entry {}", escaped_name(&file.name)),
                Event::ExFatSetRefused { .. } => "refused".to_string(),
                Event::ExFatDirectoryEnd { reached, .. } => format!("end {}", heading(reached)),
                Event::ExFatDirectoryNotRead { reached, reason } => format!(
                    "not read {} {}",
                    heading(reached),
                    match reason {
                        ExFatNotRead::TooDeep => "too deep",
                        ExFatNotRead::Error(_) => "error",
                    }
                ),
                Event::ExFatRootDirectoryUnread(_) => "root unread".to_string(),
                Event::ExFatDirectoryCapReached {
                    cap,
                    directories_unread,
                } => format!("cap {cap} unread {directories_unread}"),
                Event::ExFatQueueBoundReached {
                    bound,
                    directories_unread,
                } => format!("queue {bound} unread {directories_unread}"),
                Event::PartitionIdentified { .. } => "identified".to_string(),
                Event::ExFatIdentifiedFromBackup { .. } => "exfat from backup".to_string(),
                Event::ExFatTexFat(_) => "texfat".to_string(),
                Event::ExFatBootRejected(_) => "boot rejected".to_string(),
                Event::ExFatDeletedSetDecided { position, outcome } => format!(
                    "decided c{} s{} {}",
                    position.cluster,
                    position.slot,
                    outcome_kind(outcome)
                ),
                Event::ExFatDeletedSetsNotAssessed { reason, sets } => {
                    format!("not assessed {sets}: {}", stopped_kind(reason))
                }
                Event::ExFatDeletedDirectoriesRead(read) => {
                    format!("deleted directories read {}", read.directories_read)
                }
                Event::ExFatClaimsAssessed(claims) => {
                    format!("claims for {} sets", claims.verdicts.len())
                }
                Event::ExFatExtracted {
                    position,
                    extraction,
                    ..
                } => format!(
                    "extracted c{} s{} {}",
                    position.cluster, position.slot, extraction.digest
                ),
                Event::ExFatNotExtracted { position, .. } => {
                    format!("not extracted c{} s{}", position.cluster, position.slot)
                }
                _ => "other".to_string(),
            };
            self.0.push(line);
        }
    }

    fn outcome_kind(outcome: &DeletedSetOutcome) -> &'static str {
        use DeletedSetOutcome as O;
        match outcome {
            O::UnusableOnReread { .. } => "unusable on re-read",
            O::DeletedDirectory { .. } => "deleted directory",
            O::NoData { .. } => "no data",
            O::LeftByBound { .. } => "left by bound",
            O::ConditionFour { .. } => "condition 4",
            O::ClusterWithoutData { .. } => "condition 4 cluster without data",
            O::ConditionOne { .. } => "condition 1",
            O::ConditionTwo { .. } => "condition 2",
            O::NotAssessed { .. } => "not assessed",
            O::NoValidData { .. } => "condition 3 no valid data",
            O::ConditionThreeNotChecked { .. } => "passed",
            O::ZerosOnly { .. } => "condition 3 zeros",
            O::Recoverable { .. } => "recoverable",
        }
    }

    fn stopped_kind(reason: ExFatPhaseTwoStopped<'_>) -> &'static str {
        match reason {
            ExFatPhaseTwoStopped::NoBitmap => "no bitmap",
            ExFatPhaseTwoStopped::DirectoriesNotReadWhole => "not read whole",
            ExFatPhaseTwoStopped::BitmapNotOpened(_) => "bitmap not opened",
            ExFatPhaseTwoStopped::BitmapNotBelieved(_) => "bitmap not believed",
            ExFatPhaseTwoStopped::DeletedDirectories(_) => "deleted directories",
            ExFatPhaseTwoStopped::Claims(_) => "claims",
            ExFatPhaseTwoStopped::Decide(_) => "decide",
        }
    }

    fn heading(reached: ExFatReached<'_>) -> String {
        match reached {
            ExFatReached::Root => "/".to_string(),
            ExFatReached::Walked { components } => components
                .iter()
                .map(|c| format!("/{}", escaped_name(c)))
                .collect(),
        }
    }

    /// The default invocation: no content read, nothing written.
    const NOT_RECOVERING: Options<'static> = Options {
        recover: false,
        reference: None,
        output: None,
    };

    /// Analyses the synthetic volume through `evidence`, as partition 1.
    fn analyse_through<R: EvidenceReader>(
        evidence: &mut R,
        boot: &ExFatBootSector,
        limits: Limits,
        options: Options<'_>,
    ) -> (RunCounts, Vec<String>) {
        let mut counts = RunCounts::default();
        let mut transcript = Transcript(Vec::new());
        let volume = Volume {
            extent: extent(),
            limits,
            partition: 1,
            options,
        };
        report_volume(evidence, boot, volume, &mut counts, &mut transcript);
        (counts, transcript.0)
    }

    fn analyse(vol: &mut Vol, limits: Limits) -> (RunCounts, Vec<String>) {
        let boot = vol.boot.clone();
        analyse_through(&mut vol.image, &boot, limits, NOT_RECOVERING)
    }

    #[test]
    fn a_volume_whose_root_is_read_and_whose_entries_were_all_used_is_complete() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.put(2, 0, &file("a", 0, 0));

        let (counts, _) = analyse(&mut vol, Limits::DEFAULT);
        assert_eq!(counts.volumes_analysed(), 1);
        assert_eq!(counts.coverage(), Coverage::Complete);
    }

    #[test]
    fn a_deleted_entry_set_is_a_gap_so_the_volume_is_incomplete() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        let deleted = {
            let mut set = file("gone", 5, 10);
            for entry in set.iter_mut() {
                entry[0] &= 0x7F;
            }
            set
        };
        vol.put(2, 0, &deleted);
        vol.put(2, 3, &deleted);
        vol.put(2, 6, &file("a", 0, 0));

        let (counts, _) = analyse(&mut vol, Limits::DEFAULT);
        assert_eq!(counts.exfat_not_assessed(), 2);
        assert_eq!(counts.volumes_analysed(), 1);
        assert_eq!(counts.coverage(), Coverage::Incomplete);
    }

    #[test]
    fn a_refused_entry_set_is_a_gap() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        let mut bad = file("bad", 5, 10);
        bad[0][2] ^= 1;
        vol.put(2, 0, &bad);

        let (counts, transcript) = analyse(&mut vol, Limits::DEFAULT);
        assert_eq!(counts.exfat_sets_refused(), 1);
        assert_eq!(counts.coverage(), Coverage::Incomplete);
        assert!(transcript.contains(&"refused".to_string()));
    }

    /// A volume with a valid boot region and no readable root is not
    /// analysed.
    #[test]
    fn a_volume_is_not_counted_as_analysed_until_its_root_is_read() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        let mut critical = [0u8; 32];
        critical[0] = 0x84;
        vol.put(2, 0, &[critical]);

        let (counts, transcript) = analyse(&mut vol, Limits::DEFAULT);
        assert_eq!(counts.volumes_analysed(), 0);
        assert_eq!(counts.root_unread(), 1);
        assert_eq!(counts.coverage(), Coverage::NothingAnalysed);
        assert!(transcript.contains(&"root unread".to_string()));
    }

    #[test]
    fn a_root_that_cannot_be_followed_leaves_the_run_with_nothing_analysed() {
        let mut vol = Vol::new();
        vol.boot.root_cluster = 300;

        let (counts, _) = analyse(&mut vol, Limits::DEFAULT);
        assert_eq!(counts.volumes_analysed(), 0);
        assert_eq!(counts.root_unread(), 1);
    }

    #[test]
    fn subdirectories_are_read_under_the_path_of_the_names_that_reached_them() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.chain(&[3]);
        vol.chain(&[4]);
        vol.put(2, 0, &directory("sub", 3, 1));
        vol.put(3, 0, &directory("a\\b", 4, 1));
        vol.put(4, 0, &file("f", 0, 0));

        let (counts, transcript) = analyse(&mut vol, Limits::DEFAULT);
        assert_eq!(counts.coverage(), Coverage::Complete);
        assert!(
            transcript.contains(&"start /sub".to_string()),
            "{transcript:?}"
        );
        // The backslash is escaped, so the path cannot be forged.
        assert!(
            transcript.contains(&"start /sub/a\\u{005C}b".to_string()),
            "{transcript:?}"
        );
        assert!(transcript.contains(&"entry f".to_string()));
    }

    #[test]
    fn a_subdirectory_that_cannot_be_read_is_a_gap_and_the_volume_still_analysed() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.chain(&[3]);
        vol.put(2, 0, &directory("bad", 3, 1));
        let mut bitmap = [0u8; 32];
        bitmap[0] = 0x81;
        vol.put(3, 0, &[bitmap]);

        let (counts, transcript) = analyse(&mut vol, Limits::DEFAULT);
        assert_eq!(counts.directories_unread(), 1);
        assert_eq!(counts.volumes_analysed(), 1);
        assert_eq!(counts.coverage(), Coverage::Incomplete);
        assert!(transcript.contains(&"not read /bad error".to_string()));
    }

    #[test]
    fn a_subdirectory_chain_longer_than_its_length_is_a_gap() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        // One cluster of length, two of chain: the second is allocation
        // the directory's length does not cover (section 7.6.7).
        vol.put(2, 0, &directory("sub", 3, 1));
        vol.chain(&[3, 4]);

        let (counts, transcript) = analyse(&mut vol, Limits::DEFAULT);
        assert_eq!(counts.directories_unread(), 1);
        assert_eq!(counts.volumes_analysed(), 1);
        assert_eq!(counts.coverage(), Coverage::Incomplete);
        assert!(
            transcript.contains(&"not read /sub error".to_string()),
            "{transcript:?}"
        );
    }

    #[test]
    fn a_directory_below_the_depth_limit_is_not_read_and_is_counted() {
        let mut vol = Vol::new();
        // Cluster c holds one directory, at c + 1, so cluster 2 is the root
        // at depth 0 and cluster 131 is depth 129.
        for c in 2..=130u32 {
            vol.chain(&[c]);
            vol.put(c, 0, &directory("d", c + 1, 1));
        }
        vol.chain(&[131]);

        let (counts, transcript) = analyse(&mut vol, Limits::DEFAULT);
        let too_deep = transcript
            .iter()
            .filter(|l| l.ends_with("too deep"))
            .count();
        assert_eq!(too_deep, 1, "{transcript:?}");
        assert_eq!(counts.directories_unread(), 1);
        assert_eq!(counts.volumes_analysed(), 1);
    }

    /// The cap on directory clusters, set to 4: the root and three
    /// subdirectories are read, and the other three are counted.
    #[test]
    fn the_cluster_cap_leaves_the_unread_directories_counted_and_stops_the_walk() {
        let mut vol = Vol::new();
        // Five sets of three entries fit one cluster of sixteen; the sixth
        // goes in a second cluster of the root.
        vol.chain(&[2, 20]);
        for (i, name) in ["a", "b", "c", "d", "e", "f"].iter().enumerate() {
            let cluster = 3 + i as u32;
            vol.chain(&[cluster]);
            let (at, slot) = if i < 5 { (2, i * 3) } else { (20, 0) };
            vol.put(at, slot, &directory(name, cluster, 1));
        }
        // An unused entry in the last slot, so the root reads on into its
        // second cluster and is not ended by a zero.
        vol.put(2, 15, &[[0x01; 32]]);
        let limits = Limits {
            directory_clusters: 4,
            ..Limits::DEFAULT
        };

        // The root takes two clusters, `a` and `b` one each, and the cap is
        // reached at `c`: `c` to `f` are not read. Two of them were never
        // queued, one was queued and not reached, and one was the read that
        // failed.
        let (counts, transcript) = analyse(&mut vol, limits);
        assert_eq!(counts.directories_unread(), 4);
        assert_eq!(counts.volumes_analysed(), 1);
        assert_eq!(counts.coverage(), Coverage::Incomplete);
        assert_eq!(
            transcript.last().map(String::as_str),
            Some("cap 4 unread 4"),
            "{transcript:?}"
        );
        assert_eq!(
            transcript.iter().filter(|l| l.starts_with("end ")).count(),
            3
        );
    }

    #[test]
    fn entries_after_the_end_marker_make_the_directory_one_gap() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.put(2, 0, &file("a", 0, 0));
        // Three entries after the marker: one directory, one gap.
        vol.put(2, 5, &file("hidden", 0, 0));

        let (counts, _) = analyse(&mut vol, Limits::DEFAULT);
        assert_eq!(counts.exfat_end_marker_residue(), 1);
        assert_eq!(counts.coverage(), Coverage::Incomplete);
    }

    #[test]
    fn a_volume_with_nothing_after_any_end_marker_has_no_such_gap() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.put(2, 0, &file("a", 0, 0));

        let (counts, _) = analyse(&mut vol, Limits::DEFAULT);
        assert_eq!(counts.exfat_end_marker_residue(), 0);
    }

    /// Four directories named, room to hold two: the other two are counted
    /// as not read and are never queued.
    #[test]
    fn directories_past_the_queue_bound_are_counted_and_never_queued() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        for (i, name) in ["a", "b", "c", "d"].iter().enumerate() {
            let cluster = 3 + i as u32;
            vol.chain(&[cluster]);
            vol.put(2, i * 3, &directory(name, cluster, 1));
        }
        let limits = Limits {
            queued_directories: 2,
            ..Limits::DEFAULT
        };

        let (counts, transcript) = analyse(&mut vol, limits);
        assert_eq!(counts.directories_unread(), 2);
        assert_eq!(counts.volumes_analysed(), 1);
        assert_eq!(counts.coverage(), Coverage::Incomplete);
        assert_eq!(
            transcript.last().map(String::as_str),
            Some("queue 2 unread 2"),
            "{transcript:?}"
        );
        let read: Vec<&String> = transcript
            .iter()
            .filter(|l| l.starts_with("end "))
            .collect();
        assert_eq!(read.len(), 3, "the root and the two that were queued");
        assert!(
            !transcript
                .iter()
                .any(|l| l == "start /c" || l == "start /d")
        );
    }

    #[test]
    fn a_volume_within_the_queue_bound_reports_no_such_event() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.chain(&[3]);
        vol.put(2, 0, &directory("a", 3, 1));
        let limits = Limits {
            queued_directories: 1,
            ..Limits::DEFAULT
        };

        let (counts, transcript) = analyse(&mut vol, limits);
        assert_eq!(counts.directories_unread(), 0);
        assert!(!transcript.iter().any(|l| l.starts_with("queue ")));
    }

    #[test]
    fn a_texfat_volume_is_counted_with_the_filesystems_not_analysed() {
        let partition = MbrPartition {
            index: 1,
            boot_indicator: 0,
            partition_type: 0x07,
            start_lba: 0,
            sector_count: 4096,
        };
        let mut counts = RunCounts::default();
        let mut transcript = Transcript(Vec::new());

        report_boot_failure(
            &partition,
            ExFatBootFailure::TexFat(BootRegion::Main),
            &mut counts,
            &mut transcript,
        );

        assert_eq!(counts.unsupported(), 1);
        assert_eq!(counts.boot_rejected(), 0);
        assert_eq!(counts.coverage(), Coverage::NothingAnalysed);
    }

    #[test]
    fn refused_boot_regions_are_counted_as_a_rejected_boot_sector() {
        let partition = MbrPartition {
            index: 1,
            boot_indicator: 0,
            partition_type: 0x07,
            start_lba: 0,
            sector_count: 4096,
        };
        for failure in [
            ExFatBootFailure::Rejected {
                main: ExFatError::TexFat,
                backup: ExFatError::BackupNotLocated,
            },
            ExFatBootFailure::AmbiguousBackup {
                shifts: vec![9, 12],
            },
        ] {
            let mut counts = RunCounts::default();
            let mut transcript = Transcript(Vec::new());
            report_boot_failure(&partition, failure, &mut counts, &mut transcript);

            assert_eq!(counts.boot_rejected(), 1);
            assert_eq!(counts.coverage(), Coverage::NothingAnalysed);
        }
    }

    /// Tries the backup behind a type 0x07 partition whose first sector is
    /// zero, as `report_partition` does for one that identified nothing.
    fn report_zeroed_0x07(image: &mut MemoryImage) -> (bool, RunCounts, Vec<String>) {
        let extent = boot_tests::extent();
        let partition = MbrPartition {
            index: 1,
            boot_indicator: 0,
            partition_type: 0x07,
            start_lba: extent.start_lba,
            sector_count: extent.sector_count,
        };
        let sector_zero = UnknownReason::NoSignature { found: [0, 0] };
        let mut counts = RunCounts::default();
        let mut transcript = Transcript(Vec::new());

        let reported = report_unidentified_0x07(
            image,
            &partition,
            &sector_zero,
            extent,
            NOT_RECOVERING,
            &mut counts,
            &mut transcript,
        );
        (reported, counts, transcript.0)
    }

    #[test]
    fn a_texfat_backup_behind_a_zeroed_first_sector_is_exfat_and_unsupported() {
        let backup = boot_tests::texfat();
        let mut image = boot_tests::image_with_regions(&[(12 << 9, &backup)]);

        let (reported, counts, transcript) = report_zeroed_0x07(&mut image);
        assert!(reported);
        assert_eq!(transcript, ["exfat from backup", "texfat"]);
        assert_eq!(counts.unsupported(), 1);
        assert_eq!(counts.boot_rejected(), 0);
        assert_eq!(counts.coverage(), Coverage::NothingAnalysed);
    }

    #[test]
    fn an_ambiguous_backup_behind_a_zeroed_first_sector_is_exfat_and_rejected() {
        let small = boot_tests::valid();
        let large = boot_tests::valid_4096();
        let mut image = boot_tests::image_with_regions(&[(12 << 9, &small), (12 << 12, &large)]);

        let (reported, counts, transcript) = report_zeroed_0x07(&mut image);
        assert!(reported);
        assert_eq!(transcript, ["exfat from backup", "boot rejected"]);
        assert_eq!(counts.boot_rejected(), 1);
        assert_eq!(counts.unsupported(), 0);
        assert_eq!(counts.coverage(), Coverage::NothingAnalysed);
    }

    /// A set whose checksum is right passes every check the walk applies.
    #[test]
    fn a_sealed_set_validates_through_the_whole_walk() {
        let mut set = file("sealed", 0, 0);
        seal(&mut set);
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.put(2, 0, &set);

        let (counts, _) = analyse(&mut vol, Limits::DEFAULT);
        assert_eq!(counts.exfat_sets_refused(), 0);
    }

    // --- phase 2: every deleted set decided (ADR-0022 Appendix E) ------------

    /// The decided lines of a transcript, without their prefix.
    fn decided(transcript: &[String]) -> Vec<&str> {
        transcript
            .iter()
            .filter_map(|l| l.strip_prefix("decided "))
            .collect()
    }

    /// One deleted set of each outcome a volume shows without its content
    /// read, in the root after [`with_root`]'s five entries: slots 5 to 13
    /// of cluster 2, a set straddling into cluster 5, then slots 1 to 15 of
    /// cluster 5. Findings only, so the volume is complete.
    #[test]
    fn each_decided_set_is_counted_in_its_field_and_reported() {
        let unwritten = file_set(&utf16("unwritten"), 0x20, 33, 0, 512, false);
        let mut extra = deleted(file("empty", 0, 0));
        extra.extend(deleted(file("inuse", 30, 512)));
        extra.extend(deleted(file("short", 31, 1024)));
        extra.extend(deleted(file("odd", 32, 0)));
        extra.extend(deleted(unwritten));
        extra.extend(deleted(file("ok", 34, 512)));
        extra.extend(deleted(at_times(file("old", 35, 512), 0, 0, 0x90)));
        extra.extend(deleted(at_times(file("new", 35, 512), 8, 8, 0x90)));
        extra.extend(deleted(directory("gone", 20, 1)));
        let mut vol = with_root(&extra);
        for c in [30, 31, 32, 33, 34, 35, 20] {
            vol.chain(&[c]);
        }
        mark(&mut vol, &[3], &[30]);

        let (counts, transcript) = analyse(&mut vol, Limits::DEFAULT);
        assert_eq!(
            decided(&transcript),
            [
                "c2 s5 no data",
                "c2 s8 condition 1",
                "c2 s11 condition 4",
                "c2 s14 condition 4 cluster without data",
                "c5 s1 condition 3 no valid data",
                "c5 s4 passed",
                "c5 s7 condition 2",
                "c5 s10 passed",
                "c5 s13 deleted directory",
            ]
        );
        assert_eq!(counts.exfat_no_data(), 1);
        assert_eq!(counts.exfat_refused_condition_1(), 1);
        assert_eq!(counts.exfat_refused_condition_4(), 2);
        assert_eq!(counts.exfat_refused_condition_3(), 1);
        assert_eq!(counts.exfat_refused_condition_2(), 1);
        assert_eq!(counts.exfat_refused(), 5);
        assert_eq!(counts.exfat_passed(), 2);
        assert_eq!(counts.exfat_deleted_directories(), 1);
        assert_eq!(counts.exfat_not_assessed(), 0);
        assert!(transcript.contains(&"deleted directories read 1".to_string()));
        assert!(transcript.contains(&"claims for 9 sets".to_string()));
        assert_eq!(counts.coverage(), Coverage::Complete);
    }

    /// Room for one claimed run: the second set's claim is unseen, so it is
    /// not assessed, and the claim is a gap of its own.
    #[test]
    fn a_set_a_claims_bound_left_unseen_is_not_assessed_and_its_claim_a_gap() {
        let mut extra = deleted(file("first", 30, 512));
        extra.extend(deleted(file("second", 40, 512)));
        let mut vol = with_root(&extra);
        vol.chain(&[30]);
        vol.chain(&[40]);
        let limits = Limits {
            claimed_runs: 1,
            ..Limits::DEFAULT
        };

        let (counts, transcript) = analyse(&mut vol, limits);
        assert_eq!(decided(&transcript), ["c2 s5 passed", "c2 s8 not assessed"]);
        assert_eq!(counts.exfat_not_assessed(), 1);
        assert_eq!(counts.exfat_claimants_not_followed(), 1);
        assert_eq!(counts.coverage(), Coverage::Incomplete);
    }

    /// Room for one claimant: the second set is not recorded, so not
    /// assessed, and the orphan's claim is unseen.
    #[test]
    fn sets_and_orphans_past_the_walks_bound_are_gaps() {
        let mut extra = deleted(file("first", 30, 512));
        extra.extend(deleted(file("second", 40, 512)));
        extra.push(orphan_stream(false, 50, 512));
        let mut vol = with_root(&extra);
        vol.chain(&[30]);
        vol.chain(&[40]);
        let limits = Limits {
            deleted_sets: 1,
            ..Limits::DEFAULT
        };

        let (counts, transcript) = analyse(&mut vol, limits);
        assert_eq!(decided(&transcript), ["c2 s5 passed"]);
        assert_eq!(counts.exfat_not_assessed(), 1);
        assert_eq!(counts.exfat_claimants_not_followed(), 1);
        assert_eq!(counts.coverage(), Coverage::Incomplete);
    }

    /// A deleted set whose checksum is wrong is not usable: never decided,
    /// and counted on its own line.
    #[test]
    fn a_deleted_set_the_walk_found_unusable_is_a_gap() {
        let mut bad = file("bad", 30, 512);
        bad[2][2] ^= 1;
        let mut vol = with_root(&deleted(bad));

        let (counts, transcript) = analyse(&mut vol, Limits::DEFAULT);
        assert!(decided(&transcript).is_empty(), "{transcript:?}");
        assert_eq!(counts.exfat_sets_unusable(), 1);
        assert_eq!(counts.exfat_not_assessed(), 0);
        assert_eq!(counts.coverage(), Coverage::Incomplete);
    }

    /// `busy`'s cluster is in use, so it is not read: a finding, and the
    /// claims it holds unseen, the volume's only gap.
    #[test]
    fn a_deleted_directory_not_read_is_a_gap() {
        let mut vol = with_root(&deleted(directory("busy", 20, 1)));
        vol.chain(&[20]);
        mark(&mut vol, &[3], &[20]);

        let (counts, transcript) = analyse(&mut vol, Limits::DEFAULT);
        assert_eq!(decided(&transcript), ["c2 s5 deleted directory"]);
        assert_eq!(counts.exfat_deleted_directories(), 1);
        assert_eq!(counts.exfat_deleted_directories_unread(), 1);
        assert_eq!(counts.exfat_claimants_not_followed(), 0);
        assert_eq!(counts.coverage(), Coverage::Incomplete);
    }

    /// `full` is read and holds a set that is not usable: a finding, and
    /// that set's claim unseen, the volume's only gap.
    #[test]
    fn a_set_inside_a_deleted_directory_that_is_not_usable_is_a_gap() {
        let mut vol = with_root(&deleted(directory("full", 21, 1)));
        vol.chain(&[21]);
        let mut bad = file("bad", 30, 512);
        bad[2][2] ^= 1;
        vol.put(21, 0, &deleted(bad));

        let (counts, transcript) = analyse(&mut vol, Limits::DEFAULT);
        assert_eq!(decided(&transcript), ["c2 s5 deleted directory"]);
        assert_eq!(counts.exfat_deleted_directories(), 1);
        assert_eq!(counts.exfat_deleted_directories_unread(), 0);
        assert_eq!(counts.exfat_claimants_not_followed(), 1);
        assert_eq!(counts.coverage(), Coverage::Incomplete);
    }

    /// The synthetic volume's 200 clusters give phase 2 a bound of 800
    /// cluster steps. Three chains of 150, content not read: each is
    /// charged 300, and the third does not fit.
    #[test]
    fn a_set_past_phase_2s_work_bound_is_not_assessed() {
        let mut extra = deleted(file("a", 40, 150 * 512));
        extra.extend(deleted(file("b", 40, 150 * 512)));
        extra.extend(deleted(file("c", 40, 150 * 512)));
        let mut vol = with_root(&extra);
        vol.chain(&(40..190).collect::<Vec<u32>>());

        let (counts, transcript) = analyse(&mut vol, Limits::DEFAULT);
        assert_eq!(
            decided(&transcript).last().copied(),
            Some("c2 s11 left by bound")
        );
        assert_eq!(counts.exfat_not_assessed(), 1);
        assert_eq!(counts.coverage(), Coverage::Incomplete);
    }

    /// Evidence that, once `arm` has been read, writes `write` and fails
    /// every later read beginning inside `fail`: a volume that changes, or
    /// fails, after the walk.
    struct Tripwire<'a> {
        image: &'a mut MemoryImage,
        arm: u64,
        armed: bool,
        write: Option<(u64, Vec<u8>)>,
        fail: Option<std::ops::Range<u64>>,
    }

    impl EvidenceReader for Tripwire<'_> {
        fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
            if self.armed && self.fail.as_ref().is_some_and(|r| r.contains(&offset)) {
                return Err(Error::from_io(
                    std::path::Path::new("<memory>"),
                    std::io::Error::other("the evidence failed"),
                ));
            }
            let read = self.image.read_exact_at(offset, buf);
            if !self.armed && offset == self.arm {
                self.armed = true;
                if let Some((at, bytes)) = self.write.take() {
                    self.image.write(at, &bytes);
                }
            }
            read
        }
    }

    /// Analyses through a [`Tripwire`] armed by the first read of the
    /// bitmap's cluster, 3: the bitmap check, phase 2's first read. The walk
    /// never reads it.
    fn analyse_tripped(
        vol: &mut Vol,
        write: Option<(u64, Vec<u8>)>,
        fail: Option<std::ops::Range<u64>>,
    ) -> (RunCounts, Vec<String>) {
        analyse_tripped_with(vol, write, fail, NOT_RECOVERING)
    }

    fn analyse_tripped_with(
        vol: &mut Vol,
        write: Option<(u64, Vec<u8>)>,
        fail: Option<std::ops::Range<u64>>,
        options: Options<'_>,
    ) -> (RunCounts, Vec<String>) {
        let boot = vol.boot.clone();
        let mut tripwire = Tripwire {
            image: &mut vol.image,
            arm: Vol::cluster_offset(3),
            armed: false,
            write,
            fail,
        };
        analyse_through(&mut tripwire, &boot, Limits::DEFAULT, options)
    }

    /// A byte of the set's name changes after the walk: read again, its
    /// checksum is wrong.
    #[test]
    fn a_set_unusable_on_re_read_is_not_assessed() {
        let mut vol = with_root(&deleted(file("gone", 30, 512)));
        vol.chain(&[30]);
        // Slot 7 is the set's name entry; byte 2 its first name unit.
        let at = Vol::cluster_offset(2) + 7 * 32 + 2;

        let (counts, transcript) = analyse_tripped(&mut vol, Some((at, vec![b'G'])), None);
        assert_eq!(decided(&transcript), ["c2 s5 unusable on re-read"]);
        assert_eq!(counts.exfat_not_assessed(), 1);
        assert_eq!(counts.coverage(), Coverage::Incomplete);
    }

    /// The root's second cluster fails once phase 2 has begun. The fourth
    /// set straddles into it: the three before are decided, it and the
    /// fifth are not assessed (S6-3).
    #[test]
    fn an_error_deciding_a_set_stops_phase_2_and_leaves_the_rest_not_assessed() {
        let mut extra = Vec::new();
        for (name, cluster) in [("a", 30), ("b", 31), ("c", 32), ("d", 33), ("e", 34)] {
            extra.extend(deleted(file(name, cluster, 512)));
        }
        let mut vol = with_root(&extra);
        for c in 30..35 {
            vol.chain(&[c]);
        }
        let second = Vol::cluster_offset(5);

        let (counts, transcript) = analyse_tripped(&mut vol, None, Some(second..second + 512));
        assert_eq!(
            decided(&transcript),
            ["c2 s5 passed", "c2 s8 passed", "c2 s11 passed"]
        );
        assert_eq!(
            transcript.last().map(String::as_str),
            Some("not assessed 2: decide")
        );
        assert_eq!(counts.exfat_passed(), 3);
        assert_eq!(counts.exfat_not_assessed(), 2);
        assert_eq!(counts.coverage(), Coverage::Incomplete);
    }

    /// No room in the queue for the root's directory `D`: it is not read,
    /// so the bitmap check would cover only part of what is in use, and
    /// phase 2 does not run (S6-1).
    #[test]
    fn a_live_directory_not_read_whole_leaves_every_deleted_set_not_assessed() {
        let mut extra = deleted(file("a", 30, 512));
        extra.extend(deleted(file("b", 0, 0)));
        let mut vol = with_root(&extra);
        vol.chain(&[30]);
        let limits = Limits {
            queued_directories: 0,
            ..Limits::DEFAULT
        };

        let (counts, transcript) = analyse(&mut vol, limits);
        assert!(decided(&transcript).is_empty(), "{transcript:?}");
        assert_eq!(
            transcript.last().map(String::as_str),
            Some("not assessed 2: not read whole")
        );
        assert!(!transcript.iter().any(|l| l.starts_with("claims")));
        assert_eq!(counts.exfat_not_assessed(), 2);
        assert_eq!(counts.coverage(), Coverage::Incomplete);
    }

    /// The bitmap marks no structure cluster but 7 in use, so it is not
    /// believed, and phase 2 does not run (S6-2).
    #[test]
    fn a_bitmap_not_believed_leaves_every_deleted_set_not_assessed() {
        let mut entries = standard_root();
        entries.extend(deleted(file("a", 0, 0)));
        let mut vol = structured(&entries);

        let (counts, transcript) = analyse(&mut vol, Limits::DEFAULT);
        assert!(decided(&transcript).is_empty(), "{transcript:?}");
        assert_eq!(
            transcript.last().map(String::as_str),
            Some("not assessed 1: bitmap not believed")
        );
        assert_eq!(counts.exfat_not_assessed(), 1);
        assert_eq!(counts.coverage(), Coverage::Incomplete);
    }

    /// Each reason a deleted directory was not read whole counts once, and
    /// each kind of set inside whose claim was not recorded: distinct powers
    /// of two show every field summed, and nothing else.
    #[test]
    fn every_deleted_directory_not_read_whole_and_every_unseen_set_inside_is_a_gap() {
        let read = DeletedDirectoriesRead {
            directories_read: 1 << 12,
            not_read_out_of_range: 1,
            not_read_malformed: 2,
            not_read_in_use: 4,
            not_read_before: 8,
            not_read_bound: 16,
            partly_read: 32,
            clusters_read: 1 << 13,
            sets_inside_recorded: 1 << 14,
            sets_inside_unusable: 64,
            sets_inside_unrecorded: 128,
        };
        assert_eq!(deleted_directory_gaps(&read), (63, 192));
    }

    /// The bitmap's cluster has no chain in the FAT, so the bitmap cannot
    /// be followed and phase 2 does not run (ADR-0022 Decision E).
    #[test]
    fn a_bitmap_that_cannot_be_followed_leaves_every_deleted_set_not_assessed() {
        let mut vol = with_root(&deleted(file("a", 0, 0)));
        vol.fat(3, 0);

        let (counts, transcript) = analyse(&mut vol, Limits::DEFAULT);
        assert_eq!(
            transcript.last().map(String::as_str),
            Some("not assessed 1: bitmap not opened")
        );
        assert_eq!(counts.exfat_not_assessed(), 1);
    }

    /// The deleted directory `gone` fails as it is read: phase 2 stops, and
    /// both deleted sets the walk met are not assessed (S6-3).
    #[test]
    fn an_error_reading_deleted_directories_stops_phase_2() {
        let mut extra = deleted(directory("gone", 20, 1));
        extra.extend(deleted(file("a", 0, 0)));
        let mut vol = with_root(&extra);
        vol.chain(&[20]);
        let gone = Vol::cluster_offset(20);

        let (counts, transcript) = analyse_tripped(&mut vol, None, Some(gone..gone + 512));
        assert!(decided(&transcript).is_empty(), "{transcript:?}");
        assert_eq!(
            transcript.last().map(String::as_str),
            Some("not assessed 2: deleted directories")
        );
        assert_eq!(counts.exfat_not_assessed(), 2);
    }

    /// The set's chain is 150, 151, whose FAT entries lie in the FAT's
    /// second sector (bytes 512 to 1023 of the FAT at sector 24), which
    /// following the claims is the first to read: it fails, and phase 2
    /// stops (S6-3).
    #[test]
    fn an_error_following_the_claims_stops_phase_2() {
        let mut vol = with_root(&deleted(file("far", 150, 1024)));
        vol.chain(&[150, 151]);
        let second = 24 * 512 + 512;

        let (counts, transcript) = analyse_tripped(&mut vol, None, Some(second..second + 512));
        assert!(decided(&transcript).is_empty(), "{transcript:?}");
        assert_eq!(
            transcript.last().map(String::as_str),
            Some("not assessed 1: claims")
        );
        assert_eq!(counts.exfat_not_assessed(), 1);
    }

    /// The root holds a deleted set, then an entry that stops it being
    /// read: the set it showed is not assessed, beside the root's own gap.
    #[test]
    fn deleted_sets_of_a_root_not_read_whole_are_not_assessed() {
        let mut vol = Vol::new();
        vol.chain(&[2]);
        vol.put(2, 0, &deleted(file("gone", 0, 0)));
        let mut critical = [0u8; 32];
        critical[0] = 0x84;
        vol.put(2, 3, &[critical]);

        let (counts, transcript) = analyse(&mut vol, Limits::DEFAULT);
        assert_eq!(counts.root_unread(), 1);
        assert_eq!(counts.exfat_not_assessed(), 1);
        assert_eq!(
            transcript.last().map(String::as_str),
            Some("not assessed 1: not read whole")
        );
    }

    // --- recovering under --recover (ADR-0022 Decisions G and H) ------------

    fn digest_of(bytes: &[u8]) -> crate::hash::Sha256Digest {
        let mut hasher = crate::hash::Sha256Hasher::new();
        hasher.update(bytes);
        hasher.finish().digest
    }

    /// `found`, two clusters at 30 and 31 in a chain, holding bytes that are
    /// not zeros: recoverable once content is read.
    fn with_found() -> (Vol, Vec<u8>) {
        let mut vol = with_root(&deleted(file("found", 30, 1024)));
        vol.chain(&[30, 31]);
        fill_clusters(&mut vol, &[30, 31]);
        (vol, [cluster_data(30), cluster_data(31)].concat())
    }

    fn recovering<'a>(output: Option<&'a Path>, reference: Option<Sha256Digest>) -> Options<'a> {
        Options {
            recover: true,
            reference,
            output,
        }
    }

    /// Read, written under its position's name, and counted as an artifact;
    /// nothing else counted, and the volume complete.
    #[test]
    fn a_recoverable_set_is_extracted_written_and_counted() {
        let dir = scratch("analysis-recovered");
        let (mut vol, data) = with_found();
        let boot = vol.boot.clone();

        let (counts, transcript) = analyse_through(
            &mut vol.image,
            &boot,
            Limits::DEFAULT,
            recovering(Some(&dir), None),
        );
        assert_eq!(decided(&transcript), ["c2 s5 recoverable"]);
        assert!(
            transcript.contains(&format!("extracted c2 s5 {}", digest_of(&data))),
            "{transcript:?}"
        );
        assert_eq!(
            fs::read(dir.join("p1-c2-s5-first-30.bin")).expect("written"),
            data
        );
        assert_eq!(counts.exfat_recoverable(), 1);
        assert_eq!(counts.exfat_artifacts(), 1);
        assert_eq!(counts.exfat_passed(), 0);
        assert_eq!(counts.exfat_not_extracted(), 0);
        assert_eq!(counts.exfat_not_delivered(), 0);
        assert_eq!(counts.coverage(), Coverage::Complete);
    }

    /// The reference is the content's digest, then another's: one match,
    /// then one difference.
    #[test]
    fn an_exfat_extraction_is_compared_with_the_reference() {
        let (mut vol, data) = with_found();
        let boot = vol.boot.clone();
        let reference = digest_of(&data);
        let options = recovering(None, Some(reference));

        let (counts, _) = analyse_through(&mut vol.image, &boot, Limits::DEFAULT, options);
        assert_eq!(counts.exfat_matched(), 1);
        assert_eq!(counts.exfat_differed(options), 0);

        let options = recovering(None, Some(digest_of(b"another file")));
        let (counts, _) = analyse_through(&mut vol.image, &boot, Limits::DEFAULT, options);
        assert_eq!(counts.exfat_matched(), 0);
        assert_eq!(counts.exfat_differed(options), 1);

        // No reference: nothing compared, so nothing differed. No
        // destination: nothing asked for, so nothing undelivered.
        let options = recovering(None, None);
        let (counts, _) = analyse_through(&mut vol.image, &boot, Limits::DEFAULT, options);
        assert_eq!(counts.exfat_artifacts(), 1);
        assert_eq!(counts.exfat_differed(options), 0);
        assert_eq!(counts.exfat_not_delivered(), 0);
        assert_eq!(counts.coverage(), Coverage::Complete);
    }

    /// A file of that name is there already: the artifact stands, it was
    /// not delivered, and that is a gap.
    #[test]
    fn an_exfat_artifact_not_delivered_is_a_gap() {
        let dir = scratch("analysis-not-delivered");
        fs::write(dir.join("p1-c2-s5-first-30.bin"), b"the operator's").expect("planting");
        let (mut vol, _) = with_found();
        let boot = vol.boot.clone();

        let (counts, _) = analyse_through(
            &mut vol.image,
            &boot,
            Limits::DEFAULT,
            recovering(Some(&dir), None),
        );
        assert_eq!(counts.exfat_artifacts(), 1);
        assert_eq!(counts.exfat_not_delivered(), 1);
        assert_eq!(counts.coverage(), Coverage::Incomplete);
    }

    /// `a`'s second cluster fails once phase 2 has begun. Deciding `a`
    /// reads only its first cluster, which is not zeros, so it is
    /// recoverable; extracting it reads the second and fails. `b` is still
    /// decided and extracted: an extraction error does not stop phase 2.
    #[test]
    fn an_extraction_error_is_counted_and_the_next_set_is_still_extracted() {
        let dir = scratch("analysis-not-extracted");
        let mut extra = deleted(file("a", 30, 1024));
        extra.extend(deleted(file("b", 40, 512)));
        let mut vol = with_root(&extra);
        vol.chain(&[30, 31]);
        vol.chain(&[40]);
        fill_clusters(&mut vol, &[30, 31, 40]);
        let second = Vol::cluster_offset(31);

        let (counts, transcript) = analyse_tripped_with(
            &mut vol,
            None,
            Some(second..second + 512),
            recovering(Some(&dir), None),
        );
        assert_eq!(
            decided(&transcript),
            ["c2 s5 recoverable", "c2 s8 recoverable"]
        );
        assert!(
            transcript.contains(&"not extracted c2 s5".to_string()),
            "{transcript:?}"
        );
        assert!(
            transcript.contains(&format!("extracted c2 s8 {}", digest_of(&cluster_data(40)))),
            "{transcript:?}"
        );
        assert_eq!(counts.exfat_recoverable(), 2);
        assert_eq!(counts.exfat_not_extracted(), 1);
        assert_eq!(counts.exfat_artifacts(), 1);
        assert!(!dir.join("p1-c2-s5-first-30.bin").exists());
        assert!(dir.join("p1-c2-s8-first-40.bin").exists());
        assert_eq!(counts.coverage(), Coverage::Incomplete);
    }
}
