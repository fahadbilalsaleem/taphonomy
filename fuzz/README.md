# Fuzz targets

Two targets, from EXP-0012 (`docs/development/EXPERIMENTS.md`). Nothing
here is part of the release binary; this directory has its own
`[workspace]` and `Cargo.lock`, so the crate's `Cargo.lock` and
`cargo test --workspace` never see it.

- `analyse_image`: runs `analysis::run` over a sparse image built from the
  input, with no output. FAT32 and exFAT volumes inside an MBR.
- `exfat_reader`: drives the exFAT reader directly (live tree, bitmap
  check, claims) over each partition and over the whole image as one
  volume.

The input format and the checksum-resealing mutator are in
`fuzz_targets/sparse.rs`. Not covered: FAT12 and FAT16, GPT and NTFS (the
crate does not analyse them) and the CLI's rendering in `src/main.rs`.

## Seeds

    scripts/fuzz-seeds.sh fixtures/partition SEEDS [MADE]

One seed for each fixture except the two `fat32-fragmented-*` (about 66 MB
each, over the 131,072-byte input limit). `MADE`, the six volumes written
by `scripts/experiments/exp-0011-claims-images.sh`, adds each bare and
wrapped in an MBR (`scripts/experiments/mbr-wrap-sparse.py`), since the analysis reads a
volume only inside a partition table. `SEEDS` must not exist.

## Replay (what CI runs)

    cargo build --release --locked --manifest-path fuzz/Cargo.toml \
        --config profile.release.debug-assertions=true \
        --config profile.release.overflow-checks=true
    for t in analyse_image exfat_reader; do
        fuzz/target/release/$t SEEDS/*.seed fuzz/regressions/*.seed
    done

Plain cargo, no nightly. Pass every input as its own file: given a
directory with `-runs=0` and no `-max_len`, libFuzzer cuts each input to
4 bytes and passes without testing anything. Any crash exits non-zero.

## A campaign (local only; CI runs none)

    cargo install --locked cargo-fuzz        # version 0.13.2 was used
    cargo fuzz run -s none analyse_image SEEDS -- -max_len=131072 \
        -rss_limit_mb=700 -malloc_limit_mb=700

`-s none` builds with the pinned stable toolchain, no sanitizer. Run
`exfat_reader` the same way. Keep a crash input that is fixed in
`regressions/`, named for the target and the refusal it reaches.
