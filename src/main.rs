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

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use taphonomy::EvidenceFile;
use taphonomy::Sha256Digest;
use taphonomy::analysis::exfat::{ExFatNotRead, ExFatPhaseTwoStopped, ExFatReached};
use taphonomy::analysis::{
    Coverage, DirectoryPart, Event, MAX_DEPTH, NotReadReason, Options, Reached, RunCounts, Sink,
};
use taphonomy::confidence::Confidence;
use taphonomy::exfat_boot::{BootRegion, ExFatBootSector};
use taphonomy::exfat_claims::ConditionTwo;
use taphonomy::exfat_directory::{DeletedSetOutcome, ExFatPosition, escaped_name};
use taphonomy::fat_directory::{
    DeletedKind, Directory, Entry, EntryKind, FirstByte, associate, recovered_name,
};
use taphonomy::fat_recovery::{Extraction, Listing, Output};
use taphonomy::fat32::Fat32BootSector;
use taphonomy::filesystem::Identification;
use taphonomy::partition::MbrPartition;
use taphonomy::validation::{Outcome, Validation};

/// One line of usage, printed on any argument error.
const USAGE: &str = "usage: taphonomy <evidence-image> [--recover] \
                     [--output <directory>] [--reference-digest <hex>]";

/// The one writer of the report on standard output.
///
/// `ADR-0018` Appendix A. `println!` panics when standard output is closed
/// or full, so every line goes through here instead. The first error a write
/// returns is kept and nothing is written after it: a report with a hole in
/// the middle would read as a complete one, and `SAFETY.md` section 12
/// forbids a failure being converted into a partial success. The sink
/// reports itself stopped once this has failed, and `main` exits with 4.
///
/// Standard output keeps its own line buffering, so the order of what it
/// and standard error show on a terminal is as `println!` gave it.
struct Report {
    stdout: io::Stdout,
    failed: Option<io::Error>,
}

impl Report {
    fn new() -> Self {
        Self {
            stdout: io::stdout(),
            failed: None,
        }
    }

    /// Writes one line, unless a write has already failed.
    fn line(&mut self, text: fmt::Arguments<'_>) {
        if self.failed.is_some() {
            return;
        }
        if let Err(e) = writeln!(self.stdout, "{text}") {
            self.failed = Some(e);
        }
    }
}

/// One line of the report: `say!(out)` for an empty line, otherwise the
/// arguments of `format!` after the writer.
macro_rules! say {
    ($out:expr) => {
        $out.line(format_args!(""))
    };
    ($out:expr, $($text:tt)+) => {
        $out.line(format_args!($($text)+))
    };
}

/// One line on standard error. A failure to write it is not reported:
/// nothing is left to report it to, and the exit status still says what
/// happened (`ADR-0018` Appendix A, Decision G).
fn warn(text: fmt::Arguments<'_>) {
    let _ = writeln!(io::stderr(), "{text}");
}

macro_rules! say_err {
    ($($text:tt)+) => {
        warn(format_args!($($text)+))
    };
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);

    let Some(path) = args.next() else {
        say_err!("{USAGE}");
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
                    say_err!("error: --output given more than once");
                    say_err!("{USAGE}");
                    return ExitCode::from(2);
                }
                let Some(value) = args.next() else {
                    say_err!("error: --output requires a directory");
                    say_err!("{USAGE}");
                    return ExitCode::from(2);
                };

                output = Some(PathBuf::from(value));
            }
            Some("--reference-digest") => {
                if options.reference.is_some() {
                    say_err!("error: --reference-digest given more than once");
                    say_err!("{USAGE}");
                    return ExitCode::from(2);
                }
                let Some(value) = args.next() else {
                    say_err!("error: --reference-digest requires a digest");
                    say_err!("{USAGE}");
                    return ExitCode::from(2);
                };

                // ADR-0013 section 6.1. A digest that cannot be read stops
                // the run before any evidence is opened, which is the one
                // thing M8 fails closed on.
                match value.to_str().map(Sha256Digest::from_hex) {
                    Some(Ok(digest)) => options.reference = Some(digest),
                    Some(Err(e)) => {
                        say_err!("error: --reference-digest: {e}");
                        say_err!("{USAGE}");
                        return ExitCode::from(2);
                    }
                    None => {
                        say_err!("error: --reference-digest is not valid utf-8");
                        say_err!("{USAGE}");
                        return ExitCode::from(2);
                    }
                }
            }
            _ => {
                say_err!("error: unexpected argument");
                say_err!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }

    options.output = output.as_deref();

    // `ADR-0013` Decision H. A reference is useless without content to
    // compare, and implying --recover would enable reading a deleted file's
    // content without the operator asking for it.
    if options.reference.is_some() && !options.recover {
        say_err!("error: --reference-digest needs --recover, which reads content");
        say_err!("{USAGE}");
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
        say_err!("error: --output needs --recover, which reads content");
        say_err!("{USAGE}");
        return ExitCode::from(2);
    }

    // `ADR-0015` Decision B as Appendix A.3 corrects it. Before the
    // evidence is opened, because `SAFETY.md` section 12 lists a
    // source/destination collision among the conditions the tool fails
    // closed on.
    if let Some(directory) = options.output
        && let Err(message) = check_destination(directory, Path::new(&path))
    {
        say_err!("error: {message}");
        say_err!("{USAGE}");
        return ExitCode::from(2);
    }

    let mut out = Report::new();
    let status = match inspect(&mut out, &path, options) {
        Ok(Coverage::NothingAnalysed) => ExitCode::from(3),
        Ok(Coverage::Complete | Coverage::Incomplete) => ExitCode::SUCCESS,
        Err(e) => {
            say_err!("error: {e}");
            ExitCode::FAILURE
        }
    };

    // `ADR-0018` Appendix A, Decision G. A report that could not be written
    // is not a success whatever the coverage was, so 4 takes precedence over
    // 0, 1 and 3. `directory` is the argument as the operator gave it.
    if let Some(e) = out.failed {
        match options.output {
            Some(directory) => say_err!(
                "error: the report could not be written ({e}); the run stopped; \
                 files already written to {} may be missing from the report",
                directory.display()
            ),
            None => say_err!("error: the report could not be written ({e}); the run stopped"),
        }
        return ExitCode::from(4);
    }
    status
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
fn inspect(
    out: &mut Report,
    path: &std::ffi::OsStr,
    options: Options<'_>,
) -> Result<Coverage, taphonomy::Error> {
    let mut evidence = EvidenceFile::open(path)?;
    let reported = evidence.reported_size();
    let result = evidence.digest()?;

    say!(out, "path         {}", evidence.path().display());
    say!(out, "size         {reported} bytes");
    say!(out, "read         {} bytes", result.bytes_read);
    say!(out, "sha256       {}", result.digest);

    if result.bytes_read != reported {
        say!(out);
        say!(out, "WARNING: bytes read does not match reported size.");
        say!(out, "The digest covers {} bytes only.", result.bytes_read);
    }

    say!(out);

    let mut sink = StdoutSink::new(out);
    let counts = taphonomy::analysis::run(&mut evidence, result.bytes_read, options, &mut sink);

    print_summary(out, &counts, options);
    print_caveats(out, &counts, options);

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
struct StdoutSink<'a> {
    /// The run's one writer of standard output, which the summary after the
    /// run shares (`ADR-0018` Appendix A).
    out: &'a mut Report,

    /// Whether this directory's current-listing heading has printed.
    entries_heading_printed: bool,

    /// Whether this directory's residue heading has printed.
    residue_heading_printed: bool,

    /// Whether this exFAT volume's deleted entry sets heading has printed.
    /// A new boot region starts a new volume and resets it.
    exfat_deleted_heading_printed: bool,
}

