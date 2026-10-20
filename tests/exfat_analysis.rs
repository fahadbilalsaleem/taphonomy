//! exFAT volumes analysed end to end through the binary.
//!
//! The two images are EXP-0009's, committed compressed and expanded by
//! `./scripts/generate-fixtures.sh`. The expected counts come from The Sleuth
//! Kit 4.12.1 and the EXP-0009 analysis script, which parse the same bytes
//! independently: 122 unused File entries in image A and 125 in image B.
//!
//! Two further tests build their inputs here: a copy of image A whose
//! volume's first sector is zeroed, and a partition of unrelated bytes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Where EXP-0009's volume begins, in 512-byte sectors.
const VOLUME_LBA: u64 = 128;

fn fixture(name: &str) -> PathBuf {
    let path = Path::new("fixtures/partition").join(name);
    assert!(
        path.exists(),
        "fixture missing: {}\nrun ./scripts/generate-fixtures.sh first",
        path.display()
    );
    path
}

fn run(path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_taphonomy"))
        .arg(path)
        .output()
        .expect("running the binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A scratch directory of this test's own, never removed by the test: the
/// next run replaces what is in it.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("taphonomy-exfat-{name}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

#[test]
fn image_a_is_analysed_and_its_deleted_entry_sets_are_a_gap() {
    let output = run(&fixture("exp9-A.vhd"));
    let out = stdout(&output);

    assert_eq!(output.status.code(), Some(0), "{out}");
    assert!(out.contains("partition 1   filesystem exFAT"), "{out}");
    assert!(out.contains("    boot region          main\n"), "{out}");
    assert!(out.contains("coverage     incomplete"), "{out}");
    assert!(out.contains("  volumes analysed           1\n"), "{out}");
    assert!(out.contains("  deleted sets not examined  122\n"), "{out}");
    assert!(
        out.contains("      c5 s0                volume label  EXP9\n"),
        "{out}"
    );
    assert!(
        out.contains(
            "      c1356 s102           file          FILL115.bin   65536 bytes, cluster 1901\n"
        ),
        "{out}"
    );
    assert!(
        out.contains("file          KEEP.txt      5000 bytes, cluster 8"),
        "{out}"
    );
    assert!(
        out.contains("directory /System Volume Information"),
        "{out}"
    );
    assert!(out.contains("WPSettings.dat 12 bytes"), "{out}");
}

#[test]
fn image_b_lists_what_survived_and_counts_the_three_targets_among_its_deleted_sets() {
    let output = run(&fixture("exp9-B.vhd"));
    let out = stdout(&output);

    assert_eq!(output.status.code(), Some(0), "{out}");
    assert!(out.contains("coverage     incomplete"), "{out}");
    assert!(out.contains("  deleted sets not examined  125\n"), "{out}");
    assert!(out.contains("KEEP.txt"), "{out}");
    assert!(out.contains("IndexerVolumeGuid"), "{out}");
    // The three targets were deleted: they are not live entries.
    for deleted in ["CONTIG.bin", "SMALL.txt", "FRAG.bin"] {
        assert!(
            !out.contains(&format!("file          {deleted}")),
            "{deleted} was listed as live\n{out}"
        );
    }
}

/// The volume's first sector is zeroed in a copy of image A. The volume's
/// backup boot region is intact, so it is analysed from it.
#[test]
fn a_volume_whose_first_sector_is_zeroed_is_analysed_from_its_backup_boot_region() {
    let mut bytes = fs::read(fixture("exp9-A.vhd")).expect("reading image A");
    let start = (VOLUME_LBA * 512) as usize;
    bytes[start..start + 512].fill(0);
    let copy = scratch("zeroed-first-sector").join("a-zeroed.vhd");
    fs::write(&copy, &bytes).expect("writing the copy");

    let output = run(&copy);
    let out = stdout(&output);

    assert_eq!(output.status.code(), Some(0), "{out}");
    assert!(!out.contains("filesystem unknown"), "{out}");
    assert!(
        out.contains(
            "partition 1   filesystem exFAT, identified from its backup boot region\n\
             \x20   sector 0             no boot signature: found 0000, expected 55aa\n"
        ),
        "{out}"
    );
    assert!(
        out.contains("    boot region          backup, found by trying each sector size\n"),
        "{out}"
    );
    assert!(out.contains("  volumes analysed           1\n"), "{out}");
    assert!(out.contains("  deleted sets not examined  122\n"), "{out}");
    assert!(out.contains("KEEP.txt"), "{out}");
}

/// A partition declared as type 0x07 and holding bytes no filesystem wrote
/// is reported as it always was: unidentified.
#[test]
fn a_0x07_partition_of_unrelated_bytes_is_still_reported_as_unidentified() {
    let mut image = vec![0u8; 1 << 20];
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    for byte in image[512..].iter_mut() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *byte = (state >> 56) as u8;
    }

    // One partition entry: type 0x07, from sector 1 to the end.
    let sectors = (image.len() / 512 - 1) as u32;
    let entry = &mut image[446..462];
    entry[0] = 0x00;
    entry[1..4].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    entry[4] = 0x07;
    entry[5..8].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    entry[8..12].copy_from_slice(&1u32.to_le_bytes());
    entry[12..16].copy_from_slice(&sectors.to_le_bytes());
    image[510] = 0x55;
    image[511] = 0xAA;
    // The random bytes began at sector 1, so sector 0's other bytes are
    // zero apart from the table and the signature.

    let path = scratch("unrelated-0x07").join("unrelated.img");
    fs::write(&path, &image).expect("writing the image");

    let output = run(&path);
    let out = stdout(&output);

    assert_eq!(output.status.code(), Some(3), "{out}");
    assert!(out.contains("filesystem unknown"), "{out}");
    assert!(out.contains("  filesystems not identified 1\n"), "{out}");
    assert!(!out.contains("boot region"), "{out}");
    assert!(!out.contains("exFAT"), "{out}");
}
