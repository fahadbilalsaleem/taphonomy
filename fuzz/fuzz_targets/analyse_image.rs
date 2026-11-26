//! Runs the whole analysis over a sparse image built from the fuzz input.
//!
//! Nothing is ever written: `output` is always `None`. The analysis reads
//! a volume only inside a partition table, so an exFAT volume reaches its
//! bitmap check and claims code (`src/analysis/exfat.rs`) only from a seed
//! wrapped in an MBR (`scripts/experiments/mbr-wrap-sparse.py`); `exfat_reader` also
//! tries a volume with no table.

#![no_main]

#[path = "sparse.rs"]
mod sparse;

use libfuzzer_sys::{fuzz_mutator, fuzz_target};
use sparse::SparseImage;
use taphonomy::analysis::{self, Event, Options, Sink};

struct Discard;

impl Sink for Discard {
    fn record(&mut self, _event: Event<'_>) {}
}

// Half the mutations keep valid exFAT checksums; see `sparse.rs`.
fuzz_mutator!(|data: &mut [u8], size: usize, max_size: usize, seed: u32| {
    let size = libfuzzer_sys::fuzzer_mutate(data, size, max_size);
    sparse::fix_up(&mut data[..size], seed);
    size
});

fuzz_target!(|data: &[u8]| {
    let Some(mut image) = SparseImage::parse(data) else {
        return;
    };
    let len = image.len;
    let options = Options {
        recover: image.flags & 1 == 1,
        reference: None,
        output: None,
    };
    let _ = analysis::run(&mut image, len, options, &mut Discard);
});