impl<'a> StdoutSink<'a> {
    fn new(out: &'a mut Report) -> Self {
        Self {
            out,
            entries_heading_printed: false,
            residue_heading_printed: false,
            exfat_deleted_heading_printed: false,
        }
    }

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
        say!(self.out, "      {title}");
    }

    /// Prints the heading of an exFAT volume's deleted sets once, before
    /// its first line, after the volume's last directory.
    fn head_exfat_deleted(&mut self) {
        if !self.exfat_deleted_heading_printed {
            self.exfat_deleted_heading_printed = true;
            say!(self.out, "    deleted entry sets");
        }
    }
}

impl Sink for StdoutSink<'_> {
    fn stopped(&self) -> bool {
        self.out.failed.is_some()
    }

    fn record(&mut self, event: Event<'_>) {
        match event {
            Event::TableUnread(error) => say_err!("error: {error}"),
            Event::TableRejected(error) => say_err!("error: {error}"),
            Event::DiskSignature(signature) => {
                say!(self.out, "disk signature  {signature:#010x}");
            }
            Event::PartitionListed(p) => say!(
                self.out,
                "partition {}   type {:#04x}   start {}   sectors {}   bootable {}",
                p.index,
                p.partition_type,
                p.start_lba,
                p.sector_count,
                p.is_bootable()
            ),
            Event::PartitionsListed => say!(self.out),
            Event::GptDetected => say!(self.out, "GPT detected: not supported"),
            Event::Anomalies(anomalies) => {
                say!(self.out);
                say!(self.out, "anomalies");
                for a in anomalies {
                    say!(self.out, "  {a}");
                }
            }
            Event::PartitionUnread { partition, error } => {
                say_err!("error: partition {}: {error}", partition.index);
            }
            Event::PartitionIdentified {
                partition,
                identification,
                type_match,
            } => print_partition_identified(self.out, partition, identification, type_match),
            Event::ExFatIdentifiedFromBackup {
                partition,
                sector_zero,
            } => {
                say!(
                    self.out,
                    "partition {}   filesystem exFAT, identified from its backup boot region",
                    partition.index
                );
                say!(self.out, "    sector 0             {sector_zero}");
            }
            Event::BootSectorRejected(error) => {
                say!(self.out, "    BOOT SECTOR REJECTED: {error}");
            }
            Event::BootSector(boot) => print_boot_sector(self.out, boot),
            Event::RootDirectoryUnread(error) => {
                say!(self.out, "    ROOT DIRECTORY NOT ENUMERATED: {error}");
            }
            Event::Directory { reached, directory } => {
                self.entries_heading_printed = false;
                self.residue_heading_printed = false;
                print_directory(self.out, &heading_of(reached), directory);
            }
            Event::DirectoryNotRead { reached, reason } => {
                say!(self.out, "    {}", heading_of(reached));
                match reason {
                    NotReadReason::TooDeep => {
                        say!(
                            self.out,
                            "      NOT READ: deeper than {MAX_DEPTH} levels below the root"
                        );
                    }
                    NotReadReason::AlreadyRead { cluster } => say!(
                        self.out,
                        "      NOT READ: cluster {cluster} was already read as a directory"
                    ),
                    NotReadReason::Refused(reason) => {
                        say!(self.out, "      NOT READ: {reason}");
                    }
                    NotReadReason::Enumeration(error) => {
                        say!(self.out, "      NOT READ: {error}");
                    }
                }
            }
            Event::ListingMayContinue(cluster) => {
                say!(
                    self.out,
                    "      listing may continue: cluster {cluster} holds no terminator"
                );
            }
            Event::OrphanSearchStopped { cluster, error } => {
                say!(
                    self.out,
                    "    ORPHAN SEARCH STOPPED at cluster {cluster}: {error}"
                );
            }
            Event::OrphanedListingUnread {
                holder,
                name,
                first_cluster,
            } => {
                say!(
                    self.out,
                    "    directory {name}, listed in orphaned directory c{holder}"
                );
                say!(
                    self.out,
                    "      NOT READ: cluster {first_cluster} was not found by the search"
                );
            }
            Event::NotAssessed {
                listing,
                part,
                entry,
                error,
            } => {
                self.head_content(listing, part);
                say!(
                    self.out,
                    "        {:<9} NOT ASSESSED: {error}",
                    entry_position(entry)
                );
            }
            Event::Ineligible {
                listing,
                part,
                entry,
                reason,
            } => {
                self.head_content(listing, part);
                say!(
                    self.out,
                    "        {:<9} no content: {reason}",
                    entry_position(entry)
                );
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
                say!(self.out, "        {:<9} {detail}", entry_position(entry));
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
                say!(self.out, "        {:<9} {detail}", entry_position(entry));
            }
            Event::Extracted {
                listing: _,
                part: _,
                entry: _,
                extraction,
                validation,
                readback_matches,
            } => print_extraction(self.out, extraction, validation, readback_matches),
            Event::NotExtracted {
                listing: _,
                part: _,
                entry: _,
                error,
            } => print_not_extracted(self.out, error),
            Event::ExFatBootRegion(boot) => {
                self.exfat_deleted_heading_printed = false;
                print_exfat_boot_region(self.out, boot);
            }
            Event::ExFatBootRejected(failure) => {
                say!(self.out, "    BOOT REGION REJECTED: {failure}");
            }
            Event::ExFatTexFat(region) => {
                say!(
                    self.out,
                    "    TexFAT: two FATs in the {} boot region, not supported",
                    exfat_region_name(region)
                );
            }
            Event::ExFatRootDirectoryUnread(error) => {
                say!(self.out, "    ROOT DIRECTORY NOT ENUMERATED: {error}");
            }
            Event::ExFatDirectoryStart { reached } => {
                say!(self.out, "    {}", exfat_heading(reached));
            }
            Event::ExFatEntry { position, file } => {
                let name = escaped_name(&file.name);
                let place = exfat_position(position);
                if file.is_directory() {
                    say!(
                        self.out,
                        "      {place} {:<13} {name:<13} cluster {}",
                        "directory",
                        file.first_cluster
                    );
                } else {
                    let valid = if file.valid_data_length == file.data_length {
                        String::new()
                    } else {
                        format!(", {} valid", file.valid_data_length)
                    };
                    say!(
                        self.out,
                        "      {place} {:<13} {name:<13} {} bytes{valid}, cluster {}",
                        "file",
                        file.data_length,
                        file.first_cluster
                    );
                }
            }
            Event::ExFatVolumeLabel { position, label } => say!(
                self.out,
                "      {} {:<13} {}",
                exfat_position(position),
                "volume label",
                escaped_name(label)
            ),
            Event::ExFatBitmap { position, bitmap } => say!(
                self.out,
                "      {} {:<13} cluster {}, {} bytes",
                exfat_position(position),
                "bitmap",
                bitmap.first_cluster,
                bitmap.data_length
            ),
            Event::ExFatSetRefused { position, refusal } => say!(
                self.out,
                "      {} {:<13} {refusal}",
                exfat_position(position),
                "not used"
            ),
            Event::ExFatDirectoryEnd { summary, .. } => {
                say!(
                    self.out,
                    "      entries            {}, {} files, {} directories",
                    summary.entries_read,
                    summary.files,
                    summary.directories
                );
                if summary.unused_file_sets > 0 {
                    say!(
                        self.out,
                        "      deleted entry sets {}",
                        summary.unused_file_sets
                    );
                }
                if summary.entries_after_end_marker > 0 {
                    say!(
                        self.out,
                        "      entries after end marker {}, not read",
                        summary.entries_after_end_marker
                    );
                }
            }
            Event::ExFatDirectoryNotRead { reached, reason } => {
                say!(self.out, "    {}", exfat_heading(reached));
                match reason {
                    ExFatNotRead::TooDeep => {
                        say!(
                            self.out,
                            "      NOT READ: deeper than {MAX_DEPTH} levels below the root"
                        );
                    }
                    ExFatNotRead::Error(error) => {
                        say!(self.out, "      NOT READ: {error}");
                    }
                }
            }
            Event::ExFatDirectoryCapReached {
                cap,
                directories_unread,
            } => say!(
                self.out,
                "    {directories_unread} {} not read: the volume has more than {cap} directory clusters",
                directories(directories_unread)
            ),
            Event::ExFatQueueBoundReached {
                bound,
                directories_unread,
            } => say!(
                self.out,
                "    {directories_unread} {} not read: more than {bound} were waiting to be read",
                directories(directories_unread)
            ),
            // Their gaps are counted, and printed in the summary.
            Event::ExFatDeletedDirectoriesRead(_) | Event::ExFatClaimsAssessed(_) => {}
            Event::ExFatDeletedSetDecided { position, outcome } => {
                self.head_exfat_deleted();
                say!(self.out, "{}", exfat_decision_line(position, outcome));
            }
            Event::ExFatExtracted {
                position: _,
                file: _,
                extraction,
                validation,
                readback_matches,
            } => print_extraction(self.out, extraction, validation, readback_matches),
            Event::ExFatNotExtracted {
                position: _,
                file: _,
                error,
            } => print_not_extracted(self.out, error),
            Event::ExFatDeletedSetsNotAssessed { reason, sets } => {
                self.head_exfat_deleted();
                say!(
                    self.out,
                    "      NOT ASSESSED: {sets} deleted sets: {}",
                    phase_two_stopped(reason)
                );
            }
        }
    }
}

