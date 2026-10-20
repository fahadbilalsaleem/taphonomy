//! Live exFAT files' content, read through the library and compared with
//! what other tools say each file holds.
//!
//! The volumes are EXP-0009's, expanded by `./scripts/generate-fixtures.sh`.
//! The expected tables, `fixtures/exfat/exp9-*.expected.tsv`, come from The
//! Sleuth Kit and the EXP-0009 generator and never from Taphonomy: each row
//! is a path, a size in bytes and a SHA-256 of the content.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use taphonomy::EvidenceFile;
use taphonomy::exfat_boot::parse_boot_region;
use taphonomy::exfat_directory::{
    DirItem, DirectorySource, ExFatFile, ExFatReader, Limits, escaped_name,
};
use taphonomy::filesystem::VolumeExtent;
use taphonomy::hash::Sha256Hasher;
use taphonomy::partition::{PartitionTable, SECTOR_SIZE, parse_mbr};

/// The four targets' digests as EXPERIMENTS.md records them for EXP-0009.
const KEEP_TXT: &str = "799230d4c57749efdd2c1bf80a663062bc7323a47869e52c7122b82068a95e2e";
const CONTIG_BIN: &str = "afbc355c38682d3c1e46755da3a681fdc931129c6fc6fed3ba47eb790bd87f17";
const SMALL_TXT: &str = "ac61dcd74380dc95f3ca6ffbfc34725b9c47c67378c5a51282df2311da0e9f77";
const FRAG_BIN: &str = "446a6a9ca69120765f8f541b75790a9402dd4f61da7df2e1c9d27cd487ba259f";

/// One live file as the library read it.
struct Read {
    data_length: u64,
    no_fat_chain: bool,
    /// Bytes handed over and their digest, or why the content was not read.
    content: Result<(u64, String), String>,
}

/// One row of an expected table.
struct Expected {
    size: u64,
    digest: String,
}

fn fixture(path: &str) -> PathBuf {
    let path = Path::new("fixtures").join(path);
    assert!(
        path.exists(),
        "fixture missing: {}\nrun ./scripts/generate-fixtures.sh first",
        path.display()
    );
    path
}

/// Every live file that is not a directory on the image's one partition,
/// by path, its content read and hashed, and every directory that could not
/// be read and every path met twice.
fn read_live_files(image: &Path) -> (BTreeMap<String, Read>, Vec<String>) {
    let mut problems = Vec::new();
    let mut evidence = EvidenceFile::open(image).expect("opening the image");
    let mut sector = [0u8; SECTOR_SIZE];
    evidence
        .read_exact_at(0, &mut sector)
        .expect("reading the partition table");
    let image_sectors = evidence.reported_size() / SECTOR_SIZE as u64;
    let outcome = parse_mbr(&sector, image_sectors).expect("parsing the partition table");
    let PartitionTable::Mbr { partitions, .. } = outcome.table else {
        panic!("expected an MBR partition table");
    };
    assert_eq!(partitions.len(), 1, "one partition");
    let extent = VolumeExtent {
        start_lba: partitions[0].start_lba,
        sector_count: partitions[0].sector_count,
    };
    let boot = parse_boot_region(&mut evidence, extent).expect("a valid boot region");
    let mut reader = ExFatReader::new(&mut evidence, &boot, extent, Limits::DEFAULT);

    // Every directory from the root, breadth first. Files are collected and
    // read afterwards, because a walk holds the reader.
    let mut files: Vec<(String, ExFatFile)> = Vec::new();
    let mut pending = VecDeque::from([(
        String::new(),
        DirectorySource::Root {
            first_cluster: boot.root_cluster,
        },
    )]);
    while let Some((path, source)) = pending.pop_front() {
        let walked = reader.read_directory(source, &mut |item| {
            let DirItem::File { file, .. } = item else {
                return;
            };
            let name = escaped_name(&file.name);
            let child = if path.is_empty() {
                name
            } else {
                format!("{path}/{name}")
            };
            if file.is_directory() {
                pending.push_back((
                    child,
                    DirectorySource::Subdirectory {
                        first_cluster: file.first_cluster,
                        data_length: file.data_length,
                        no_fat_chain: file.no_fat_chain,
                    },
                ));
            } else {
                files.push((child, file.clone()));
            }
        });
        if let Err(e) = walked {
            problems.push(format!("directory {path:?} not read: {e}"));
        }
    }

    let mut read = BTreeMap::new();
    for (path, file) in files {
        let mut hasher = Sha256Hasher::new();
        let content = reader
            .read_content(&file, &mut |chunk| hasher.update(chunk))
            .map(|()| {
                let hashed = hasher.finish();
                (hashed.bytes_read, hashed.digest.to_hex())
            })
            .map_err(|e| e.to_string());
        let previous = read.insert(
            path.clone(),
            Read {
                data_length: file.data_length,
                no_fat_chain: file.no_fat_chain,
                content,
            },
        );
        if previous.is_some() {
            problems.push(format!("{path}: two live files at this path"));
        }
    }
    (read, problems)
}

