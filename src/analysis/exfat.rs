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

use crate::evidence::EvidenceReader;
use crate::exfat_boot::{ExFatBootFailure, ExFatBootSector, parse_boot_region};
use crate::exfat_directory::{
    DirItem, DirectorySource, DirectorySummary, ExFatDirectoryError, ExFatReader, Limits,
};
use crate::filesystem::{UnknownReason, VolumeExtent};
use crate::partition::MbrPartition;

use super::{Event, MAX_DEPTH, RunCounts, Sink};

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
    counts: &mut RunCounts,
    sink: &mut dyn Sink,
) {
    match parse_boot_region(evidence, extent) {
        Ok(boot) => report_volume(evidence, &boot, extent, Limits::DEFAULT, counts, sink),
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
    counts: &mut RunCounts,
    sink: &mut dyn Sink,
) -> bool {
    match parse_boot_region(evidence, extent) {
        Ok(boot) => {
            sink.record(Event::ExFatIdentifiedFromBackup {
                partition,
                sector_zero,
            });
            report_volume(evidence, &boot, extent, Limits::DEFAULT, counts, sink);
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

/// Reads every directory reachable from the root of a volume.
///
/// Breadth first, each directory once. `limits` bounds the work: a directory
/// beyond [`MAX_DEPTH`] is not read, and the volume's cluster cap, where it is
/// reached, leaves the directories not yet read counted and not read, never a
/// hang.
fn report_volume<R: EvidenceReader>(
    evidence: &mut R,
    boot: &ExFatBootSector,
    extent: VolumeExtent,
    limits: Limits,
    counts: &mut RunCounts,
    sink: &mut dyn Sink,
) {
    sink.record(Event::ExFatBootRegion(boot));

    let mut reader = ExFatReader::new(evidence, boot, extent, limits);
    let mut pending: VecDeque<Pending> = VecDeque::new();

    // Directories named and never queued, because queueing them would hold
    // more directories than `limits.queued_directories` allows.
    let mut unqueued = 0usize;

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
            // counted as analysed.
            sink.record(Event::ExFatRootDirectoryUnread(&error));
            counts.root_unread += 1;
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
            sink.record(Event::ExFatBitmap { position, bitmap });
        }
        DirItem::Refused { position, refusal } => {
            counts.exfat_sets_refused += 1;
            sink.record(Event::ExFatSetRefused { position, refusal });
        }
        DirItem::UnusedFileSet { .. } => counts.exfat_deleted_sets_not_examined += 1,
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
    use crate::exfat_directory::tests::{Vol, directory, extent, file, seal};

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
                _ => "other".to_string(),
            };
            self.0.push(line);
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

    fn analyse(vol: &mut Vol, limits: Limits) -> (RunCounts, Vec<String>) {
        let mut counts = RunCounts::default();
        let mut transcript = Transcript(Vec::new());
        let boot = vol.boot.clone();
        report_volume(
            &mut vol.image,
            &boot,
            extent(),
            limits,
            &mut counts,
            &mut transcript,
        );
        (counts, transcript.0)
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
        assert_eq!(counts.exfat_deleted_sets_not_examined(), 2);
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
}
