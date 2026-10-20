//! Taphonomy command-line interface.
//!
//! The CLI is an interface to the library. It parses arguments, opens and
//! hashes the evidence, and calls `taphonomy::analysis::run`, which walks
//! the image and reports what it finds as a stream of events. This file's
//! [`Sink`] renders each event to standard output or standard error, in the
//! same words and the same order the run reported them before the walk
//! moved into the library. It contains no recovery logic: the partition
//! loop, the directory walk, the orphan search, and the run's counts live
//! in `taphonomy::analysis`. `ADR-0018`.
//!
//! See `docs/development/RESEARCH_LOG.md` conclusion 6 and `CLAUDE.md`
//! section 11.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use taphonomy::EvidenceFile;
use taphonomy::Sha256Digest;
use taphonomy::analysis::exfat::{ExFatNotRead, ExFatReached};
use taphonomy::analysis::{
    Coverage, DirectoryPart, Event, MAX_DEPTH, NotReadReason, Options, Reached, RunCounts, Sink,
};
use taphonomy::confidence::Confidence;
use taphonomy::exfat_boot::{BootRegion, ExFatBootSector};
use taphonomy::exfat_directory::{ExFatPosition, escaped_name};
use taphonomy::fat_directory::{
    DeletedKind, Directory, Entry, EntryKind, FirstByte, associate, recovered_name,
};
use taphonomy::fat_recovery::{Listing, Output};
use taphonomy::fat32::Fat32BootSector;
use taphonomy::filesystem::Identification;
use taphonomy::partition::MbrPartition;
use taphonomy::validation::Outcome;

