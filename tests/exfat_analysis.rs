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

/// The first line of the exFAT caveat (`ADR-0019` section 5).
const EXFAT_CAVEAT_FIRST_LINE: &str = "A deleted exFAT file is reported with the\n";

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
    // Sets passed, content unread: the exFAT caveat, and the line about
    // reading content, each once (`ADR-0019` section 5).
    assert_eq!(out.matches(EXFAT_CAVEAT_FIRST_LINE).count(), 1, "{out}");
    assert_eq!(
        out.matches("No content read. Pass --recover to read it.\n")
            .count(),
        1,
        "{out}"
    );
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

// --- recovering under --recover (ADR-0022 section 14 item 2) -------------

/// The EXP-0009 generator's content for the file `name` of `size` bytes,
/// as `scripts/experiments/exp-0011-score-exp9.py` `content(name, size)`
/// defines it: each 512-byte block is the line
/// `TAPHONOMY EXP-0009 FILE={name} BLOCK={i:06d}\n`, then `name` and a
/// space repeated, cut to 512; the whole is cut to `size`. Written here
/// apart from the binary, so a recovered file is judged by the generator
/// and never by what the binary read.
fn content(name: &str, size: usize) -> Vec<u8> {
    const BLOCK: usize = 512;
    let mut out = Vec::with_capacity(size + BLOCK);
    let mut index = 0;
    while out.len() < size {
        let header = format!("TAPHONOMY EXP-0009 FILE={name} BLOCK={index:06}\n");
        let pad = format!("{name} ").repeat(BLOCK);
        let mut block = header.into_bytes();
        block.extend_from_slice(&pad.as_bytes()[..BLOCK - block.len()]);
        out.extend_from_slice(&block);
        index += 1;
    }
    out.truncate(size);
    out
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = taphonomy::hash::Sha256Hasher::new();
    hasher.update(bytes);
    hasher.finish().digest.to_string()
}

/// The four digest prefixes the scorer asserts its generator against.
#[test]
fn the_generators_content_is_the_scorers() {
    for (name, size, prefix) in [
        ("KEEP.txt", 5000, "799230d4"),
        ("CONTIG.bin", 262_144, "afbc355c"),
        ("SMALL.txt", 1000, "ac61dcd7"),
        ("FRAG.bin", 245_760, "446a6a9c"),
    ] {
        let digest = sha256_hex(&content(name, size));
        assert!(digest.starts_with(prefix), "{name}: {digest}");
    }
}

fn run_with(path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_taphonomy"))
        .arg(path)
        .args(args)
        .output()
        .expect("running the binary")
}

/// [`scratch`], emptied: a directory the binary is to write into, which a
/// previous run's files would otherwise answer for.
fn empty_scratch(name: &str) -> PathBuf {
    let dir = scratch(name);
    fs::remove_dir_all(&dir).expect("emptying the scratch directory");
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

/// One recovered set as the binary reported it: its name and DataLength
/// from its per-set line, then the extraction lines that follow it.
struct Recovered {
    name: String,
    size: usize,
    digest: String,
    written: Option<PathBuf>,
}

/// Every recovered set in `out`, in order.
fn recovered(out: &str) -> Vec<Recovered> {
    let lines: Vec<&str> = out.lines().collect();
    let mut found = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.ends_with("conditions 4, 1, 2 and 3 passed") {
            continue;
        }
        // `c<cluster> s<slot> <name> cluster <first>, <size> bytes, ...`
        let words: Vec<&str> = line.split_whitespace().collect();
        let name = words[2].to_string();
        let size = words[5].parse().expect("the size");
        let digest = lines[i + 1]
            .split_whitespace()
            .nth(2)
            .expect("the recovered digest")
            .to_string();
        assert!(lines[i + 1].trim_start().starts_with("recovered sha256 "));
        let written = lines.get(i + 2).and_then(|next| {
            let rest = next.trim_start().strip_prefix("written ")?;
            let path = rest
                .strip_suffix(" matches what was read")
                .unwrap_or_else(|| panic!("not read back equal: {next}"));
            Some(PathBuf::from(path))
        });
        found.push(Recovered {
            name,
            size,
            digest,
            written,
        });
    }
    found
}

