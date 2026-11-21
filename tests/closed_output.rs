//! A report that cannot be written stops the run (`ADR-0018` Appendix A).
//!
//! The first three tests run the binary with standard output closed or
//! full. The rest call the library with a sink that reports itself stopped
//! at a chosen event and keeps every event it is handed afterwards, so each
//! check the run makes of `Sink::stopped` is shown to matter: a sink
//! stopped by the first event of a run is stopped by the first loop that
//! asks, and would hide every check after it.
//!
//! The fixtures are generated and not committed. Run
//! `./scripts/generate-fixtures.sh` first.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use taphonomy::EvidenceFile;
use taphonomy::analysis::exfat::ExFatReached;
use taphonomy::analysis::{self, Event, Options, Reached, Sink};
use taphonomy::exfat_directory::DeletedSetOutcome;

/// The line the binary writes to standard error when its report fails, up to
/// the error it names.
const FAILURE_PREFIX: &str = "error: the report could not be written (";

fn fixture(name: &str) -> PathBuf {
    let path = Path::new("fixtures/partition").join(name);
    assert!(
        path.exists(),
        "fixture missing: {}\nrun ./scripts/generate-fixtures.sh first",
        path.display()
    );
    path
}

/// A scratch directory of this test's own, emptied: `closed-output-{name}`
/// inside the folder Cargo sets aside for integration tests, as
/// `tests/exfat_analysis.rs` does. `name` must be distinct for each test,
/// because tests of one binary run in parallel.
fn empty_scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("closed-output-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

/// Every file below `dir`, however deep.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).expect("reading the scratch directory") {
        let path = entry.expect("reading an entry").path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Runs the binary with standard output given, and standard error captured.
fn run_with_stdout(args: &[&std::ffi::OsStr], stdout: Stdio) -> Output {
    Command::new(env!("CARGO_BIN_EXE_taphonomy"))
        .args(args)
        .stdout(stdout)
        .output()
        .expect("running the binary")
}

/// A standard output whose reading end is already closed: every write to it
/// fails with a broken pipe.
fn closed_pipe() -> Stdio {
    let (reader, writer) = std::io::pipe().expect("making a pipe");
    drop(reader);
    Stdio::from(writer)
}

#[test]
fn a_closed_standard_output_stops_the_run_with_status_4() {
    let image = fixture("fat32-recover-run.img");
    let output = run_with_stdout(&[image.as_os_str()], closed_pipe());
    let err = stderr(&output);

    assert_eq!(output.status.code(), Some(4), "{err}");
    assert!(err.starts_with(FAILURE_PREFIX), "{err}");
    assert!(err.contains("); the run stopped"), "{err}");
    assert!(!err.contains("files already written"), "{err}");
    assert!(!err.contains("panicked"), "{err}");
}

#[test]
fn a_closed_standard_output_writes_no_recovered_file() {
    let image = fixture("fat32-recover-run.img");
    let dir = empty_scratch("closed-pipe");
    let output = run_with_stdout(
        &[
            image.as_os_str(),
            "--recover".as_ref(),
            "--output".as_ref(),
            dir.as_os_str(),
        ],
        closed_pipe(),
    );
    let err = stderr(&output);

    assert_eq!(output.status.code(), Some(4), "{err}");
    assert!(err.starts_with(FAILURE_PREFIX), "{err}");
    assert!(
        err.contains(&format!(
            "; files already written to {} may be missing from the report",
            dir.display()
        )),
        "{err}"
    );
    assert!(!err.contains("panicked"), "{err}");
    assert!(
        fs::read_dir(&dir).expect("reading").next().is_none(),
        "the output directory is not empty: {:?}",
        files_under(&dir)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_full_standard_output_stops_the_run_with_status_4() {
    let image = fixture("fat32-recover-run.img");
    let full = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .expect("opening /dev/full");
    let output = run_with_stdout(&[image.as_os_str()], Stdio::from(full));
    let err = stderr(&output);

    assert_eq!(output.status.code(), Some(4), "{err}");
    assert!(err.starts_with(FAILURE_PREFIX), "{err}");
    assert!(err.contains("No space left on device"), "{err}");
    assert!(!err.contains("panicked"), "{err}");
}

/// What a probe sink makes of one event: a name for its kind, so a test can
/// say which kinds of event came after the sink stopped. Events borrow from
/// the run and cannot be kept, so only this is.
fn kind(event: &Event<'_>) -> &'static str {
    match event {
        Event::DiskSignature(_) => "disk signature",
        Event::PartitionListed(_) => "partition listed",
        Event::PartitionsListed => "partitions listed",
        Event::PartitionIdentified { .. } => "partition identified",
        Event::BootSector(_) => "boot sector",
        Event::Directory {
            reached: Reached::Root,
            ..
        } => "directory root",
        Event::Directory {
            reached: Reached::Walked { .. },
            ..
        } => "directory walked",
        Event::Directory {
            reached: Reached::Orphaned { .. },
            ..
        } => "directory orphaned",
        Event::Recoverable { .. } => "recoverable",
        Event::Extracted { .. } => "extracted",
        Event::ExFatDirectoryStart { .. } => "exfat directory start",
        Event::ExFatDirectoryEnd { .. } => "exfat directory end",
        Event::ExFatDeletedSetDecided {
            outcome: DeletedSetOutcome::Recoverable { .. },
            ..
        } => "exfat decided recoverable",
        Event::ExFatDeletedSetDecided { .. } => "exfat decided",
        Event::ExFatExtracted { .. } => "exfat extracted",
        Event::ExFatDeletedDirectoriesRead(_)
        | Event::ExFatClaimsAssessed(_)
        | Event::ExFatDeletedSetsNotAssessed { .. } => "exfat phase two",
        _ => "other",
    }
}

/// A sink that stops at an event a test chooses, then keeps what it is
/// handed, so a test can see what the run did after being told to stop.
struct Probe {
    /// Whether this event stops the sink.
    stops_at: fn(&Event<'_>) -> bool,
    stopped: bool,
    /// The kinds of event handed over up to and including the stopping one.
    before: Vec<&'static str>,
    /// The kinds of event handed over after it.
    after: Vec<&'static str>,
}

impl Probe {
    fn new(stops_at: fn(&Event<'_>) -> bool) -> Self {
        Self {
            stops_at,
            stopped: false,
            before: Vec::new(),
            after: Vec::new(),
        }
    }

    /// A sink that never stops: the control each test below is compared to.
    fn never() -> Self {
        Self::new(|_| false)
    }

    fn count(&self, name: &str) -> usize {
        self.before
            .iter()
            .chain(&self.after)
            .filter(|k| **k == name)
            .count()
    }
}

impl Sink for Probe {
    fn record(&mut self, event: Event<'_>) {
        let kind = kind(&event);
        if self.stopped {
            self.after.push(kind);
        } else {
            self.before.push(kind);
            self.stopped = (self.stops_at)(&event);
        }
    }

    fn stopped(&self) -> bool {
        self.stopped
    }
}

/// Runs the library's analysis of `name` into `sink`, recovering into
/// `output` where one is given.
fn analyse(name: &str, output: Option<&Path>, sink: &mut Probe) {
    let mut evidence = EvidenceFile::open(fixture(name)).expect("opening the fixture");
    let length = evidence.reported_size();
    let options = Options {
        recover: true,
        reference: None,
        output,
    };
    analysis::run(&mut evidence, length, options, sink);
}

#[test]
fn a_stopped_sink_gets_no_file_written() {
    let dir = empty_scratch("fat32-first-event");
    let mut stopping = Probe::new(|_| true);
    analyse("fat32-recover-run.img", Some(&dir), &mut stopping);
    assert!(files_under(&dir).is_empty(), "{:?}", files_under(&dir));
    assert_eq!(stopping.count("extracted"), 0);

    // The control: the same call with a sink that never stops writes files,
    // so the test above can fail.
    let control_dir = empty_scratch("fat32-first-event-control");
    let mut never = Probe::never();
    analyse("fat32-recover-run.img", Some(&control_dir), &mut never);
    assert!(!files_under(&control_dir).is_empty());
    assert!(never.count("extracted") >= 2, "{:?}", never.before);
}

#[test]
fn a_stopped_sink_gets_no_exfat_file_written() {
    let dir = empty_scratch("exfat-first-event");
    let mut stopping = Probe::new(|_| true);
    analyse("exp9-A.vhd", Some(&dir), &mut stopping);
    assert!(files_under(&dir).is_empty(), "{:?}", files_under(&dir));
    assert_eq!(stopping.count("exfat extracted"), 0);

    let control_dir = empty_scratch("exfat-first-event-control");
    let mut never = Probe::never();
    analyse("exp9-A.vhd", Some(&control_dir), &mut never);
    assert!(!files_under(&control_dir).is_empty());
    assert!(never.count("exfat extracted") >= 1, "{:?}", never.before);
}

/// The first loop over partitions only lists them, and the second reads
/// each: a sink stopped by the disk signature gets neither, and the table's
/// closing event is all it is handed after.
#[test]
fn a_stopped_sink_is_handed_no_partition() {
    let mut never = Probe::never();
    analyse("fat32-two-partitions.img", None, &mut never);
    assert_eq!(never.count("partition listed"), 2, "{:?}", never.before);
    assert_eq!(never.count("partition identified"), 2);

    let mut stopping = Probe::new(|event| matches!(event, Event::DiskSignature(_)));
    analyse("fat32-two-partitions.img", None, &mut stopping);
    assert_eq!(stopping.before, ["disk signature"]);
    assert_eq!(stopping.after, ["partitions listed"]);
}

/// The event that stops this sink comes from the finding whose file would
/// be written next: no file is, and the finding after it is not assessed.
#[test]
fn a_sink_stopped_by_a_recoverable_finding_gets_no_file_and_no_next_finding() {
    let dir = empty_scratch("fat32-recoverable");
    let mut stopping = Probe::new(|event| matches!(event, Event::Recoverable { .. }));
    analyse("fat32-recover-run.img", Some(&dir), &mut stopping);

    assert_eq!(stopping.count("recoverable"), 1, "{:?}", stopping.before);
    assert!(files_under(&dir).is_empty(), "{:?}", files_under(&dir));
    assert_eq!(stopping.count("extracted"), 0);
    assert!(
        !stopping.after.contains(&"recoverable"),
        "{:?}",
        stopping.after
    );

    let mut never = Probe::never();
    analyse("fat32-recover-run.img", None, &mut never);
    assert!(never.count("recoverable") >= 2, "{:?}", never.before);
}

/// A sink stopped at the root directory is handed no directory below it.
#[test]
fn a_stopped_sink_is_handed_no_further_directory_of_the_walk() {
    let mut never = Probe::never();
    analyse("fat32-nested-directories.img", None, &mut never);
    assert!(never.count("directory walked") >= 2, "{:?}", never.before);

    let mut stopping = Probe::new(|event| {
        matches!(
            event,
            Event::Directory {
                reached: Reached::Root,
                ..
            }
        )
    });
    analyse("fat32-nested-directories.img", None, &mut stopping);
    assert!(
        stopping.before.len() + stopping.after.len() < never.before.len(),
        "{:?} then {:?}",
        stopping.before,
        stopping.after
    );
    assert!(
        !stopping.after.contains(&"directory walked"),
        "{:?}",
        stopping.after
    );
    assert!(
        !stopping.after.contains(&"directory orphaned"),
        "{:?}",
        stopping.after
    );
}

/// A sink stopped by the first orphaned directory is handed no second one.
#[test]
fn a_stopped_sink_is_handed_no_further_orphaned_directory() {
    let mut never = Probe::never();
    analyse("fat32-formatted-tree.img", None, &mut never);
    assert!(never.count("directory orphaned") >= 2, "{:?}", never.before);

    let mut stopping = Probe::new(|event| {
        matches!(
            event,
            Event::Directory {
                reached: Reached::Orphaned { .. },
                ..
            }
        )
    });
    analyse("fat32-formatted-tree.img", None, &mut stopping);
    assert_eq!(
        stopping.count("directory orphaned"),
        1,
        "{:?}",
        stopping.before
    );
}

/// The exFAT volume's file is not extracted though the decision that stops
/// the sink is the decision of a recoverable set.
#[test]
fn a_sink_stopped_by_a_recoverable_exfat_set_gets_no_file() {
    let dir = empty_scratch("exfat-recoverable");
    let mut stopping = Probe::new(|event| {
        matches!(
            event,
            Event::ExFatDeletedSetDecided {
                outcome: DeletedSetOutcome::Recoverable { .. },
                ..
            }
        )
    });
    analyse("exp9-A.vhd", Some(&dir), &mut stopping);

    assert_eq!(
        stopping.count("exfat decided recoverable"),
        1,
        "{:?}",
        stopping.before
    );
    assert!(files_under(&dir).is_empty(), "{:?}", files_under(&dir));
    assert_eq!(stopping.count("exfat extracted"), 0);
}

/// A sink stopped by the root's end is handed no directory below it.
#[test]
fn a_stopped_sink_is_handed_no_further_exfat_directory() {
    let mut never = Probe::never();
    analyse("exp9-A.vhd", None, &mut never);
    assert!(
        never.count("exfat directory start") >= 2,
        "{:?}",
        never.before
    );

    let mut stopping = Probe::new(|event| {
        matches!(
            event,
            Event::ExFatDirectoryEnd {
                reached: ExFatReached::Root,
                ..
            }
        )
    });
    analyse("exp9-A.vhd", None, &mut stopping);
    assert_eq!(
        stopping.count("exfat directory start"),
        1,
        "{:?}",
        stopping.before
    );
    assert!(
        !stopping.after.contains(&"exfat directory start"),
        "{:?}",
        stopping.after
    );
}

/// A sink stopped by the first decided set is handed no second decision.
#[test]
fn a_stopped_sink_is_handed_no_further_exfat_decision() {
    let mut never = Probe::never();
    analyse("exp9-A.vhd", None, &mut never);
    assert!(
        never.count("exfat decided") + never.count("exfat decided recoverable") >= 2,
        "{:?}",
        never.before
    );

    let mut stopping = Probe::new(|event| matches!(event, Event::ExFatDeletedSetDecided { .. }));
    analyse("exp9-A.vhd", None, &mut stopping);
    assert!(
        !stopping.after.contains(&"exfat decided")
            && !stopping.after.contains(&"exfat decided recoverable"),
        "{:?}",
        stopping.after
    );
}

/// A sink stopped at the end of the root's listing is handed nothing of
/// phase 2: the bitmap, the deleted directories and the claims are not read
/// for events it would drop.
#[test]
fn a_stopped_sink_reads_no_exfat_phase_two() {
    let mut never = Probe::never();
    analyse("exp9-A.vhd", None, &mut never);
    assert!(never.count("exfat phase two") >= 1, "{:?}", never.before);

    let mut stopping = Probe::new(|event| {
        matches!(
            event,
            Event::ExFatDirectoryEnd {
                reached: ExFatReached::Root,
                ..
            }
        )
    });
    analyse("exp9-A.vhd", None, &mut stopping);
    for phase_two in [
        "exfat phase two",
        "exfat decided",
        "exfat decided recoverable",
    ] {
        assert_eq!(
            stopping.count(phase_two),
            0,
            "{phase_two}: {:?}",
            stopping.after
        );
    }
}
