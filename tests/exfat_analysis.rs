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

/// A scratch directory of this test's own, `exfat-{name}` inside the folder
/// Cargo sets aside for integration tests in the target directory
/// (`CARGO_TARGET_TMPDIR`). The test does not remove it; the next run
/// writes over the copy in it, so there is one per test name per target
/// directory, and it goes when the target directory does. `name` must be
/// distinct for each test.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("exfat-{name}"));
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

#[test]
fn image_a_is_analysed_and_every_deleted_set_is_decided() {
    let output = run(&fixture("exp9-A.vhd"));
    let out = stdout(&output);

    assert_eq!(output.status.code(), Some(0), "{out}");
    assert!(out.contains("partition 1   filesystem exFAT"), "{out}");
    assert!(out.contains("    boot region          main\n"), "{out}");
    // EXP-0011's scanner and scorer (2026-11-17): of 122 deleted sets, 6
    // hold no data, 4 are refused under condition 1, and 112 pass 4, 1 and
    // 2, with condition 3 not checked. No gap of any kind (ADR-0022
    // section 11).
    assert!(out.contains("coverage     complete\n"), "{out}");
    assert!(out.contains("  volumes analysed           1\n"), "{out}");
    assert_image_a_decisions(&out);
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

/// Image A's deleted sets, decided as EXP-0011's scanner table
/// (`exp-0011-scan.py --deleted-dirs`) and scorer (`exp-0011-score-exp9.py`)
/// decide them, through the summary and one line of each kind A holds.
fn assert_image_a_decisions(out: &str) {
    assert!(out.contains("      deleted entry sets 122\n"), "{out}");
    assert!(!out.contains("not examined"), "{out}");
    assert!(out.contains("\n    deleted entry sets\n"), "{out}");
    assert!(
        out.contains("unrecovered  4 refused, 112 not read, 6 size zero\n"),
        "{out}"
    );
    assert!(!out.contains("entries not assessed"), "{out}");
    assert!(
        out.contains(
            "      c5 s19               FILL002.bin   REFUSED, condition 1: cluster 91 is in use\n"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "      c3407 s69            FILL232.bin   REFUSED, condition 1: cluster 3776 is in use\n"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "      c3407 s72            FILL233.bin   no content: no cluster and no data\n"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "      c5 s37               FILL008.bin   cluster 187, 65536 bytes, \
             conditions 4, 1 and 2 passed, condition 3 not checked\n"
        ),
        "{out}"
    );
}

#[test]
fn image_b_lists_what_survived_and_counts_the_three_targets_among_its_deleted_sets() {
    let output = run(&fixture("exp9-B.vhd"));
    let out = stdout(&output);

    assert_eq!(output.status.code(), Some(0), "{out}");
    // EXP-0011's scanner and scorer (2026-11-17): of 125 deleted sets, 6
    // hold no data, 1 is refused under condition 1, 3 under condition 2
    // (FRAG.bin, created later, claims their clusters), and 115 pass 4, 1
    // and 2, with condition 3 not checked. No gap of any kind.
    assert!(out.contains("coverage     complete\n"), "{out}");
    assert!(out.contains("      deleted entry sets 125\n"), "{out}");
    assert!(!out.contains("not examined"), "{out}");
    assert!(
        out.contains("unrecovered  4 refused, 115 not read, 6 size zero\n"),
        "{out}"
    );
    assert!(!out.contains("entries not assessed"), "{out}");
    for line in [
        "      c5 s19               FILL002.bin   REFUSED, condition 2: a later claim\n",
        "      c3407 s69            FILL232.bin   REFUSED, condition 2: a later claim\n",
        "      c5 s31               FILL006.bin   REFUSED, condition 1: cluster 165 is in use\n",
        "      c3407 s72            FILL233.bin   no content: no cluster and no data\n",
        "      c5 s10               CONTIG.bin    cluster 10, 262144 bytes, \
         conditions 4, 1 and 2 passed, condition 3 not checked\n",
        "      c3407 s90            FRAG.bin      cluster 3776, 245760 bytes, \
         conditions 4, 1 and 2 passed, condition 3 not checked\n",
    ] {
        assert!(out.contains(line), "{line}\n{out}");
    }
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
    assert!(out.contains("coverage     complete\n"), "{out}");
    assert_image_a_decisions(&out);
    assert!(out.contains("KEEP.txt"), "{out}");
}

/// Where image A's cluster `cluster` begins, in bytes: its heap begins 256
/// sectors into the volume, and a cluster is 4096 bytes.
fn image_a_cluster(cluster: u64) -> usize {
    ((VOLUME_LBA + 256) * 512 + (cluster - 2) * 4096) as usize
}

/// Where the entry at `slot` of image A's root cluster 5 begins.
fn image_a_root_slot(slot: usize) -> usize {
    image_a_cluster(5) + slot * 32
}

/// A summary gap line, as the binary pads its labels.
fn gap_line(label: &str, count: usize) -> String {
    format!("  {label:<26} {count}\n")
}

/// Runs the binary on a copy of image A changed by `edit`.
fn run_altered_a(name: &str, edit: impl FnOnce(&mut Vec<u8>)) -> (Output, String) {
    let mut bytes = fs::read(fixture("exp9-A.vhd")).expect("reading image A");
    edit(&mut bytes);
    let copy = scratch(name).join("a-altered.vhd");
    fs::write(&copy, &bytes).expect("writing the copy");
    let output = run(&copy);
    let out = stdout(&output);
    (output, out)
}

/// An entry set's SetChecksum, section 6.3.3, written here apart from the
/// reader: every byte of the set but the two that hold it.
fn set_checksum(set: &[u8]) -> u16 {
    let mut checksum: u16 = 0;
    for (index, &byte) in set.iter().enumerate() {
        if index != 2 && index != 3 {
            checksum = checksum.rotate_right(1).wrapping_add(u16::from(byte));
        }
    }
    checksum
}

/// Image A with its allocation bitmap zeroed: every cluster reads free,
/// the root's among them, so the bitmap is not believed and none of the
/// 122 deleted sets is assessed (ADR-0022 Appendix A, S6-2).
#[test]
fn a_bitmap_not_believed_leaves_every_deleted_set_of_image_a_not_assessed() {
    let (output, out) = run_altered_a("bitmap-zeroed", |bytes| {
        let at = image_a_cluster(2);
        bytes[at..at + 474].fill(0);
    });

    assert_eq!(output.status.code(), Some(0), "{out}");
    assert!(out.contains("coverage     incomplete\n"), "{out}");
    assert!(
        out.contains(&gap_line("entries not assessed", 122)),
        "{out}"
    );
    assert!(out.contains("unrecovered  122 not assessed\n"), "{out}");
    assert!(
        out.contains(
            "    deleted entry sets\n      NOT ASSESSED: 122 deleted sets: \
             allocation bitmap not believed: "
        ),
        "{out}"
    );
}

/// Seals a deleted set of three entries again after it was changed: its
/// checksum is over the set as written, every InUse bit set.
fn reseal(set: &mut [u8]) {
    let mut sealed = set.to_vec();
    for entry in sealed.chunks_mut(32) {
        entry[0] |= 0x80;
    }
    set[2..4].copy_from_slice(&set_checksum(&sealed).to_le_bytes());
}

/// Image A with three deleted sets altered:
///
/// * FILL002.bin (slot 19) marked a directory and sealed again: its
///   clusters are in use, so it is a deleted directory that is not read.
/// * FILL008.bin (slot 37) with a byte of its name changed: its checksum
///   fails, so it is not usable.
/// * FILL010.bin (slot 43) marked a directory of one cluster, 219, and
///   sealed again; cluster 219 is zeroed and given a copy of FILL012.bin's
///   set (slot 49) with a byte of its name changed. The directory is read,
///   and the set inside is not usable, so its claim is unseen.
#[test]
fn a_deleted_directory_not_read_and_an_unusable_set_are_gaps_on_image_a() {
    let (output, out) = run_altered_a("directory-and-unusable", |bytes| {
        let at = image_a_root_slot(19);
        let set = &mut bytes[at..at + 96];
        set[4] = 0x10;
        set[5] = 0x00;
        reseal(set);

        bytes[image_a_root_slot(37) + 2 * 32 + 2] ^= 1;

        let at = image_a_root_slot(43);
        let set = &mut bytes[at..at + 96];
        set[4] = 0x10;
        set[5] = 0x00;
        // ValidDataLength and DataLength, in the Stream Extension.
        set[32 + 8..32 + 16].copy_from_slice(&4096u64.to_le_bytes());
        set[32 + 24..32 + 32].copy_from_slice(&4096u64.to_le_bytes());
        reseal(set);

        let mut inside = bytes[image_a_root_slot(49)..image_a_root_slot(49) + 96].to_vec();
        inside[2 * 32 + 2] ^= 1;
        let cluster = image_a_cluster(219);
        bytes[cluster..cluster + 4096].fill(0);
        bytes[cluster..cluster + 96].copy_from_slice(&inside);
    });

    assert_eq!(output.status.code(), Some(0), "{out}");
    assert!(out.contains("coverage     incomplete\n"), "{out}");
    assert!(
        out.contains(&gap_line("deleted sets not usable", 1)),
        "{out}"
    );
    assert!(
        out.contains(&gap_line("deleted directories unread", 1)),
        "{out}"
    );
    assert!(
        out.contains(&gap_line("claimants not followed", 1)),
        "{out}"
    );
    for name in ["FILL002.bin", "FILL010.bin"] {
        assert!(
            out.contains(&format!("   {name}   deleted directory, not decided\n")),
            "{name}\n{out}"
        );
    }
    assert!(
        out.contains("      c5 s19               FILL002.bin   deleted directory, not decided\n"),
        "{out}"
    );
    assert!(!out.contains("FILL008.bin   cluster"), "{out}");
    assert!(
        out.contains("unrecovered  3 refused, 110 not read, 6 size zero\n"),
        "{out}"
    );
}

/// Image A with its partition entry copied into the second slot: the one
/// volume is analysed twice, as two partitions, and each volume's
/// decisions are printed under a heading of its own.
#[test]
fn each_exfat_volume_prints_its_own_deleted_entry_sets_heading() {
    let (output, out) = run_altered_a("two-volumes", |bytes| {
        let first = bytes[446..462].to_vec();
        bytes[462..478].copy_from_slice(&first);
    });

    assert_eq!(output.status.code(), Some(0), "{out}");
    assert_eq!(out.matches("filesystem exFAT\n").count(), 2, "{out}");
    assert_eq!(
        out.matches("\n    deleted entry sets\n").count(),
        2,
        "{out}"
    );
    assert_eq!(
        out.matches(
            "      c3407 s72            FILL233.bin   no content: no cluster and no data\n"
        )
        .count(),
        2,
        "{out}"
    );
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