/// One line of usage, printed on any argument error.
const USAGE: &str = "usage: taphonomy <evidence-image> [--recover] \
                     [--output <directory>] [--reference-digest <hex>]";

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);

    let Some(path) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };

    // Two flags, matched exactly. ADR-0010 Decision B puts reading a deleted
    // file's content behind an explicit request, and ADR-0013 section 13 sets
    // the ceiling this reasoning stops at: a third flag, or a flag taking
    // more than one value, is where an argument parser is reconsidered.
    let mut options = Options {
        recover: false,
        reference: None,
        output: None,
    };

    // Owned here so that `options` can borrow it for the rest of the run,
    // which keeps `Options` `Copy` and the reporting functions taking it by
    // value.
    let mut output = None;

    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--recover") => options.recover = true,
            // A repeated flag is refused rather than letting the last one
            // win: which directory or digest the operator meant is exactly
            // the ambiguity `CLAUDE.md` section 39 says must be an error.
            Some("--output") => {
                if output.is_some() {
                    eprintln!("error: --output given more than once");
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
                let Some(value) = args.next() else {
                    eprintln!("error: --output requires a directory");
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                };

                output = Some(PathBuf::from(value));
            }
            Some("--reference-digest") => {
                if options.reference.is_some() {
                    eprintln!("error: --reference-digest given more than once");
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
                let Some(value) = args.next() else {
                    eprintln!("error: --reference-digest requires a digest");
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                };

                // ADR-0013 section 6.1. A digest that cannot be read stops
                // the run before any evidence is opened, which is the one
                // thing M8 fails closed on.
                match value.to_str().map(Sha256Digest::from_hex) {
                    Some(Ok(digest)) => options.reference = Some(digest),
                    Some(Err(e)) => {
                        eprintln!("error: --reference-digest: {e}");
                        eprintln!("{USAGE}");
                        return ExitCode::from(2);
                    }
                    None => {
                        eprintln!("error: --reference-digest is not valid utf-8");
                        eprintln!("{USAGE}");
                        return ExitCode::from(2);
                    }
                }
            }
            _ => {
                eprintln!("error: unexpected argument");
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }

    options.output = output.as_deref();

    // `ADR-0013` Decision H. A reference is useless without content to
    // compare, and implying --recover would enable reading a deleted file's
    // content without the operator asking for it.
    if options.reference.is_some() && !options.recover {
        eprintln!("error: --reference-digest needs --recover, which reads content");
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }

    // `ADR-0014` Appendix B.10. A run that analysed nothing past the digest
    // exits non-zero, because Appendix A.4 and B.3 measured such runs
    // reporting their error on stderr, printing nothing on stdout that says
    // so, and exiting zero: `SAFETY.md` section 12 forbids a failure being
    // converted silently into a partial success.
    //
    // A run that covered part of the evidence exits zero. Reading no
    // subdirectory makes that the ordinary state of any DCF camera card, and
    // a code that fires on almost every run is one operators learn to
    // ignore. The gap is stated on stdout, where the harm was measured.
    //
    // 1 stays an evidence that could not be opened or hashed, 2 an argument
    // error, and a differing reference stays zero under `ADR-0013` section 3.
    // `ADR-0015` Decision A. Writing is more than reading, so it inherits
    // `ADR-0010` Decision B's requirement that the operator ask.
    if options.output.is_some() && !options.recover {
        eprintln!("error: --output needs --recover, which reads content");
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }

    // `ADR-0015` Decision B as Appendix A.3 corrects it. Before the
    // evidence is opened, because `SAFETY.md` section 12 lists a
    // source/destination collision among the conditions the tool fails
    // closed on.
    if let Some(directory) = options.output
        && let Err(message) = check_destination(directory, Path::new(&path))
    {
        eprintln!("error: {message}");
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }

    match inspect(&path, options) {
        Ok(Coverage::NothingAnalysed) => ExitCode::from(3),
        Ok(Coverage::Complete | Coverage::Incomplete) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Refuses a destination that is, or would contain, the evidence.
///
/// `ADR-0015` Decision B, corrected by its Appendix A.3. What must never
/// happen is a write into the volume under analysis: free space and
/// unrecovered evidence are the same bytes, so an artifact written there
/// lands on deleted files that have not been recovered yet, and any write
/// changes the digest every finding in the run is anchored to. `SAFETY.md`
/// section 3.3 forbids it and section 4 requires a separate destination.
///
/// With the evidence an image file, the volume is not mounted and the host
/// filesystem has no path into it, so the reachable case is a destination
/// holding the evidence file itself. A destination that merely shares a
/// filesystem with the image is permitted and draws no warning: Appendix
/// A.2 records that established practice constrains the source filesystem
/// rather than the device an image happens to sit on.
///
/// The device comparison that enforces this where the evidence is a block
/// device is not written here, because no block device can be named as
/// evidence yet and a check no input can reach cannot be tested.
/// `ADR-0015` Appendix A.3 records what it is to be.
fn check_destination(directory: &Path, evidence: &Path) -> Result<(), String> {
    let metadata =
        fs::metadata(directory).map_err(|e| format!("--output {}: {e}", directory.display()))?;

    if !metadata.is_dir() {
        return Err(format!(
            "--output {} is not a directory",
            directory.display()
        ));
    }

    // Canonicalised, so that a destination reaching the evidence's
    // directory by a different path is still refused.
    let destination = directory
        .canonicalize()
        .map_err(|e| format!("--output {}: {e}", directory.display()))?;
    let source = evidence
        .canonicalize()
        .map_err(|e| format!("{}: {e}", evidence.display()))?;

    if source.parent() == Some(destination.as_path()) {
        return Err(format!(
            "--output {} holds the evidence",
            directory.display()
        ));
    }

    Ok(())
}

/// Opens the evidence, hashes it, and renders the library's run of it.
///
/// `ADR-0018` Decision A: opening and hashing the evidence and every digest
/// line stay here, and the walk that follows is `taphonomy::analysis::run`,
/// rendered by [`StdoutSink`] below.
fn inspect(path: &std::ffi::OsStr, options: Options<'_>) -> Result<Coverage, taphonomy::Error> {
    let mut evidence = EvidenceFile::open(path)?;
    let reported = evidence.reported_size();
    let result = evidence.digest()?;

    println!("path         {}", evidence.path().display());
    println!("size         {reported} bytes");
    println!("read         {} bytes", result.bytes_read);
    println!("sha256       {}", result.digest);

    if result.bytes_read != reported {
        println!();
        println!("WARNING: bytes read does not match reported size.");
        println!("The digest covers {} bytes only.", result.bytes_read);
    }

    println!();

    let mut sink = StdoutSink::default();
    let counts = taphonomy::analysis::run(&mut evidence, result.bytes_read, options, &mut sink);

    print_summary(&counts, options);
    print_caveats(&counts, options);

    Ok(counts.coverage())
}

/// Renders a run's events to standard output and standard error.
///
/// `ADR-0018` Decision B. Every match arm renders one event's fields; none
/// carries text formatted elsewhere. This is the only `Sink` the CLI has,
/// and the only place in this file that prints what a run found.
///
/// Whether a content heading has printed is state the sink keeps for
/// itself: `ADR-0018` follow-up moved that decision out of the library,
/// which now reports `listing` and `part` as findings on every content
/// event and lets a sink do whatever rendering, or none, it likes with
/// them. A `Directory` event starts a new directory and resets both.
#[derive(Default)]
struct StdoutSink {
    /// Whether this directory's current-listing heading has printed.
    entries_heading_printed: bool,

    /// Whether this directory's residue heading has printed.
    residue_heading_printed: bool,
}

impl StdoutSink {
    /// Prints a content heading before the first finding of a
    /// `(listing, part)` pass and never again for the same directory,
    /// exactly as `report_recovery` used to decide for itself.
    /// `ADR-0017` Decision C: an orphaned listing's files are not
    /// deleted, so its heading says so.
    fn head_content(&mut self, listing: Listing, part: DirectoryPart) {
        let printed = match part {
            DirectoryPart::Entries => &mut self.entries_heading_printed,
            DirectoryPart::Residue => &mut self.residue_heading_printed,
        };
        if *printed {
            return;
        }
        *printed = true;

        let title = match listing {
            Listing::Walked => "deleted content",
            Listing::Orphaned => "orphaned content",
        };
        println!("      {title}");
    }
}

impl Sink for StdoutSink {
    fn record(&mut self, event: Event<'_>) {
        match event {
            Event::TableUnread(error) => eprintln!("error: {error}"),
            Event::TableRejected(error) => eprintln!("error: {error}"),
            Event::DiskSignature(signature) => println!("disk signature  {signature:#010x}"),
            Event::PartitionListed(p) => println!(
                "partition {}   type {:#04x}   start {}   sectors {}   bootable {}",
                p.index,
                p.partition_type,
                p.start_lba,
                p.sector_count,
                p.is_bootable()
            ),
            Event::PartitionsListed => println!(),
            Event::GptDetected => println!("GPT detected: not supported"),
            Event::Anomalies(anomalies) => {
                println!();
                println!("anomalies");
                for a in anomalies {
                    println!("  {a}");
                }
            }
            Event::PartitionUnread { partition, error } => {
                eprintln!("error: partition {}: {error}", partition.index);
            }
            Event::PartitionIdentified {
                partition,
                identification,
                type_match,
            } => print_partition_identified(partition, identification, type_match),
            Event::ExFatIdentifiedFromBackup {
                partition,
                sector_zero,
            } => {
                println!(
                    "partition {}   filesystem exFAT, identified from its backup boot region",
                    partition.index
                );
                println!("    sector 0             {sector_zero}");
            }
            Event::BootSectorRejected(error) => println!("    BOOT SECTOR REJECTED: {error}"),
            Event::BootSector(boot) => print_boot_sector(boot),
            Event::RootDirectoryUnread(error) => {
                println!("    ROOT DIRECTORY NOT ENUMERATED: {error}");
            }
            Event::Directory { reached, directory } => {
                self.entries_heading_printed = false;
                self.residue_heading_printed = false;
                print_directory(&heading_of(reached), directory);
            }
            Event::DirectoryNotRead { reached, reason } => {
                println!("    {}", heading_of(reached));
                match reason {
                    NotReadReason::TooDeep => {
                        println!("      NOT READ: deeper than {MAX_DEPTH} levels below the root");
                    }
                    NotReadReason::AlreadyRead { cluster } => println!(
                        "      NOT READ: cluster {cluster} was already read as a directory"
                    ),
                    NotReadReason::Refused(reason) => println!("      NOT READ: {reason}"),
                    NotReadReason::Enumeration(error) => println!("      NOT READ: {error}"),
                }
            }
            Event::ListingMayContinue(cluster) => {
                println!("      listing may continue: cluster {cluster} holds no terminator");
            }
            Event::OrphanSearchStopped { cluster, error } => {
                println!("    ORPHAN SEARCH STOPPED at cluster {cluster}: {error}");
            }
            Event::OrphanedListingUnread {
                holder,
                name,
                first_cluster,
            } => {
                println!("    directory {name}, listed in orphaned directory c{holder}");
                println!("      NOT READ: cluster {first_cluster} was not found by the search");
            }
            Event::NotAssessed {
                listing,
                part,
                entry,
                error,
            } => {
                self.head_content(listing, part);
                println!("        {:<9} NOT ASSESSED: {error}", entry_position(entry));
            }
            Event::Ineligible {
                listing,
                part,
                entry,
                reason,
            } => {
                self.head_content(listing, part);
                println!("        {:<9} no content: {reason}", entry_position(entry));
            }
            Event::RunBroken {
                listing,
                part,
                entry,
                run,
                first_allocated,
            } => {
                self.head_content(listing, part);
                let detail = format!(
                    "run {}-{}, {} bytes, REFUSED: cluster {first_allocated} is in use",
                    run.first_cluster,
                    run.last_cluster(),
                    run.file_size
                );
                println!("        {:<9} {detail}", entry_position(entry));
            }
            Event::Recoverable {
                listing,
                part,
                entry,
                run,
            } => {
                self.head_content(listing, part);
                let detail = format!(
                    "run {}-{}, {} bytes, {} slack, every cluster free",
                    run.first_cluster,
                    run.last_cluster(),
                    run.file_size,
                    run.slack_bytes
                );
                println!("        {:<9} {detail}", entry_position(entry));
            }
            Event::Extracted {
                listing: _,
                part: _,
                entry: _,
                extraction,
                validation,
                readback_matches,
            } => {
                // `docs/SAFETY.md` section 10 requires the recovered-file
                // hash and the validation hash to be distinguishable where
                // both are reported.
                println!(
                    "        {:<9} recovered sha256 {} over {} bytes",
                    "", extraction.digest, extraction.bytes_hashed
                );

                if let Some(output) = &extraction.output {
                    print_output(output, readback_matches);
                }

                match (validation.outcome, validation.reference) {
                    (Outcome::Match, Some(reference)) => {
                        println!("        {:<9} reference sha256 {reference} MATCHES", "");
                    }
                    (Outcome::Differs, Some(reference)) => {
                        println!("        {:<9} reference sha256 {reference} DIFFERS", "");
                    }
                    // `NotAttempted` carries no reference, and a reference is
                    // always carried where a comparison happened.
                    _ => {}
                }
            }
            Event::NotExtracted {
                listing: _,
                part: _,
                entry: _,
                error,
            } => {
                println!("        {:<9} NOT EXTRACTED: {error}", "");
            }
            Event::ExFatBootRegion(boot) => print_exfat_boot_region(boot),
            Event::ExFatBootRejected(failure) => {
                println!("    BOOT REGION REJECTED: {failure}");
            }
            Event::ExFatTexFat(region) => {
                println!(
                    "    TexFAT: two FATs in the {} boot region, not supported",
                    exfat_region_name(region)
                );
            }
            Event::ExFatRootDirectoryUnread(error) => {
                println!("    ROOT DIRECTORY NOT ENUMERATED: {error}");
            }
            Event::ExFatDirectoryStart { reached } => {
                println!("    {}", exfat_heading(reached));
            }
            Event::ExFatEntry { position, file } => {
                let name = escaped_name(&file.name);
                let place = exfat_position(position);
                if file.is_directory() {
                    println!(
                        "      {place} {:<13} {name:<13} cluster {}",
                        "directory", file.first_cluster
                    );
                } else {
                    let valid = if file.valid_data_length == file.data_length {
                        String::new()
                    } else {
                        format!(", {} valid", file.valid_data_length)
                    };
                    println!(
                        "      {place} {:<13} {name:<13} {} bytes{valid}, cluster {}",
                        "file", file.data_length, file.first_cluster
                    );
                }
            }
            Event::ExFatVolumeLabel { position, label } => println!(
                "      {} {:<13} {}",
                exfat_position(position),
                "volume label",
                escaped_name(label)
            ),
            Event::ExFatBitmap { position, bitmap } => println!(
                "      {} {:<13} cluster {}, {} bytes",
                exfat_position(position),
                "bitmap",
                bitmap.first_cluster,
                bitmap.data_length
            ),
            Event::ExFatSetRefused { position, refusal } => println!(
                "      {} {:<13} {refusal}",
                exfat_position(position),
                "not used"
            ),
            Event::ExFatDirectoryEnd { summary, .. } => {
                println!(
                    "      entries            {}, {} files, {} directories",
                    summary.entries_read, summary.files, summary.directories
                );
                if summary.unused_file_sets > 0 {
                    println!(
                        "      deleted entry sets {}, not examined",
                        summary.unused_file_sets
                    );
                }
                if summary.entries_after_end_marker > 0 {
                    println!(
                        "      entries after end marker {}, not read",
                        summary.entries_after_end_marker
                    );
                }
            }
            Event::ExFatDirectoryNotRead { reached, reason } => {
                println!("    {}", exfat_heading(reached));
                match reason {
                    ExFatNotRead::TooDeep => {
                        println!("      NOT READ: deeper than {MAX_DEPTH} levels below the root");
                    }
                    ExFatNotRead::Error(error) => println!("      NOT READ: {error}"),
                }
            }
            Event::ExFatDirectoryCapReached {
                cap,
                directories_unread,
            } => println!(
                "    {directories_unread} {} not read: the volume has more than {cap} directory clusters",
                directories(directories_unread)
            ),
            Event::ExFatQueueBoundReached {
                bound,
                directories_unread,
            } => println!(
                "    {directories_unread} {} not read: more than {bound} were waiting to be read",
                directories(directories_unread)
            ),
        }
    }
}

/// The heading an exFAT directory is reported under.
///
/// The path is built from names that are UTF-16 in the evidence, escaped as
/// everywhere else, and never joined with anything but a slash the escape
/// leaves no way to forge.
fn exfat_heading(reached: ExFatReached<'_>) -> String {
    match reached {
        ExFatReached::Root => "root directory".to_string(),
        ExFatReached::Walked { components } => {
            let mut path = String::new();
            for name in components {
                path.push('/');
                path.push_str(&escaped_name(name));
            }
            format!("directory {path}")
        }
    }
}

fn exfat_region_name(region: BootRegion) -> &'static str {
    match region {
        BootRegion::Main => "main",
        BootRegion::Backup => "backup",
    }
}

/// Prints an exFAT volume's boot region, in the lines FAT32's boot sector
/// has where the meaning is the same.
fn print_exfat_boot_region(boot: &ExFatBootSector) {
    let found = if boot.located_by_search {
        ", found by trying each sector size"
    } else {
        ""
    };
    println!(
        "    boot region          {}{found}",
        exfat_region_name(boot.region)
    );
    println!("    bytes per sector     {}", boot.bytes_per_sector());
    println!(
        "    sectors per cluster  {}",
        1u32 << boot.sectors_per_cluster_shift
    );
    println!("    cluster count        {}", boot.cluster_count);
    println!("    first data sector    {}", boot.cluster_heap_offset);
    println!("    root cluster         {}", boot.root_cluster);
    println!("    volume id            {:#010x}", boot.volume_serial);

    if !boot.observations.is_empty() {
        println!("    boot region observations");
        for o in &boot.observations {
            println!("      {o}");
        }
    }
}

/// The width of the position column on an exFAT entry line: that of
/// `c4294967295 s1048575`, the largest cluster number and the last slot of
/// a 32 MiB cluster, which holds 1,048,576 entries. No position a volume
/// can hold is wider, so every entry keeps one line and its columns.
const EXFAT_POSITION_WIDTH: usize = 20;

/// The `c{cluster} s{slot}` position column of an exFAT entry line.
fn exfat_position(position: ExFatPosition) -> String {
    let place = format!("c{} s{}", position.cluster, position.slot);
    format!("{place:<EXFAT_POSITION_WIDTH$}")
}

/// The noun for a count of directories.
fn directories(count: usize) -> &'static str {
    if count == 1 {
        "directory"
    } else {
        "directories"
    }
}

/// The `c{cluster} s{slot}` position column shared by every content line.
fn entry_position(entry: &Entry) -> String {
    format!("c{} s{}", entry.cluster, entry.slot)
}

/// The heading a directory, or a directory not read, is reported under.
fn heading_of(reached: Reached<'_>) -> String {
    match reached {
        Reached::Root => "root directory".to_string(),
        Reached::Walked { path, deleted } => {
            let kind = if deleted {
                "deleted directory"
            } else {
                "directory"
            };
            format!("{kind} {path}")
        }
        Reached::Orphaned { cluster, parent } => {
            let parent = parent.map_or_else(|| "?".to_string(), |p| p.to_string());
            format!("orphaned directory c{cluster}, .. names c{parent}")
        }
    }
}

/// Prints what a partition's volume boot record identified.
fn print_partition_identified(
    partition: &MbrPartition,
    identification: &Identification,
    type_match: Option<bool>,
) {
    match identification {
        Identification::Identified { filesystem, .. } => {
            println!("partition {}   filesystem {filesystem}", partition.index);
        }
        Identification::Unknown { reason } => {
            println!(
                "partition {}   filesystem unknown ({reason})",
                partition.index
            );
        }
    }

    if let Identification::Identified {
        geometry: Some(g), ..
    } = identification
    {
        println!("    bytes per sector     {}", g.bytes_per_sector);
        println!("    sectors per cluster  {}", g.sectors_per_cluster);
        println!("    cluster count        {}", g.cluster_count);
        println!("    first data sector    {}", g.first_data_sector());
    }

    if type_match == Some(false)
        && let Some(filesystem) = identification.filesystem()
    {
        println!(
            "    MISMATCH: declared type {:#04x} disagrees with observed filesystem {filesystem}",
            partition.partition_type
        );
    }

    if let Identification::Identified { observations, .. } = identification
        && !observations.is_empty()
    {
        println!("    observations");
        for o in observations {
            println!("      {o}");
        }
    }
}

/// Prints a FAT32 volume's validated boot sector.
fn print_boot_sector(boot: &Fat32BootSector) {
    println!("    root cluster         {}", boot.root_cluster);
    println!("    backup boot sector   {}", boot.backup_boot_sector);

    match boot.active_fat() {
        Some(n) => println!("    FAT mirroring        disabled, FAT {n} active"),
        None => println!("    FAT mirroring        enabled"),
    }

    if let Some(id) = boot.volume_id {
        println!("    volume id            {id:#010x}");
    }

    if let Some(label) = &boot.volume_label {
        println!("    volume label         {label}");
    }

    if !boot.observations.is_empty() {
        println!("    boot sector observations");
        for o in &boot.observations {
            println!("      {o}");
        }
    }
}

/// Prints a directory's statistics, entries, residue and observations.
///
/// The root's block is printed exactly as it was before subdirectories were
/// read, so every heading below the root reuses it unchanged.
fn print_directory(heading: &str, directory: &Directory) {
    let chain: Vec<String> = directory.clusters.iter().map(|c| c.to_string()).collect();

    println!("    {heading}");
    println!("      cluster chain      {}", chain.join(" -> "));
    println!("      entries            {}", directory.entries.len());
    println!("      short entries      {}", directory.short_entry_count());
    println!("      long name entries  {}", directory.long_name_count());
    println!("      deleted entries    {}", directory.deleted_count());

    for (index, entry) in directory.entries.iter().enumerate() {
        print_entry(entry, &directory.entries, index);
    }

    if !directory.residue.is_empty() {
        println!("      past the terminator");
        for (index, entry) in directory.residue.iter().enumerate() {
            print_entry(entry, &directory.residue, index);
        }
    }

    if !directory.observations.is_empty() {
        println!("      directory observations");
        for o in &directory.observations {
            println!("        {o}");
        }
    }
}

/// Says what became of a written artifact, where one was asked for.
///
/// `ADR-0015` Decision H. The comparison is made in the library rather than
/// here, because a difference between what was read and what landed is a
/// finding to report and not an error to raise: `ADR-0013` section 3 treats
/// a differing reference the same way, and the exit status is unchanged by
/// either. `readback_matches` carries that comparison's answer; it is
/// `Some` exactly where `output` is [`Output::Written`].
fn print_output(output: &Output, readback_matches: Option<bool>) {
    match output {
        Output::Written { path, .. } => {
            let verdict = if readback_matches == Some(true) {
                "matches what was read"
            } else {
                "DIFFERS from what was read"
            };

            println!("        {:<9} written {} {verdict}", "", path.display());
        }
        Output::Unverified { path, message } => {
            println!(
                "        {:<9} written {} NOT VERIFIED: {message}",
                "",
                path.display()
            );
        }
        Output::Exists { path } => {
            println!("        {:<9} NOT WRITTEN: {} exists", "", path.display());
        }
        Output::NotCreated { path, message } => {
            println!(
                "        {:<9} NOT WRITTEN: {} could not be created: {message}",
                "",
                path.display()
            );
        }
        Output::Failed {
            path,
            message,
            removed,
        } => {
            let partial = if *removed { "" } else { ", partial file left" };

            println!(
                "        {:<9} NOT WRITTEN: {}: {message}{partial}",
                "",
                path.display()
            );
        }
    }
}

/// Column the gap counts print in, set by the longest label below.
const GAP_LABEL_WIDTH: usize = 26;

/// Prints what the run covered and what it produced, once per run.
///
/// `ADR-0003` section 4.7 requires a session to report counts per level and
/// forbids a single combined success figure. Under `ADR-0014` Decision A one
/// level is reachable, so the level is one line here and the count on it is
/// what varies: Appendix A.10 keeps the word off the per-entry lines, and
/// Appendix B.8 keeps the count on this one.
///
/// The coverage line comes first because it scopes every count below it, and
/// each line beneath states what was not analysed rather than what was
/// there, which Appendix A.6 requires: the tool cannot know what a directory
/// it did not read held, or what a partition it could not identify carried.
///
/// Printed for every run, including one whose partition table failed.
/// Appendix A.4 measured that such a run reports its error on stderr and
/// says nothing on stdout, so a reader capturing stdout alone sees a digest
/// and no indication that nothing else was analysed.
fn print_summary(counts: &RunCounts, options: Options<'_>) {
    println!();
    println!("coverage     {}", counts.coverage());

    print_count("volumes analysed", counts.volumes_analysed());
    print_gap("partition table not read", counts.table_unread());
    print_gap("partition table not parsed", counts.table_rejected());
    print_gap("partition entries rejected", counts.partitions_anomalous());
    print_gap("GPT not analysed", counts.gpt());
    print_gap("partitions not read", counts.partition_unread());
    print_gap("filesystems not identified", counts.unidentified());
    print_gap("filesystems not analysed", counts.unsupported());
    print_gap("boot sectors rejected", counts.boot_rejected());
    print_gap("root directories not read", counts.root_unread());
    print_gap("entries not assessed", counts.not_assessed());
    print_gap("entries not extracted", counts.not_extracted());
    print_gap("artifacts not written", counts.not_delivered());
    print_gap("directories not read", counts.directories_unread());
    print_gap("listings that may continue", counts.listings_may_continue());
    print_gap("orphan searches stopped", counts.orphan_search_stopped());
    print_gap("entry sets not used", counts.exfat_sets_refused());
    print_gap(
        "deleted sets not examined",
        counts.exfat_deleted_sets_not_examined(),
    );
    print_gap(
        "entries after end marker",
        counts.exfat_end_marker_residue(),
    );

    // Only `--recover` produces an artifact, so without it the line would
    // report a zero that the invocation already determined.
    if options.recover {
        let artifacts = counts.artifacts(options);
        if artifacts == 0 {
            println!("artifacts    0");
        } else {
            println!("artifacts    {artifacts} {}", Confidence::of_inferred_run());
        }
    }

    // Only where a reference was supplied. Where none was, the caveat block
    // below already states that nothing was validated, and saying it twice
    // would present one fact about the invocation as two findings.
    if options.reference.is_some() {
        let matched = counts.matched();
        let differed = counts.differed(options);
        println!("compared     {matched} matched, {differed} differed");
    }

    // Deleted entries that produced no artifact, by what stopped them.
    // `ADR-0013` Appendix C.4: an operator asking whether a file is present
    // needs to know the question went unanswered for part of the volume.
    let mut unrecovered = Vec::new();
    push_part(&mut unrecovered, counts.refused(), "refused");
    push_part(&mut unrecovered, counts.not_read(options), "not read");
    push_part(&mut unrecovered, counts.not_assessed(), "not assessed");
    push_part(&mut unrecovered, counts.not_extracted(), "not extracted");
    push_part(&mut unrecovered, counts.empty(), "size zero");
    push_part(
        &mut unrecovered,
        counts.reserved_cluster(),
        "reserved first cluster",
    );
    push_part(
        &mut unrecovered,
        counts.run_out_of_range(),
        "run out of range",
    );

    if !unrecovered.is_empty() {
        println!("unrecovered  {}", unrecovered.join(", "));
    }
}

/// One line of the coverage block.
fn print_count(label: &str, count: usize) {
    println!("  {label:<width$} {count}", width = GAP_LABEL_WIDTH);
}

/// One line of the coverage block, printed only where there is something to
/// report. A zero would state that a kind of gap did not occur, which reads
/// as a finding about the evidence rather than about the run.
fn print_gap(label: &str, count: usize) {
    if count > 0 {
        print_count(label, count);
    }
}

/// One part of the unrecovered line, left out where its count is zero.
fn push_part(parts: &mut Vec<String>, count: usize, label: &str) {
    if count > 0 {
        parts.push(format!("{count} {label}"));
    }
}

/// Prints what the reported entries oblige, once per volume.
///
/// `ADR-0003` section 4.2 and `ADR-0010` Decision C for the first
/// paragraph, `ADR-0013` section 8.2 and Decision E for the others. None of
/// these is a summary of findings, which `ADR-0013` Decision I reserves for
/// M9: each states what a finding does not establish.
///
/// Printed once per run. `ADR-0013` Appendix C.6 made this once per volume
/// when `report_recovery` printed it directly; `ADR-0003` section 4.7
/// requires a statement about a session rather than a volume, so the value
/// now travels to `inspect` and prints there.
///
/// Indented to zero for the same reason. At volume indentation it would
/// read as a statement about the last volume printed rather than about the
/// run it now covers.
fn print_caveats(counts: &RunCounts, options: Options<'_>) {
    if counts.free_runs() == 0 {
        return;
    }

    // ADR-0003 section 4.2 and ADR-0010 Decision C. Saying only that the run
    // is free would let a reader take it for a finding. It is the absence of
    // contrary evidence, which is not the same thing and never becomes it.
    println!();
    println!("A free run means nothing has claimed those");
    println!("clusters since the entry lost them. It is not");
    println!("evidence that the content there is this file's.");

    // ADR-0013 section 8.2. A match is byte equality with what the operator
    // supplied, and where the content is not distinctive that is weaker
    // evidence than it reads as.
    if counts.matched() > 0 {
        println!();
        println!("A match establishes that these bytes are the");
        println!("reference's, byte for byte. Where content is not");
        println!("distinctive, other clusters could hold the same");
        println!("bytes.");
    }

    // ADR-0013 Decision E. The causes are listed and none is chosen,
    // because the evidence does not distinguish between them.
    if counts.differed(options) > 0 {
        println!();
        println!("A differing digest does not say which of these");
        println!("happened: the file was fragmented, clusters of");
        println!("the run were reused, the recorded size is wrong,");
        println!("or the reference is another file.");
    }

    // Attached to the paragraph above rather than set apart: each is one
    // line about the invocation, not a caveat of its own.
    if !options.recover {
        println!("No content read. Pass --recover to read it.");
    } else if options.reference.is_none() {
        println!("No reference supplied. Nothing was validated.");
    }
}

/// A name the parser could render, or a marker that it could not.
///
/// A name field is eleven bytes of untrusted evidence. When the parser
/// refuses it, the bytes appear in an observation as hexadecimal rather than
/// reaching the terminal as characters.
fn rendered(name: Option<&str>) -> &str {
    name.unwrap_or("<not printable ascii>")
}

/// Prints one entry, from the listing or from the residue.
///
/// Both are printed the same way and in the same columns, because both are
/// directory entries read from the same bytes. Which vector an entry came
/// from is shown by the heading above it, not by its formatting.
///
/// `entries` and `index` locate the entry among its neighbours, which a
/// deleted short entry needs: the checksum that recovers its first byte is
/// held by the entries before it.
fn print_entry(entry: &Entry, entries: &[Entry], index: usize) {
    let position = format!("c{} s{}", entry.cluster, entry.slot);
    let (label, detail) = describe(&entry.kind, entries, index);

    println!("      {position:<9} {label:<13} {detail}");
}

/// A label and a one-line description for one classified entry.
fn describe(kind: &EntryKind, entries: &[Entry], index: usize) -> (&'static str, String) {
    match kind {
        EntryKind::VolumeLabel { name, .. } => {
            ("volume label", rendered(name.as_deref()).to_string())
        }
        EntryKind::ShortName {
            name,
            directory,
            first_cluster,
            file_size,
            ..
        } => {
            let name = rendered(name.as_deref());
            if *directory {
                ("directory", format!("{name:<13} cluster {first_cluster}"))
            } else {
                (
                    "file",
                    format!("{name:<13} {file_size} bytes, cluster {first_cluster}"),
                )
            }
        }
        EntryKind::LongName { ordinal, last, .. } => {
            let detail = if *last {
                format!("ordinal {ordinal}, last")
            } else {
                format!("ordinal {ordinal}")
            };
            ("long name", detail)
        }
        EntryKind::Deleted { was } => ("deleted", deleted_detail(was, associate(entries, index))),
        EntryKind::Invalid { attr } => ("invalid", format!("attribute {attr:#04x}")),
        // The listing never contains a terminator, but the residue can: a
        // slot whose first byte is zero and whose remaining bytes are not.
        EntryKind::Terminator => ("terminator", "first byte zero, rest not".to_string()),
    }
}

/// What can be said about a deleted entry without inventing a name.
///
/// A name appears only where the checksum of a surviving long-name component
/// determined the destroyed first byte. Where nothing determined it, that is
/// stated rather than filled in: ADR-0009 section 6.4 forbids substituting a
/// guess, and an omitted field would read as an absence of interest rather
/// than an absence of evidence.
fn deleted_detail(was: &DeletedKind, first: Option<FirstByte>) -> String {
    match was {
        DeletedKind::LongName { checksum } => {
            format!("long name component, checksum {checksum:#04x}")
        }
        DeletedKind::VolumeLabel { .. } => "volume label".to_string(),
        DeletedKind::ShortName {
            surviving_name,
            directory,
            first_cluster,
            file_size,
            ..
        } => {
            let what = if *directory {
                format!("directory, cluster {first_cluster}")
            } else {
                format!("file, {file_size} bytes, cluster {first_cluster}")
            };

            format!("{what}, {}", first_byte_detail(first, surviving_name))
        }
        DeletedKind::Invalid { attr } => format!("invalid, attribute {attr:#04x}"),
    }
}

/// How the destroyed first byte of a short name turned out.
fn first_byte_detail(first: Option<FirstByte>, surviving: &[u8; 10]) -> String {
    match first {
        Some(FirstByte::Recovered(byte)) => match recovered_name(byte, surviving) {
            Some(name) => format!("name {name} recovered"),
            None => format!("first byte {byte:#04x} recovered, name not printable ascii"),
        },
        Some(FirstByte::Destroyed) => "first byte destroyed, no long name survives".to_string(),
        Some(FirstByte::NotAssociated) => {
            "first byte destroyed, the long name before it names another entry".to_string()
        }
        // Unreachable: this arm is only reached for a deleted short entry,
        // which is exactly when `associate` answers.
        None => "first byte destroyed".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FITS: ExFatPosition = ExFatPosition {
        cluster: 5,
        slot: 0,
    };

    /// The largest cluster number, and the last slot of a 32 MiB cluster.
    const WIDEST: ExFatPosition = ExFatPosition {
        cluster: u32::MAX,
        slot: 1_048_575,
    };

    #[test]
    fn a_short_exfat_position_is_padded_to_the_column() {
        assert_eq!(exfat_position(FITS), format!("c5 s0{}", " ".repeat(15)));
    }

    #[test]
    fn the_widest_exfat_position_fills_the_column_exactly() {
        assert_eq!(exfat_position(WIDEST), "c4294967295 s1048575");
    }

    /// The widest position a volume can hold stays on its entry's line, and
    /// the columns after it begin where every other line has them.
    #[test]
    fn the_widest_exfat_position_keeps_one_line_and_the_columns() {
        let widest = format!("      {} {:<13} x", exfat_position(WIDEST), "file");
        let short = format!("      {} {:<13} x", exfat_position(FITS), "file");

        assert_eq!(widest.lines().count(), 1);
        assert_eq!(widest.find("file"), short.find("file"));
    }

    #[test]
    fn one_directory_is_singular() {
        assert_eq!(directories(1), "directory");
        assert_eq!(directories(0), "directories");
        assert_eq!(directories(2), "directories");
    }
}
