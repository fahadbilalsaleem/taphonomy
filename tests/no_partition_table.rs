//! A volume with no partition table (`ADR-0024`).
//!
//! Sector 0 is classified before it is parsed as a table. The FAT32 test
//! runs the binary on a committed-recipe fixture; the exFAT test is a
//! differential, running the library on a partition of `exp9-A.vhd` and on
//! the same bytes with nothing before them; the last two build their
//! images here. The fixtures are generated and not committed. Run
//! `./scripts/generate-fixtures.sh` first.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use taphonomy::EvidenceFile;
use taphonomy::analysis::{self, Event, Options, Sink};
use taphonomy::partition::{PartitionTable, SectorZero, classify_sector_zero, parse_mbr};

fn fixture(name: &str) -> PathBuf {
    let path = Path::new("fixtures/partition").join(name);
    assert!(
        path.exists(),
        "fixture missing: {}\nrun ./scripts/generate-fixtures.sh first",
        path.display()
    );
    path
}

/// An empty directory of this test's own inside the folder Cargo sets aside
/// for integration tests. `name` must be distinct for each test, because
/// tests of one binary run in parallel.
fn empty_scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("no-partition-table-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

fn run(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_taphonomy"))
        .args(args)
        .output()
        .expect("running the binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// What `fixture_fat32_no_partition_table` writes to `GONE.TXT`, rebuilt
/// from the same loop: forty lines, each `taphonomy no-partition-table line`
/// and its number, 1520 bytes.
fn gone_txt() -> Vec<u8> {
    (1..=40)
        .flat_map(|i| format!("taphonomy no-partition-table line {i:03}\n").into_bytes())
        .collect()
}

#[test]
fn a_fat32_volume_with_no_partition_table_is_read_and_its_deleted_file_recovered() {
    let image = fixture("fat32-no-partition-table.img");

    let report = run(&[image.as_os_str()]);
    let out = stdout(&report);
    assert_eq!(report.status.code(), Some(0), "{out}");
    assert!(
        out.contains("partition table  none: volume boot record at sector 0 (FAT32)\n"),
        "{out}"
    );
    assert!(out.contains("\nwhole image   filesystem FAT32\n"), "{out}");
    assert!(!out.contains("disk signature"), "{out}");
    assert!(out.contains("coverage     complete\n"), "{out}");
    assert!(out.contains("  volumes analysed           1\n"), "{out}");

    let dir = empty_scratch("fat32");
    let recovered = run(&[
        image.as_os_str(),
        "--recover".as_ref(),
        "--output".as_ref(),
        dir.as_os_str(),
    ]);
    assert_eq!(recovered.status.code(), Some(0), "{}", stdout(&recovered));

    let files: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("reading the output directory")
        .map(|entry| entry.expect("a directory entry").path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("p0-"), "{name}");
    assert_eq!(fs::read(&files[0]).expect("reading it"), gone_txt());
}

/// What one library run of an image reported, for comparing two runs.
#[derive(Default)]
struct Observed {
    /// Each deleted exFAT set's position and what was decided of it.
    decisions: Vec<String>,
    /// Every classification event, by name.
    classification: Vec<&'static str>,
}

impl Sink for Observed {
    fn record(&mut self, event: Event<'_>) {
        match event {
            Event::ExFatDeletedSetDecided { position, outcome } => {
                self.decisions.push(format!("{position:?} {outcome:?}"));
            }
            Event::PartitionIdentified { .. } => self.classification.push("partition"),
            Event::WholeImageVolume(_) => self.classification.push("whole image"),
            _ => {}
        }
    }
}

/// Analyses `image`, recovering into `output`; returns what was observed and
/// the run's counts, which derive `Debug` and have no other equality.
fn analyse(image: &Path, output: &Path) -> (Observed, String) {
    let mut evidence = EvidenceFile::open(image).expect("opening the image");
    let length = evidence.reported_size();
    let options = Options {
        recover: true,
        reference: None,
        output: Some(output),
    };
    let mut observed = Observed::default();
    let counts = analysis::run(&mut evidence, length, options, &mut observed);
    (observed, format!("{counts:?}"))
}

/// Every file in `dir` by name with `from` replaced by `to` at the start,
/// with its bytes, sorted.
fn renamed_files(dir: &Path, from: &str, to: &str) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = fs::read_dir(dir)
        .expect("reading the output directory")
        .map(|entry| {
            let path = entry.expect("a directory entry").path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let name = name
                .strip_prefix(from)
                .unwrap_or_else(|| panic!("{name} does not start with {from}"));
            (format!("{to}{name}"), fs::read(&path).expect("reading it"))
        })
        .collect();
    files.sort();
    files
}

/// The exFAT partition of `exp9-A.vhd` is the same volume with and without
/// a table before it: every deleted set decided alike, every recovered file
/// equal in bytes, and only the partition number in the names differs.
#[test]
fn an_exfat_volume_decides_and_recovers_alike_inside_a_partition_and_alone() {
    let vhd = fixture("exp9-A.vhd");
    let bytes = fs::read(&vhd).expect("reading the image");
    let PartitionTable::Mbr { partitions, .. } = parse_mbr(&bytes[..512], bytes.len() as u64 / 512)
        .expect("a table")
        .table
    else {
        panic!("exp9-A.vhd is not an MBR disk");
    };
    assert_eq!(partitions.len(), 1, "{partitions:?}");
    let partition = partitions[0];

    // Exactly the entry's extent: a .vhd ends in a 512-byte footer.
    let start = partition.start_byte() as usize;
    let end = start + partition.length_bytes() as usize;
    let volume = &bytes[start..end];
    assert_eq!(
        classify_sector_zero(&volume[..512]),
        SectorZero::VolumeBootRecord(taphonomy::filesystem::Filesystem::ExFat),
        "the copy must classify as a volume, or this compares a run with itself"
    );

    let alone = empty_scratch("exfat-alone").join("volume.img");
    fs::write(&alone, volume).expect("writing the copy");
    let inside_dir = empty_scratch("exfat-inside-out");
    let alone_dir = empty_scratch("exfat-alone-out");

    let (inside, inside_counts) = analyse(&vhd, &inside_dir);
    let (whole, whole_counts) = analyse(&alone, &alone_dir);

    assert_eq!(inside.classification, ["partition"]);
    assert_eq!(whole.classification, ["whole image"]);
    assert!(inside.decisions.len() > 100, "{}", inside.decisions.len());
    assert_eq!(inside.decisions, whole.decisions);
    assert_eq!(inside_counts, whole_counts);

    let inside_files = renamed_files(&inside_dir, "p1-", "");
    assert!(!inside_files.is_empty());
    assert_eq!(inside_files, renamed_files(&alone_dir, "p0-", ""));
}

/// Runs the binary on `sector_zero` followed by zeros to 64 KiB.
fn run_on_sector_zero(name: &str, sector_zero: &[u8]) -> (Output, String) {
    let mut image = sector_zero.to_vec();
    image.resize(64 * 1024, 0);
    let path = empty_scratch(name).join("image.img");
    fs::write(&path, &image).expect("writing the image");
    let output = run(&[path.as_os_str()]);
    let out = stdout(&output);
    (output, out)
}

#[test]
fn a_sector_zero_that_is_neither_a_table_nor_a_boot_record_analyses_nothing() {
    let mut sector = vec![0u8; 512];
    sector[0x1BE] = 0x33;
    sector[0x1FE] = 0x55;
    sector[0x1FF] = 0xAA;

    let (output, out) = run_on_sector_zero("neither", &sector);
    assert_eq!(output.status.code(), Some(3), "{out}");
    assert!(
        out.contains(
            "partition table  none: sector 0 is neither a partition table nor a volume boot record\n"
        ),
        "{out}"
    );
    assert!(out.contains("coverage     none\n"), "{out}");
    assert!(out.contains("  sector 0 not analysed      1\n"), "{out}");
    assert!(!out.contains("volumes analysed           1"), "{out}");
}

#[test]
fn a_sector_zero_that_is_a_boot_record_and_lists_a_partition_analyses_nothing() {
    let image = fs::read(fixture("fat32-no-partition-table.img")).expect("reading the fixture");
    let mut sector = image[..512].to_vec();
    // Entry 1: type 0x0c, 1000 sectors from sector 2048.
    sector[0x1BE + 4] = 0x0C;
    sector[0x1BE + 8..0x1BE + 12].copy_from_slice(&2048u32.to_le_bytes());
    sector[0x1BE + 12..0x1BE + 16].copy_from_slice(&1000u32.to_le_bytes());

    let (output, out) = run_on_sector_zero("ambiguous", &sector);
    assert_eq!(output.status.code(), Some(3), "{out}");
    assert!(
        out.contains(
            "partition table  ambiguous: volume boot record at sector 0 (FAT32), used entries 1; not analysed\n"
        ),
        "{out}"
    );
    assert!(out.contains("coverage     none\n"), "{out}");
    assert!(out.contains("  sector 0 not analysed      1\n"), "{out}");
    assert!(!out.contains("whole image"), "{out}");
}