/// Runs image `image` with `--recover --output`, and checks every file
/// written against the generator, byte for byte, and that nothing else was
/// written. Returns the binary's stdout.
fn recover_and_check(image: &str, scratch_name: &str, expected: usize) -> String {
    let dir = empty_scratch(scratch_name);
    let output = run_with(
        &fixture(image),
        &["--recover", "--output", dir.to_str().expect("a UTF-8 path")],
    );
    let out = stdout(&output);
    assert_eq!(output.status.code(), Some(0), "{out}");

    let sets = recovered(&out);
    assert_eq!(sets.len(), expected, "{out}");
    for set in &sets {
        let path = set
            .written
            .as_ref()
            .unwrap_or_else(|| panic!("{} not written\n{out}", set.name));
        let bytes = fs::read(path).expect("reading a recovered file");
        assert!(
            bytes == content(&set.name, set.size),
            "{} differs from the generator's content",
            set.name
        );
        assert_eq!(set.digest, sha256_hex(&bytes), "{}", set.name);
    }
    assert_eq!(
        fs::read_dir(&dir).expect("listing the output").count(),
        expected,
        "a file was written for a set not recovered"
    );
    assert!(
        out.contains(&format!("artifacts    {expected} UNVERIFIED\n")),
        "{out}"
    );
    assert!(out.contains("coverage     complete\n"), "{out}");
    // FAT32's caveat is about a free run; no FAT32 run was read here.
    assert!(!out.contains("A free run means"), "{out}");
    out
}

/// Image A with its bitmap zeroed, under `--recover`: no set is decided,
/// so no artifact, and the line says `0` as a run with none always has.
#[test]
fn a_recovering_run_with_no_artifact_reports_zero() {
    let mut bytes = fs::read(fixture("exp9-A.vhd")).expect("reading image A");
    let at = image_a_cluster(2);
    bytes[at..at + 474].fill(0);
    let copy = scratch("recover-nothing").join("a-altered.vhd");
    fs::write(&copy, &bytes).expect("writing the copy");

    let output = run_with(&copy, &["--recover"]);
    let out = stdout(&output);
    assert!(out.contains("\nartifacts    0\n"), "{out}");
}

/// Image A: 96 recovered, each the generator's file; 4 refused under
/// condition 1, 16 under condition 3, 6 holding no data (ADR-0022 section
/// 14 item 2; EXP-0011's scorer, rule `moe`).
#[test]
fn image_a_recovers_96_files_each_equal_to_the_generators() {
    let out = recover_and_check("exp9-A.vhd", "recover-a", 96);
    assert_eq!(out.matches("REFUSED, condition 1:").count(), 4, "{out}");
    assert_eq!(
        out.matches("REFUSED, condition 3: zeros only\n").count(),
        16,
        "{out}"
    );
    assert_eq!(out.matches("no content: no cluster and no data").count(), 6);
    assert_eq!(out.matches(EXFAT_CAVEAT_FIRST_LINE).count(), 1, "{out}");
    assert!(out.contains("No reference supplied. Nothing was validated.\n"));
}

/// Image B: 99 recovered, each the generator's file, FRAG.bin among them
/// by its chain; 1 refused under condition 1, 3 under condition 2, 16 under
/// condition 3, 6 holding no data (`ADR-0019` section 6 items 1 and 2).
#[test]
fn image_b_recovers_99_files_frag_bin_by_its_chain() {
    let out = recover_and_check("exp9-B.vhd", "recover-b", 99);
    assert!(
        recovered(&out)
            .iter()
            .any(|set| set.name == "FRAG.bin" && set.size == 245_760),
        "{out}"
    );
    assert_eq!(out.matches("REFUSED, condition 1:").count(), 1, "{out}");
    assert_eq!(out.matches("REFUSED, condition 2:").count(), 3, "{out}");
    assert_eq!(
        out.matches("REFUSED, condition 3: zeros only\n").count(),
        16,
        "{out}"
    );
    assert_eq!(out.matches("no content: no cluster and no data").count(), 6);
}