/// An expected table, by path, its `#` lines skipped.
fn expected_table(path: &Path) -> BTreeMap<String, Expected> {
    let text = fs::read_to_string(path).expect("reading the expected table");
    let mut table = BTreeMap::new();
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let fields: Vec<&str> = line.split('\t').collect();
        let [path, size, digest] = fields[..] else {
            panic!("not three tab-separated fields: {line:?}");
        };
        let size = size
            .parse()
            .unwrap_or_else(|e| panic!("size in {line:?}: {e}"));
        let row = Expected {
            size,
            digest: digest.to_string(),
        };
        assert!(
            table.insert(path.to_string(), row).is_none(),
            "{path} listed twice"
        );
    }
    table
}

/// Every way the files read differ from the table: none means they agree.
fn differences(
    read: &BTreeMap<String, Read>,
    expected: &BTreeMap<String, Expected>,
) -> Vec<String> {
    let mut found = Vec::new();

    for path in expected.keys().filter(|p| !read.contains_key(*p)) {
        found.push(format!("{path}: in the table, not read"));
    }
    for path in read.keys().filter(|p| !expected.contains_key(*p)) {
        found.push(format!("{path}: read, not in the table"));
    }

    for (path, file) in read {
        let Some(row) = expected.get(path) else {
            continue;
        };
        if file.data_length != row.size {
            found.push(format!(
                "{path}: data length {} where the table has {}",
                file.data_length, row.size
            ));
        }
        match &file.content {
            Err(e) => found.push(format!("{path}: content not read: {e}")),
            Ok((bytes, digest)) => {
                if *bytes != file.data_length {
                    found.push(format!(
                        "{path}: {bytes} bytes handed over for a data length of {}",
                        file.data_length
                    ));
                }
                if *digest != row.digest {
                    found.push(format!(
                        "{path}: sha256 {digest} where the table has {}",
                        row.digest
                    ));
                }
            }
        }
    }

    found
}

fn assert_matches_table(image: &str, table: &str) -> BTreeMap<String, Read> {
    let (read, mut found) = read_live_files(&fixture(image));
    let expected = expected_table(&fixture(table));
    found.extend(differences(&read, &expected));
    assert!(
        found.is_empty(),
        "{} differences from {table}:\n{}",
        found.len(),
        found.join("\n")
    );
    read
}

#[test]
fn every_live_file_on_image_a_has_the_content_the_table_records() {
    let read = assert_matches_table("partition/exp9-A.vhd", "exfat/exp9-A.expected.tsv");

    for (path, digest) in [
        ("KEEP.txt", KEEP_TXT),
        ("CONTIG.bin", CONTIG_BIN),
        ("SMALL.txt", SMALL_TXT),
        ("FRAG.bin", FRAG_BIN),
    ] {
        let content = read.get(path).map(|file| &file.content);
        assert!(
            matches!(content, Some(Ok((_, d))) if d == digest),
            "{path}: {:?}",
            content
        );
    }

    // The two ways content is laid out are each exercised.
    assert!(
        !read["FRAG.bin"].no_fat_chain,
        "FRAG.bin is not a FAT chain"
    );
    assert!(read["CONTIG.bin"].no_fat_chain, "CONTIG.bin is not a run");
}

#[test]
fn every_live_file_on_image_b_has_the_content_the_table_records() {
    assert_matches_table("partition/exp9-B.vhd", "exfat/exp9-B.expected.tsv");
}