/// One decided deleted exFAT set: its position, its name, and the outcome
/// (ADR-0022 section 6 and Appendix E). The name is in the column a live
/// entry's kind takes; a set unusable on re-read has none to show.
fn exfat_decision_line(position: ExFatPosition, outcome: &DeletedSetOutcome) -> String {
    use DeletedSetOutcome as O;
    let (name, text) = match outcome {
        O::UnusableOnReread { refusal } => (String::new(), format!("NOT ASSESSED: {refusal}")),
        O::DeletedDirectory { file } => (
            escaped_name(&file.name),
            "deleted directory, not decided".to_string(),
        ),
        O::NoData { file } => (
            escaped_name(&file.name),
            "no content: no cluster and no data".to_string(),
        ),
        O::LeftByBound { file } => (
            escaped_name(&file.name),
            "NOT ASSESSED: phase 2's work bound reached".to_string(),
        ),
        O::ConditionFour { file, reason } => (
            escaped_name(&file.name),
            format!("REFUSED, condition 4: {reason}"),
        ),
        O::ClusterWithoutData { file } => (
            escaped_name(&file.name),
            format!(
                "REFUSED, condition 4: cluster {} with no data",
                file.first_cluster
            ),
        ),
        O::ConditionOne { file, cluster } => (
            escaped_name(&file.name),
            format!("REFUSED, condition 1: cluster {cluster} is in use"),
        ),
        O::ConditionTwo { file, verdict } => (
            escaped_name(&file.name),
            format!(
                "REFUSED, condition 2: {}",
                match verdict {
                    ConditionTwo::Later => "a later claim",
                    ConditionTwo::Unordered => "claims not ordered",
                    // Never built: a set that holds is not refused. Should
                    // it reach here, the line must not read as a finding.
                    ConditionTwo::Holds => "internal error, the verdict holds",
                }
            ),
        ),
        O::NotAssessed { file } => (
            escaped_name(&file.name),
            "NOT ASSESSED: claims bound reached".to_string(),
        ),
        O::NoValidData { file } => (
            escaped_name(&file.name),
            "REFUSED, condition 3: no valid data".to_string(),
        ),
        O::ConditionThreeNotChecked { file } => (
            escaped_name(&file.name),
            format!(
                "cluster {}, {} bytes, conditions 4, 1 and 2 passed, condition 3 not checked",
                file.first_cluster, file.data_length
            ),
        ),
        O::ZerosOnly { file } => (
            escaped_name(&file.name),
            "REFUSED, condition 3: zeros only".to_string(),
        ),
        // No level word: the level is the summary's (`ADR-0014` Appendix
        // A.10). The extraction lines follow, as for FAT32.
        O::Recoverable { file } => {
            let valid = if file.valid_data_length < file.data_length {
                format!(
                    ", {} bytes valid, the rest written as zeros",
                    file.valid_data_length
                )
            } else {
                String::new()
            };
            (
                escaped_name(&file.name),
                format!(
                    "cluster {}, {} bytes, conditions 4, 1, 2 and 3 passed{valid}",
                    file.first_cluster, file.data_length
                ),
            )
        }
    };
    format!("      {} {name:<13} {text}", exfat_position(position))
}

