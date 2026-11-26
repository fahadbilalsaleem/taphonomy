//! Drives the exFAT reader over each partition of a sparse image, in the
//! order the analysis uses it (`src/analysis/exfat.rs`): the live tree,
//! then the bitmap opened and checked once, then condition 2 over the
//! claims.
//!
//! The whole image is also tried as one volume at sector 0: the analysis
//! does not read a volume with no partition table (KNOWN_ISSUES), and the made
//! benchmark volumes are such volumes.

#![no_main]

#[path = "sparse.rs"]
mod sparse;

use std::collections::VecDeque;

use libfuzzer_sys::{fuzz_mutator, fuzz_target};
use sparse::{SECTOR, SparseImage};
use taphonomy::evidence::EvidenceReader;
use taphonomy::exfat_boot::parse_boot_region;
use taphonomy::exfat_directory::{DirItem, DirectorySource, ExFatReader, Limits};
use taphonomy::filesystem::VolumeExtent;
use taphonomy::partition::{PartitionTable, parse_mbr};

// Half the mutations keep valid set and boot checksums, so the parsers
// past the checksums are reached; the other half leave them as mutated.
fuzz_mutator!(|data: &mut [u8], size: usize, max_size: usize, seed: u32| {
    let size = libfuzzer_sys::fuzzer_mutate(data, size, max_size);
    sparse::fix_up(&mut data[..size], seed);
    size
});

fuzz_target!(|data: &[u8]| {
    let Some(mut image) = SparseImage::parse(data) else {
        return;
    };
    let mut sector = [0u8; SECTOR as usize];
    if image.read_exact_at(0, &mut sector).is_err() {
        return;
    }
    let mut extents = vec![VolumeExtent {
        start_lba: 0,
        sector_count: u32::try_from(image.len / SECTOR).unwrap_or(u32::MAX),
    }];
    if let Ok(outcome) = parse_mbr(&sector, image.len / SECTOR)
        && let PartitionTable::Mbr { partitions, .. } = outcome.table
    {
        extents.extend(partitions.iter().map(|p| VolumeExtent {
            start_lba: p.start_lba,
            sector_count: p.sector_count,
        }));
    }

    for extent in extents {
        let Ok(boot) = parse_boot_region(&mut image, extent) else {
            continue;
        };
        let mut reader = ExFatReader::new(&mut image, &boot, extent, Limits::DEFAULT);

        let mut bitmap_entry = None;
        let mut pending = VecDeque::from([DirectorySource::Root {
            first_cluster: boot.root_cluster,
        }]);
        while let Some(source) = pending.pop_front() {
            let _ = reader.read_directory(source, &mut |item| match item {
                DirItem::File { file, .. } if file.is_directory() => {
                    pending.push_back(DirectorySource::Subdirectory {
                        first_cluster: file.first_cluster,
                        data_length: file.data_length,
                        no_fat_chain: file.no_fat_chain,
                    });
                }
                DirItem::Bitmap { bitmap, .. } if bitmap_entry.is_none() => {
                    bitmap_entry = Some(*bitmap);
                }
                _ => {}
            });
        }

        if let Some(entry) = bitmap_entry
            && let Ok(mut bitmap) = reader.open_bitmap(&entry)
        {
            let _ = reader.check_bitmap(&mut bitmap);
        }
        let _ = reader.assess_claims();
    }
});
