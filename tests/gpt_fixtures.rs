//! GPT disks through the binary. `ADR-0025`.
//!
//! The patched copies are written under `CARGO_TARGET_TMPDIR`; the fixtures
//! themselves are never modified. The fixtures are generated and not
//! committed: run `./scripts/generate-fixtures.sh` first.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use taphonomy::gpt::{BASIC_DATA, crc32};

const SECTOR: usize = 512;

fn fixture(name: &str) -> PathBuf {
    let path = Path::new("fixtures/partition").join(name);
    assert!(
        path.exists(),
        "fixture missing: {}\nrun ./scripts/generate-fixtures.sh first",
        path.display()
    );
    path
}

fn run_on(bytes: &[u8], name: &str) -> (i32, String) {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    fs::write(&path, bytes).expect("writing the patched copy");
    let output: Output = Command::new(env!("CARGO_BIN_EXE_taphonomy"))
        .arg(&path)
        .output()
        .expect("running the binary");
    (
        output.status.code().expect("the binary exited by signal"),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

fn healthy() -> Vec<u8> {
    fs::read(fixture("gpt-protective.img")).expect("reading the fixture")
}

#[test]
fn the_fixtures_partition_is_analysed_through_its_gpt() {
    // The fixture's partition holds no filesystem, so, as the same
    // partition in an MBR would be, it is analysed and nothing is
    // identified: exit status 3, one filesystem not identified.
    let (status, out) = run_on(&healthy(), "gpt-healthy.img");
    assert_eq!(status, 3, "{out}");
    assert!(
        out.contains("header primary"),
        "the primary header should be used: {out}"
    );
    assert!(
        out.contains("type ebd0a0a2-b9e5-4433-87c0-68b6b72699c7 (Microsoft basic data)"),
        "{out}"
    );
    assert!(
        out.contains("partition 1   filesystem unknown (no boot signature"),
        "{out}"
    );
    assert!(out.contains("filesystems not identified 1"), "{out}");
    assert!(!out.contains("GPT not analysed"), "{out}");
    assert!(!out.contains("GPT detected"), "{out}");
    assert!(!out.contains("disk signature"), "{out}");
}

#[test]
fn a_broken_primary_header_gives_the_backup_and_the_reason() {
    let mut bytes = healthy();
    bytes[SECTOR + 56] ^= 0xFF; // the primary's DiskGUID, not its CRC
    let (status, out) = run_on(&bytes, "gpt-primary-broken.img");
    assert_eq!(status, 3, "{out}"); // the partition holds no filesystem
    assert!(
        out.contains("header backup (primary: header CRC does not match)"),
        "{out}"
    );
    assert!(
        out.contains("partition 1   filesystem unknown (no boot signature"),
        "{out}"
    );
    assert!(!out.contains("GPT not analysed"), "{out}");
}

#[test]
fn both_headers_broken_is_a_gap_and_exit_three() {
    let mut bytes = healthy();
    let backup = bytes.len() - SECTOR;
    bytes[SECTOR + 56] ^= 0xFF;
    bytes[backup + 56] ^= 0xFF;
    let (status, out) = run_on(&bytes, "gpt-both-broken.img");
    assert_eq!(status, 3, "{out}");
    assert!(
        out.contains("GPT not analysed: primary header: header CRC does not match; backup header: header CRC does not match"),
        "{out}"
    );
    assert!(out.contains("coverage     none"), "{out}");
    assert!(out.contains("GPT not analysed           1"), "{out}");
}

#[test]
fn a_gpt_with_4096_byte_blocks_is_reported_and_not_read() {
    let mut bytes = vec![0u8; 1 << 20];
    // A protective record at sector 0, then "EFI PART" at byte 4096 only.
    bytes[0x1BE + 4] = 0xEE;
    bytes[0x1BE + 8..0x1BE + 12].copy_from_slice(&1u32.to_le_bytes());
    bytes[0x1BE + 12..0x1BE + 16].copy_from_slice(&u32::MAX.to_le_bytes());
    bytes[0x1FE] = 0x55;
    bytes[0x1FF] = 0xAA;
    bytes[4096..4104].copy_from_slice(b"EFI PART");
    let (status, out) = run_on(&bytes, "gpt-4096.img");
    assert_eq!(status, 3, "{out}");
    assert!(
        out.contains("GPT not analysed: logical blocks of 4096 bytes are not read"),
        "{out}"
    );
}

/// `ADR-0025` Decision B: a GPT left behind a legacy MBR is not honoured.
#[test]
fn a_stale_gpt_behind_a_legacy_mbr_is_reported_and_the_mbr_is_read() {
    let mut bytes = healthy();
    // Replace the protective record with the FAT32 partition's own.
    let slot = 0x1BE;
    bytes[slot..slot + 16].fill(0);
    bytes[slot + 4] = 0x0C;
    bytes[slot + 8..slot + 12].copy_from_slice(&2048u32.to_le_bytes());
    bytes[slot + 12..slot + 16].copy_from_slice(&128_991u32.to_le_bytes());
    let (_, out) = run_on(&bytes, "gpt-stale.img");
    assert!(out.contains("disk signature"), "{out}");
    assert!(
        out.contains("partition 1   type 0x0c   start 2048   sectors 128991"),
        "the MBR's partition is the one listed: {out}"
    );
    assert!(
        out.contains("a GPT header lies at sector 1 behind a legacy MBR and is not honoured"),
        "{out}"
    );
    assert!(!out.contains("partition table GPT"), "{out}");
}

#[test]
fn an_mbr_with_no_header_behind_it_reports_no_stale_gpt() {
    let bytes = fs::read(fixture("mbr-single-fat32.img")).expect("reading the fixture");
    let (_, out) = run_on(&bytes, "mbr-no-stale.img");
    assert!(!out.contains("behind a legacy MBR"), "{out}");
}

/// The exFAT volume of `exp9-A.vhd` (an MBR image: partition 1 at sector
/// 128, 30,592 sectors) moved under a GPT whose one entry has `type_guid`,
/// with the volume's first sector zeroed, so that only its backup boot
/// region identifies it.
fn exfat_under_gpt(type_guid: [u8; 16]) -> Vec<u8> {
    exfat_under_gpt_with(type_guid, true)
}

/// As [`exfat_under_gpt`]; `zero_first` says whether the volume's first
/// sector is zeroed, so that only its backup boot region identifies it.
fn exfat_under_gpt_with(type_guid: [u8; 16], zero_first: bool) -> Vec<u8> {
    let mut disk = fs::read(fixture("exp9-A.vhd")).expect("reading the fixture");
    let sectors = (disk.len() / SECTOR) as u64;
    let (first, last) = (128u64, 128u64 + 30_592 - 1);
    let backup_array = sectors - 33;

    disk[..SECTOR].fill(0);
    disk[0x1BE + 4] = 0xEE;
    disk[0x1BE + 8..0x1BE + 12].copy_from_slice(&1u32.to_le_bytes());
    disk[0x1BE + 12..0x1BE + 16].copy_from_slice(&u32::MAX.to_le_bytes());
    disk[0x1FE] = 0x55;
    disk[0x1FF] = 0xAA;
    if zero_first {
        disk[128 * SECTOR..129 * SECTOR].fill(0);
    }

    let mut array = vec![0u8; 128 * 128];
    array[..16].copy_from_slice(&type_guid);
    array[16] = 1;
    array[32..40].copy_from_slice(&first.to_le_bytes());
    array[40..48].copy_from_slice(&last.to_le_bytes());
    let array_crc = crc32(&array);

    for (lba, alternate, entries) in [(1u64, sectors - 1, 2u64), (sectors - 1, 1, backup_array)] {
        let mut header = [0u8; SECTOR];
        header[..8].copy_from_slice(b"EFI PART");
        header[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        header[12..16].copy_from_slice(&92u32.to_le_bytes());
        header[24..32].copy_from_slice(&lba.to_le_bytes());
        header[32..40].copy_from_slice(&alternate.to_le_bytes());
        header[40..48].copy_from_slice(&34u64.to_le_bytes());
        header[48..56].copy_from_slice(&(backup_array - 1).to_le_bytes());
        header[56..72].copy_from_slice(&[7; 16]);
        header[72..80].copy_from_slice(&entries.to_le_bytes());
        header[80..84].copy_from_slice(&128u32.to_le_bytes());
        header[84..88].copy_from_slice(&128u32.to_le_bytes());
        header[88..92].copy_from_slice(&array_crc.to_le_bytes());
        let crc = crc32(&header[..92]);
        header[16..20].copy_from_slice(&crc.to_le_bytes());
        let at = lba as usize * SECTOR;
        disk[at..at + SECTOR].copy_from_slice(&header);
        let at = entries as usize * SECTOR;
        disk[at..at + array.len()].copy_from_slice(&array);
    }
    disk
}

/// `ADR-0025` Decision F, end to end.
#[test]
fn the_exfat_backup_boot_region_is_searched_behind_a_basic_data_entry_only() {
    let (status, out) = run_on(&exfat_under_gpt(BASIC_DATA), "gpt-exfat-basic.img");
    assert_eq!(status, 0, "{out}");
    assert!(
        out.contains("partition 1   filesystem exFAT, identified from its backup boot region"),
        "{out}"
    );

    // EFI System Partition: the same bytes, a type that is not basic data.
    let efi_system = [
        0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9,
        0x3B,
    ];
    let (_, out) = run_on(&exfat_under_gpt(efi_system), "gpt-exfat-efi.img");
    assert!(
        !out.contains("identified from its backup boot region"),
        "{out}"
    );
    assert!(out.contains("partition 1   filesystem unknown"), "{out}");
}

/// `ADR-0025` Decision F for an MBR 0x07 partition: the same search, behind
/// the table's type byte rather than the GPT's basic data.
#[test]
fn the_exfat_backup_boot_region_is_searched_behind_an_mbr_0x07_partition() {
    let mut bytes = fs::read(fixture("exp9-A.vhd")).expect("reading the fixture");
    assert_eq!(bytes[0x1BE + 4], 0x07, "the fixture's partition is 0x07");
    bytes[128 * SECTOR..129 * SECTOR].fill(0);
    let (_, out) = run_on(&bytes, "mbr-exfat-0x07-zeroed.img");
    assert!(
        out.contains("partition 1   filesystem exFAT, identified from its backup boot region"),
        "{out}"
    );
}

/// A declared type the GPT does not carry must not be compared with the
/// filesystem: no mismatch is reported (`ADR-0025` Decision F).
#[test]
fn a_gpt_partition_is_not_reported_as_a_type_mismatch() {
    let (_, out) = run_on(
        &exfat_under_gpt_with(BASIC_DATA, false),
        "gpt-exfat-intact.img",
    );
    assert!(out.contains("filesystem exFAT"), "{out}");
    assert!(!out.contains("MISMATCH"), "{out}");
}

/// A used entry's anomaly reaches the report: a backup that fails its check.
#[test]
fn an_anomaly_on_the_backup_header_is_reported() {
    let mut bytes = healthy();
    let backup = bytes.len() - SECTOR;
    bytes[backup + 56] ^= 0xFF;
    let (_, out) = run_on(&bytes, "gpt-backup-broken.img");
    assert!(out.contains("anomalies"), "{out}");
    assert!(
        out.contains("GPT backup header at sector 131071 is invalid: header CRC does not match"),
        "{out}"
    );
}

/// An entry left out, because it lies past the image, is a gap: the run
/// says how many partitions it rejected.
#[test]
fn an_entry_past_the_image_is_a_gap_in_the_run() {
    let bytes = healthy()[..1000 * SECTOR].to_vec();
    let (status, out) = run_on(&bytes, "gpt-truncated.img");
    assert_eq!(status, 3, "{out}");
    assert!(out.contains("partition entries rejected 1"), "{out}");
}

/// Rewrites both entry arrays' and both headers' CRCs for an image of
/// `sectors` sectors, after the arrays have been changed.
fn reseal_gpt(bytes: &mut [u8], sectors: usize) {
    for (header_lba, array_lba) in [(1usize, 2usize), (sectors - 1, sectors - 33)] {
        let array = array_lba * SECTOR;
        let crc = crc32(&bytes[array..array + 128 * 128]);
        let header = header_lba * SECTOR;
        bytes[header + 88..header + 92].copy_from_slice(&crc.to_le_bytes());
        bytes[header + 16..header + 20].fill(0);
        let hcrc = crc32(&bytes[header..header + 92]);
        bytes[header + 16..header + 20].copy_from_slice(&hcrc.to_le_bytes());
    }
}

/// The entry number an artifact and a heading carry is the entry's index
/// plus one, whichever entry it is.
#[test]
fn the_second_entry_is_reported_as_partition_two() {
    let sectors = 131_072;
    let mut bytes = healthy();
    for array in [2 * SECTOR, (sectors - 33) * SECTOR] {
        let first = bytes[array..array + 128].to_vec();
        let second = array + 128;
        bytes[second..second + 128].copy_from_slice(&first);
        bytes[second + 32..second + 40].copy_from_slice(&4096u64.to_le_bytes());
        bytes[second + 40..second + 48].copy_from_slice(&4199u64.to_le_bytes());
    }
    reseal_gpt(&mut bytes, sectors);
    let (status, out) = run_on(&bytes, "gpt-two-entries.img");
    assert_eq!(status, 3, "{out}");
    assert!(out.contains("partition 2   filesystem unknown"), "{out}");
}