/// Why an exFAT volume's deleted sets were left not assessed.
fn phase_two_stopped(reason: ExFatPhaseTwoStopped<'_>) -> String {
    match reason {
        ExFatPhaseTwoStopped::NoBitmap => "the root holds no allocation bitmap".to_string(),
        ExFatPhaseTwoStopped::DirectoriesNotReadWhole => {
            "a directory of the volume was not read whole".to_string()
        }
        ExFatPhaseTwoStopped::BitmapNotOpened(error) => {
            format!("allocation bitmap not followed: {error}")
        }
        ExFatPhaseTwoStopped::BitmapNotBelieved(error) => {
            format!("allocation bitmap not believed: {error}")
        }
        ExFatPhaseTwoStopped::DeletedDirectories(error) => {
            format!("deleted directories not read: {error}")
        }
        ExFatPhaseTwoStopped::Claims(error) => format!("claims not followed: {error}"),
        ExFatPhaseTwoStopped::Decide(error) => format!("deciding stopped: {error}"),
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
fn print_exfat_boot_region(out: &mut Report, boot: &ExFatBootSector) {
    let found = if boot.located_by_search {
        ", found by trying each sector size"
    } else {
        ""
    };
    say!(
        out,
        "    boot region          {}{found}",
        exfat_region_name(boot.region)
    );
    say!(out, "    bytes per sector     {}", boot.bytes_per_sector());
    say!(
        out,
        "    sectors per cluster  {}",
        1u32 << boot.sectors_per_cluster_shift
    );
    say!(out, "    cluster count        {}", boot.cluster_count);
    say!(out, "    first data sector    {}", boot.cluster_heap_offset);
    say!(out, "    root cluster         {}", boot.root_cluster);
    say!(out, "    volume id            {:#010x}", boot.volume_serial);

    if !boot.observations.is_empty() {
        say!(out, "    boot region observations");
        for o in &boot.observations {
            say!(out, "      {o}");
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
    out: &mut Report,
    partition: &MbrPartition,
    identification: &Identification,
    type_match: Option<bool>,
) {
    match identification {
        Identification::Identified { filesystem, .. } => {
            say!(
                out,
                "partition {}   filesystem {filesystem}",
                partition.index
            );
        }
        Identification::Unknown { reason } => {
            say!(
                out,
                "partition {}   filesystem unknown ({reason})",
                partition.index
            );
        }
    }

    if let Identification::Identified {
        geometry: Some(g), ..
    } = identification
    {
        say!(out, "    bytes per sector     {}", g.bytes_per_sector);
        say!(out, "    sectors per cluster  {}", g.sectors_per_cluster);
        say!(out, "    cluster count        {}", g.cluster_count);
        say!(out, "    first data sector    {}", g.first_data_sector());
    }

    if type_match == Some(false)
        && let Some(filesystem) = identification.filesystem()
    {
        say!(
            out,
            "    MISMATCH: declared type {:#04x} disagrees with observed filesystem {filesystem}",
            partition.partition_type
        );
    }

    if let Identification::Identified { observations, .. } = identification
        && !observations.is_empty()
    {
        say!(out, "    observations");
        for o in observations {
            say!(out, "      {o}");
        }
    }
}

/// Prints a FAT32 volume's validated boot sector.
fn print_boot_sector(out: &mut Report, boot: &Fat32BootSector) {
    say!(out, "    root cluster         {}", boot.root_cluster);
    say!(out, "    backup boot sector   {}", boot.backup_boot_sector);

    match boot.active_fat() {
        Some(n) => say!(out, "    FAT mirroring        disabled, FAT {n} active"),
        None => say!(out, "    FAT mirroring        enabled"),
    }

    if let Some(id) = boot.volume_id {
        say!(out, "    volume id            {id:#010x}");
    }

    if let Some(label) = &boot.volume_label {
        say!(out, "    volume label         {label}");
    }

    if !boot.observations.is_empty() {
        say!(out, "    boot sector observations");
        for o in &boot.observations {
            say!(out, "      {o}");
        }
    }
}

/// Prints a directory's statistics, entries, residue and observations.
///
/// The root's block is printed exactly as it was before subdirectories were
/// read, so every heading below the root reuses it unchanged.
fn print_directory(out: &mut Report, heading: &str, directory: &Directory) {
    let chain: Vec<String> = directory.clusters.iter().map(|c| c.to_string()).collect();

    say!(out, "    {heading}");
    say!(out, "      cluster chain      {}", chain.join(" -> "));
    say!(out, "      entries            {}", directory.entries.len());
    say!(
        out,
        "      short entries      {}",
        directory.short_entry_count()
    );
    say!(
        out,
        "      long name entries  {}",
        directory.long_name_count()
    );
    say!(
        out,
        "      deleted entries    {}",
        directory.deleted_count()
    );

    for (index, entry) in directory.entries.iter().enumerate() {
        print_entry(out, entry, &directory.entries, index);
    }

    if !directory.residue.is_empty() {
        say!(out, "      past the terminator");
        for (index, entry) in directory.residue.iter().enumerate() {
            print_entry(out, entry, &directory.residue, index);
        }
    }

    if !directory.observations.is_empty() {
        say!(out, "      directory observations");
        for o in &directory.observations {
            say!(out, "        {o}");
        }
    }
}

/// Prints an extraction: its digest, what became of its file, and its
/// comparison with the reference. The same lines for a FAT32 run and an
/// exFAT set, with no level word on them (`ADR-0014` Appendix A.10).
fn print_extraction(
    out: &mut Report,
    extraction: &Extraction,
    validation: Validation,
    readback_matches: Option<bool>,
) {
    // `docs/SAFETY.md` section 10 requires the recovered-file hash and the
    // validation hash to be distinguishable where both are reported.
    say!(
        out,
        "        {:<9} recovered sha256 {} over {} bytes",
        "",
        extraction.digest,
        extraction.bytes_hashed
    );

    if let Some(output) = &extraction.output {
        print_output(out, output, readback_matches);
    }

    match (validation.outcome, validation.reference) {
        (Outcome::Match, Some(reference)) => {
            say!(
                out,
                "        {:<9} reference sha256 {reference} MATCHES",
                ""
            );
        }
        (Outcome::Differs, Some(reference)) => {
            say!(
                out,
                "        {:<9} reference sha256 {reference} DIFFERS",
                ""
            );
        }
        // `NotAttempted` carries no reference, and a reference is always
        // carried where a comparison happened.
        _ => {}
    }
}

/// Prints an extraction that failed, for either filesystem.
fn print_not_extracted(out: &mut Report, error: &dyn std::fmt::Display) {
    say!(out, "        {:<9} NOT EXTRACTED: {error}", "");
}

/// Says what became of a written artifact, where one was asked for.
///
/// `ADR-0015` Decision H. The comparison is made in the library rather than
/// here, because a difference between what was read and what landed is a
/// finding to report and not an error to raise: `ADR-0013` section 3 treats
/// a differing reference the same way, and the exit status is unchanged by
/// either. `readback_matches` carries that comparison's answer; it is
/// `Some` exactly where `output` is [`Output::Written`].
fn print_output(out: &mut Report, output: &Output, readback_matches: Option<bool>) {
    match output {
        Output::Written { path, .. } => {
            let verdict = if readback_matches == Some(true) {
                "matches what was read"
            } else {
                "DIFFERS from what was read"
            };

            say!(
                out,
                "        {:<9} written {} {verdict}",
                "",
                path.display()
            );
        }
        Output::Unverified { path, message } => {
            say!(
                out,
                "        {:<9} written {} NOT VERIFIED: {message}",
                "",
                path.display()
            );
        }
        Output::Exists { path } => {
            say!(
                out,
                "        {:<9} NOT WRITTEN: {} exists",
                "",
                path.display()
            );
        }
        Output::NotCreated { path, message } => {
            say!(
                out,
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

            say!(
                out,
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
/// forbids a single combined success figure. Two levels are reachable,
/// `RECONSTRUCTED` for FAT32 (`ADR-0014` Decision A) and `UNVERIFIED` for
/// exFAT (`ADR-0022` Decision A), so the levels share one line here and the
/// counts on it are what vary: Appendix A.10 keeps the word off the
/// per-entry lines, and Appendix B.8 keeps the counts on this one.
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
fn print_summary(out: &mut Report, counts: &RunCounts, options: Options<'_>) {
    say!(out);
    say!(out, "coverage     {}", counts.coverage());

    print_count(out, "volumes analysed", counts.volumes_analysed());
    print_gap(out, "partition table not read", counts.table_unread());
    print_gap(out, "partition table not parsed", counts.table_rejected());
    print_gap(
        out,
        "partition entries rejected",
        counts.partitions_anomalous(),
    );
    print_gap(out, "GPT not analysed", counts.gpt());
    print_gap(out, "partitions not read", counts.partition_unread());
    print_gap(out, "filesystems not identified", counts.unidentified());
    print_gap(out, "filesystems not analysed", counts.unsupported());
    print_gap(out, "boot sectors rejected", counts.boot_rejected());
    print_gap(out, "root directories not read", counts.root_unread());
    print_gap(
        out,
        "entries not assessed",
        counts.not_assessed() + counts.exfat_not_assessed(),
    );
    print_gap(
        out,
        "entries not extracted",
        counts.not_extracted() + counts.exfat_not_extracted(),
    );
    print_gap(
        out,
        "artifacts not written",
        counts.not_delivered() + counts.exfat_not_delivered(),
    );
    print_gap(out, "directories not read", counts.directories_unread());
    print_gap(
        out,
        "listings that may continue",
        counts.listings_may_continue(),
    );
    print_gap(
        out,
        "orphan searches stopped",
        counts.orphan_search_stopped(),
    );
    print_gap(out, "entry sets not used", counts.exfat_sets_refused());
    print_gap(
        out,
        "entries after end marker",
        counts.exfat_end_marker_residue(),
    );
    print_gap(out, "deleted sets not usable", counts.exfat_sets_unusable());
    print_gap(
        out,
        "claimants not followed",
        counts.exfat_claimants_not_followed(),
    );
    print_gap(
        out,
        "deleted directories unread",
        counts.exfat_deleted_directories_unread(),
    );
    // ADR-0022 Decision K's words for a set inside a deleted directory.
    print_gap(
        out,
        "deleted sets not examined",
        counts.exfat_sets_inside_not_examined(),
    );

    // Only `--recover` produces an artifact, so without it the line would
    // report a zero that the invocation already determined.
    // `ADR-0022` section 3: each level with a count above zero; a run with
    // no exFAT artifact prints the line it printed before.
    if options.recover {
        say!(out, "artifacts    {}", artifacts_by_level(counts, options));
    }

    // Only where a reference was supplied. Where none was, the caveat block
    // below already states that nothing was validated, and saying it twice
    // would present one fact about the invocation as two findings. Summed
    // over both filesystems (`ADR-0022` section 11).
    if options.reference.is_some() {
        let matched = counts.matched() + counts.exfat_matched();
        let differed = counts.differed(options) + counts.exfat_differed(options);
        say!(out, "compared     {matched} matched, {differed} differed");
    }

    // Deleted entries that produced no artifact, by what stopped them.
    // `ADR-0013` Appendix C.4: an operator asking whether a file is present
    // needs to know the question went unanswered for part of the volume.
    // `ADR-0022` section 11: each part sums FAT32's and exFAT's. An exFAT
    // set that passed is not read whatever `--recover` says, as content is
    // not yet read for exFAT.
    let mut unrecovered = Vec::new();
    push_part(
        &mut unrecovered,
        counts.refused() + counts.exfat_refused(),
        "refused",
    );
    push_part(
        &mut unrecovered,
        counts.not_read(options) + counts.exfat_passed(),
        "not read",
    );
    push_part(
        &mut unrecovered,
        counts.not_assessed() + counts.exfat_not_assessed(),
        "not assessed",
    );
    push_part(
        &mut unrecovered,
        counts.not_extracted() + counts.exfat_not_extracted(),
        "not extracted",
    );
    push_part(
        &mut unrecovered,
        counts.empty() + counts.exfat_no_data(),
        "size zero",
    );
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
        say!(out, "unrecovered  {}", unrecovered.join(", "));
    }
}

/// The artifacts produced, by level: `0`, or each level with a count above
/// zero, FAT32's `RECONSTRUCTED` first (`ADR-0022` section 3, `ADR-0003`
/// section 4.7).
fn artifacts_by_level(counts: &RunCounts, options: Options<'_>) -> String {
    let levels = [
        (counts.artifacts(options), Confidence::of_inferred_run()),
        (counts.exfat_artifacts(), Confidence::of_metadata_extent()),
    ];
    let parts: Vec<String> = levels
        .iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, level)| format!("{count} {level}"))
        .collect();
    if parts.is_empty() {
        "0".to_string()
    } else {
        parts.join(", ")
    }
}

/// One line of the coverage block.
fn print_count(out: &mut Report, label: &str, count: usize) {
    say!(out, "  {label:<width$} {count}", width = GAP_LABEL_WIDTH);
}

/// One line of the coverage block, printed only where there is something to
/// report. A zero would state that a kind of gap did not occur, which reads
/// as a finding about the evidence rather than about the run.
fn print_gap(out: &mut Report, label: &str, count: usize) {
    if count > 0 {
        print_count(out, label, count);
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
fn print_caveats(out: &mut Report, counts: &RunCounts, options: Options<'_>) {
    let fat32 = counts.free_runs() > 0;
    let exfat = counts.exfat_recoverable() + counts.exfat_passed() > 0;
    if !fat32 && !exfat {
        return;
    }

    if fat32 {
        // ADR-0003 section 4.2 and ADR-0010 Decision C. Saying only that the
        // run is free would let a reader take it for a finding. It is the
        // absence of contrary evidence, which is not the same thing and never
        // becomes it.
        say!(out);
        say!(out, "A free run means nothing has claimed those");
        say!(out, "clusters since the entry lost them. It is not");
        say!(out, "evidence that the content there is this file's.");
    }

    // ADR-0013 section 8.2. A match is byte equality with what the operator
    // supplied, and where the content is not distinctive that is weaker
    // evidence than it reads as. Nothing in it is FAT32's, so a match on
    // either filesystem prints it.
    if counts.matched() + counts.exfat_matched() > 0 {
        say!(out);
        say!(out, "A match establishes that these bytes are the");
        say!(out, "reference's, byte for byte. Where content is not");
        say!(out, "distinctive, other clusters could hold the same");
        say!(out, "bytes.");
    }

    // ADR-0013 Decision E. The causes are listed and none is chosen, because
    // the evidence does not distinguish between them. They are a FAT32 run's
    // causes, so only FAT32's differences print it.
    if fat32 && counts.differed(options) > 0 {
        say!(out);
        say!(out, "A differing digest does not say which of these");
        say!(out, "happened: the file was fragmented, clusters of");
        say!(out, "the run were reused, the recorded size is wrong,");
        say!(out, "or the reference is another file.");
    }

    // ADR-0019 section 5 and ADR-0022 section 15: a set states the checks it
    // passed and never that the data is the file's. True with content read
    // or not.
    if exfat {
        say!(out);
        for line in EXFAT_CAVEAT {
            say!(out, "{line}");
        }
    }

    // Attached to the paragraph above rather than set apart: each is one
    // line about the invocation, not a caveat of its own.
    if !options.recover {
        say!(out, "No content read. Pass --recover to read it.");
    } else if options.reference.is_none() {
        say!(out, "No reference supplied. Nothing was validated.");
    }
}

/// The caveat printed where any exFAT set was recovered or passed, at most
/// 50 columns a line, as the FAT32 paragraphs are. True whether or not
/// content was read: the per-set line names which conditions passed.
const EXFAT_CAVEAT: [&str; 5] = [
    "A deleted exFAT file is reported with the",
    "checks it passed. They show its metadata intact",
    "and its clusters unclaimed, not that the data is",
    "this file's: data can be stale, or never",
    "written, while all of its metadata is intact.",
];

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
fn print_entry(out: &mut Report, entry: &Entry, entries: &[Entry], index: usize) {
    let position = format!("c{} s{}", entry.cluster, entry.slot);
    let (label, detail) = describe(&entry.kind, entries, index);

    say!(out, "      {position:<9} {label:<13} {detail}");
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

    /// Every decided set's line: the position and name columns, then the
    /// outcome in the words the operator reads (ADR-0022 Appendix E).
    #[test]
    fn each_decided_exfat_set_prints_its_outcome_in_its_own_words() {
        use DeletedSetOutcome as O;
        use taphonomy::exfat_directory::{ExFatContentError, ExFatFile, SetRefusal};

        let file = || ExFatFile {
            name: "f.bin".encode_utf16().collect(),
            attributes: 0x20,
            first_cluster: 30,
            data_length: 1024,
            valid_data_length: 1024,
            no_fat_chain: false,
        };
        let reason = ExFatContentError::ChainEndsEarly {
            cluster: 30,
            needed: 2,
        };
        let refusal = SetRefusal::ChangedSinceWalk;
        let cases = [
            (
                O::UnusableOnReread {
                    refusal: refusal.clone(),
                },
                format!("{:<13} NOT ASSESSED: {refusal}", ""),
            ),
            (
                O::DeletedDirectory { file: file() },
                "f.bin         deleted directory, not decided".to_string(),
            ),
            (
                O::NoData { file: file() },
                "f.bin         no content: no cluster and no data".to_string(),
            ),
            (
                O::LeftByBound { file: file() },
                "f.bin         NOT ASSESSED: phase 2's work bound reached".to_string(),
            ),
            (
                O::ConditionFour {
                    file: file(),
                    reason: ExFatContentError::ChainEndsEarly {
                        cluster: 30,
                        needed: 2,
                    },
                },
                format!("f.bin         REFUSED, condition 4: {reason}"),
            ),
            (
                O::ClusterWithoutData { file: file() },
                "f.bin         REFUSED, condition 4: cluster 30 with no data".to_string(),
            ),
            (
                O::ConditionOne {
                    file: file(),
                    cluster: 31,
                },
                "f.bin         REFUSED, condition 1: cluster 31 is in use".to_string(),
            ),
            (
                O::ConditionTwo {
                    file: file(),
                    verdict: ConditionTwo::Later,
                },
                "f.bin         REFUSED, condition 2: a later claim".to_string(),
            ),
            (
                O::ConditionTwo {
                    file: file(),
                    verdict: ConditionTwo::Unordered,
                },
                "f.bin         REFUSED, condition 2: claims not ordered".to_string(),
            ),
            (
                O::ConditionTwo {
                    file: file(),
                    verdict: ConditionTwo::Holds,
                },
                "f.bin         REFUSED, condition 2: internal error, the verdict holds".to_string(),
            ),
            (
                O::NotAssessed { file: file() },
                "f.bin         NOT ASSESSED: claims bound reached".to_string(),
            ),
            (
                O::NoValidData { file: file() },
                "f.bin         REFUSED, condition 3: no valid data".to_string(),
            ),
            (
                O::ConditionThreeNotChecked { file: file() },
                "f.bin         cluster 30, 1024 bytes, conditions 4, 1 and 2 passed, \
                 condition 3 not checked"
                    .to_string(),
            ),
        ];
        for (outcome, text) in cases {
            assert_eq!(
                exfat_decision_line(FITS, &outcome),
                format!("      c5 s0{} {text}", " ".repeat(15))
            );
        }
    }

    /// A recovered set's line states the four checks it passed, and where
    /// ValidDataLength is short, that the rest was written as zeros; no
    /// level word (`ADR-0014` Appendix A.10).
    #[test]
    fn a_recoverable_exfat_set_names_the_checks_it_passed() {
        use taphonomy::exfat_directory::ExFatFile;

        let file = |valid_data_length| ExFatFile {
            name: "f.bin".encode_utf16().collect(),
            attributes: 0x20,
            first_cluster: 30,
            data_length: 1024,
            valid_data_length,
            no_fat_chain: false,
        };
        let prefix = format!("      c5 s0{} f.bin         ", " ".repeat(15));
        assert_eq!(
            exfat_decision_line(FITS, &DeletedSetOutcome::Recoverable { file: file(1024) }),
            format!("{prefix}cluster 30, 1024 bytes, conditions 4, 1, 2 and 3 passed")
        );
        assert_eq!(
            exfat_decision_line(FITS, &DeletedSetOutcome::Recoverable { file: file(600) }),
            format!(
                "{prefix}cluster 30, 1024 bytes, conditions 4, 1, 2 and 3 passed, \
                 600 bytes valid, the rest written as zeros"
            )
        );
    }

    /// The exFAT caveat keeps to the width the FAT32 paragraphs keep to,
    /// and names each check and why it is not evidence of the file.
    #[test]
    fn the_exfat_caveat_fits_fifty_columns_and_states_its_limit() {
        for line in EXFAT_CAVEAT {
            assert!(line.chars().count() <= 50, "{line}");
        }
        // The planning session's text (M15-6b2 review note 1), true whether
        // or not content was read.
        assert_eq!(
            EXFAT_CAVEAT,
            [
                "A deleted exFAT file is reported with the",
                "checks it passed. They show its metadata intact",
                "and its clusters unclaimed, not that the data is",
                "this file's: data can be stale, or never",
                "written, while all of its metadata is intact.",
            ]
        );
    }

    #[test]
    fn phase_2_left_without_a_bitmap_or_a_directory_read_whole_says_so() {
        assert_eq!(
            phase_two_stopped(ExFatPhaseTwoStopped::NoBitmap),
            "the root holds no allocation bitmap"
        );
        assert_eq!(
            phase_two_stopped(ExFatPhaseTwoStopped::DirectoriesNotReadWhole),
            "a directory of the volume was not read whole"
        );
    }
}