/// A second run into the same directory finds each of A's 96 files there:
/// each is left as it was, its digest still reported, and none delivered,
/// a gap the summary states.
#[test]
fn image_a_recovered_twice_into_one_directory_delivers_nothing_the_second_time() {
    let dir = empty_scratch("recover-twice");
    let args = ["--recover", "--output", dir.to_str().expect("a UTF-8 path")];
    run_with(&fixture("exp9-A.vhd"), &args);

    let output = run_with(&fixture("exp9-A.vhd"), &args);
    let out = stdout(&output);
    assert_eq!(out.matches(" exists\n").count(), 96, "{out}");
    assert!(
        out.contains(&gap_line("artifacts not written", 96)),
        "{out}"
    );
    assert!(out.contains("coverage     incomplete\n"), "{out}");
    assert!(out.contains("artifacts    96 UNVERIFIED\n"), "{out}");
}

/// B against the generator's FRAG.bin: one of the 99 matches, 98 differ,
/// and the summary states both (`ADR-0022` section 11).
#[test]
fn image_b_compared_with_frag_bins_digest_matches_once() {
    let reference = sha256_hex(&content("FRAG.bin", 245_760));
    let output = run_with(
        &fixture("exp9-B.vhd"),
        &["--recover", "--reference-digest", &reference],
    );
    let out = stdout(&output);
    assert_eq!(output.status.code(), Some(0), "{out}");
    assert!(
        out.contains("compared     1 matched, 98 differed\n"),
        "{out}"
    );
    assert_eq!(out.matches(" MATCHES\n").count(), 1, "{out}");
    // The match paragraph is not FAT32's: an exFAT match prints it, once.
    assert_eq!(
        out.matches("A match establishes that these bytes are the\n")
            .count(),
        1,
        "{out}"
    );
    // The differ paragraph's causes are a FAT32 run's; none was read.
    assert!(!out.contains("A differing digest"), "{out}");
}

/// The copy of image A whose first sector is zeroed is analysed from its
/// backup boot region, and recovers as A does: the options reach that path
/// too.
#[test]
fn a_volume_found_by_its_backup_region_recovers_as_image_a() {
    let mut bytes = fs::read(fixture("exp9-A.vhd")).expect("reading image A");
    let start = (VOLUME_LBA * 512) as usize;
    bytes[start..start + 512].fill(0);
    let copy = scratch("zeroed-recover").join("a-zeroed.vhd");
    fs::write(&copy, &bytes).expect("writing the copy");

    let output = run_with(&copy, &["--recover"]);
    let out = stdout(&output);
    assert_eq!(output.status.code(), Some(0), "{out}");
    assert!(
        out.contains("filesystem exFAT, identified from its backup boot region\n"),
        "{out}"
    );
    assert_eq!(recovered(&out).len(), 96, "{out}");
    assert!(out.contains("artifacts    96 UNVERIFIED\n"), "{out}");
}

/// Without `--output`, image A's 96 are read and hashed, each digest the
/// generator's file's, and nothing is written.
#[test]
fn image_a_with_recover_and_no_output_writes_nothing() {
    let output = run_with(&fixture("exp9-A.vhd"), &["--recover"]);
    let out = stdout(&output);
    assert_eq!(output.status.code(), Some(0), "{out}");

    let sets = recovered(&out);
    assert_eq!(sets.len(), 96, "{out}");
    for set in &sets {
        assert!(set.written.is_none(), "{} written\n{out}", set.name);
        assert_eq!(
            set.digest,
            sha256_hex(&content(&set.name, set.size)),
            "{}",
            set.name
        );
    }
    assert!(!out.contains("written "), "{out}");
    assert!(out.contains("artifacts    96 UNVERIFIED\n"), "{out}");
}
