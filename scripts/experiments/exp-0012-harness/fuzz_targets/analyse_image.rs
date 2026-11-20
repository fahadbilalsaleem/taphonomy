//! Runs the whole analysis over a sparse image built from the fuzz input.
//!
//! Nothing is ever written: `output` is always `None`. At `d7ad2a0` the
//! analysis calls neither the bitmap check nor the claims code;
//! `exfat_reader` reaches those.

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
    taphonomy::fuzz_canary::exec();
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
