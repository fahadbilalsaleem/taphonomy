# Taphonomy

[![CI](https://github.com/fahadbilalsaleem/taphonomy/actions/workflows/ci.yml/badge.svg)](https://github.com/fahadbilalsaleem/taphonomy/actions/workflows/ci.yml)

Taphonomy recovers deleted files, and files whose directories were lost to
a quick format, from FAT32 disk images. It opens evidence read-only, reports
what it could not analyse, and refuses a recovery it cannot support rather
than guessing. It is written in Rust.

Every behaviour below is recorded in a decision record and measured in an
experiment record in this repository. Where a claim rests on a measurement,
the measurement is linked.

## What it does today

* Opens a raw disk image read-only and hashes every byte with SHA-256 before
  reporting anything.
* Parses the MBR partition table and validates every declared extent
  against the image's true size. GPT is detected and reported as
  unsupported.
* Identifies each partition's filesystem from its structure rather than its
  label: FAT12, FAT16 and FAT32 by cluster count, exFAT and NTFS by
  signature. A partition type that disagrees with the filesystem on it is
  reported as a finding.
* Reads every FAT32 directory the root reaches: a live one along its
  cluster chain, a deleted one from its first cluster
  ([ADR-0016](docs/decisions/ADR-0016-m11-subdirectory-traversal-decisions.md)).
* Finds directories that nothing names, such as the tree a quick format
  leaves behind, and offers the files they list for recovery
  ([ADR-0017](docs/decisions/ADR-0017-m12-orphaned-directory-decisions.md),
  [EXP-0007](docs/development/EXPERIMENTS.md)).
* Recovers an unfragmented file whose clusters are all free, and refuses
  one whose implied run reaches a cluster in use, naming the cluster
  ([ADR-0010](docs/decisions/ADR-0010-m7-implementation-decisions.md)).
* Writes each recovered file, reads it back and reports whether what landed
  matches what was read
  ([ADR-0015](docs/decisions/ADR-0015-m10-recovery-output-decisions.md)).
* Compares a recovery against a digest you supply, and never looks one up
  ([ADR-0013](docs/decisions/ADR-0013-m8-reference-validation-decisions.md)).
* States its own coverage: every partition, directory or listing it did not
  analyse is counted and named
  ([ADR-0014](docs/decisions/ADR-0014-m9-classification-decisions.md)).

## Evidence that it works

[EXP-0008](docs/development/EXPERIMENTS.md) ran Taphonomy against three of
NIST's [CFReDS](https://cfreds-archive.nist.gov/dfr-test-images.html)
deleted-file-recovery test images and against The Sleuth Kit 4.12.1. NIST
documents the sectors every deleted file occupied, so each recovery was
checked against the image itself rather than against either tool.

| Image | Case | Taphonomy | The Sleuth Kit |
| --- | --- | --- | --- |
| `dfr-01-fat` | one unfragmented file | recovered; equals NIST's sectors | identical bytes |
| `dfr-02-fat` | a file fragmented around a live file | **refused**: the implied run crosses the live file | recovered correctly |
| `dfr-11-fat` | a deleted directory of three files | all three recovered; each equals NIST's sectors | identical bytes |

Every file Taphonomy recovered is the file that was deleted. On `dfr-02`
the run its entry implies is half another, still-existing file; Taphonomy
refused it where The Sleuth Kit's rule for passing over allocated clusters
happened to reach the right one. Across the three images Taphonomy reached
four of the fifteen deleted files and refused that one. The other ten sit
on the FAT12 and FAT16 partitions each image carries, which it identifies
and does not yet analyse.

## Quick start

Taphonomy is developed on Ubuntu 24.04 under WSL2. The toolchain is pinned
in `rust-toolchain.toml`, so [rustup](https://rustup.rs) installs the right
one on first build. The test fixtures are built from ordinary filesystem
tools:

```sh
sudo apt install fdisk gdisk dosfstools mtools xz-utils
git clone https://github.com/fahadbilalsaleem/taphonomy
cd taphonomy
cargo build --release
./scripts/generate-fixtures.sh
./target/release/taphonomy fixtures/partition/fat32-formatted-tree.img --recover
```

That fixture is a card whose `DCIM/100TAPH` folder was lost to a quick
format. Part of what the run reports:

```text
    orphaned directory c4, .. names c3
      c4 s2     file          IMG_0001.JPG  13 bytes, cluster 5
      c4 s3     file          IMG_0002.JPG  13 bytes, cluster 6
      c4 s4     file          IMG_0003.JPG  1300 bytes, cluster 7
      orphaned content
        c4 s2     run 5-5, 13 bytes, 499 slack, every cluster free
                  recovered sha256 86a773f90bcf220bea99ad333fdc1476de79daddba2ea7e017de7f302e394417 over 13 bytes
        ...

coverage     complete
  volumes analysed           1
artifacts    3 RECONSTRUCTED

A free run means nothing has claimed those
clusters since the entry lost them. It is not
evidence that the content there is this file's.
No reference supplied. Nothing was validated.
```

`./scripts/verify-fixtures.sh` builds every fixture twice and confirms they
are byte-identical; `cargo test --workspace --no-fail-fast` runs the suite
against them.

## Usage

```text
taphonomy <evidence-image> [--recover] [--output <directory>] [--reference-digest <hex>]
```

| Option | Effect |
| --- | --- |
| *(none)* | Report what the evidence states. No file content is read. |
| `--recover` | Read and hash the content of each eligible file. |
| `--output <directory>` | Also write each recovered file there, then read it back and verify it. The directory may not hold the evidence. |
| `--reference-digest <hex>` | Compare each recovery against the SHA-256 of the file you are looking for. Requires `--recover`. |

| Exit status | Meaning |
| --- | --- |
| 0 | The evidence was analysed, completely or in part |
| 1 | The evidence could not be opened or hashed |
| 2 | An argument error; no evidence was opened |
| 3 | Nothing past the evidence digest was analysed |

A gap in coverage does not by itself change the status: it is reported on
standard output. A digest that differs from the reference is a finding
about the evidence and also exits 0.

## Scope and limitations

Taphonomy is early, and deliberately narrow.

* Only FAT32 is analysed. FAT12 and FAT16 are identified and not read;
  exFAT and NTFS are identified and reported as unsupported.
* It reads disk images, not physical devices, and only MBR partition
  tables with 512-byte sectors.
* Recovery assumes a file's clusters were contiguous. Deletion erases the
  cluster chain, so for a fragmented file whose clusters have since been
  freed, the run its entry implies can hold content that is not the file's.
  Taphonomy cannot detect that case, and every recovery carries a caveat
  saying so. Only a reference digest establishes that content is the
  file's.
* A deleted directory is read from its first cluster only; a listing that
  may continue past it is reported as a gap.
* A directory nothing names is found only while its first cluster still
  identifies itself. On a card written to after a format, none does.
* Long file names are not assembled: every name is shown in its 8.3 form.

The full list, each with the measurement behind it, is in
[KNOWN_ISSUES.md](docs/development/KNOWN_ISSUES.md).

## How it is built

A recovery result that appears plausible but is incorrect is treated as a
failure, not a partial success. The project is organised around that.

* **Decisions before code.** Every capability starts as an Architecture
  Decision Record in [docs/decisions](docs/decisions), stating what will be
  built, what will not, and the measured conditions it must meet.
* **Measurements before decisions.** Experiments in
  [EXPERIMENTS.md](docs/development/EXPERIMENTS.md) record a hypothesis
  before the measurement, the method, and the result, including the
  predictions that were wrong.
* **Reproducible evidence.** Every test image is generated from ordinary
  filesystem tools by `scripts/generate-fixtures.sh`, byte-identical on
  every build, with its digest committed in
  `fixtures/partition/MANIFEST.sha256`. The exception is two exFAT
  images that Windows made, which no tool here can regenerate: they are
  committed compressed in `fixtures/exfat/`, expanded by the same script
  and checked against the digests EXP-0009 recorded. They hold only
  synthetic files. No real personal evidence is ever committed.
* **Tested boundaries.** Read-only access, bounds checks on what is
  parsed, each refusal, and every kind of coverage gap have tests.

Evidence preservation, correctness, security and reproducibility come
first; performance and breadth come after. The project's contracts are in
[PROJECT.md](docs/PROJECT.md), [ARCHITECTURE.md](docs/ARCHITECTURE.md),
[SAFETY.md](docs/SAFETY.md) and [SECURITY.md](SECURITY.md); the build
environment in
[DEVELOPMENT_ENVIRONMENT.md](docs/development/DEVELOPMENT_ENVIRONMENT.md);
research in [RESEARCH_LOG.md](docs/development/RESEARCH_LOG.md); changes in
[CHANGELOG.md](docs/development/CHANGELOG.md).

## Planned

Not yet implemented, in this order:

1. exFAT, the filesystem of most cards larger than 32 GB. It moved first
   after EXP-0009 measured what Windows 10 leaves when it deletes from
   exFAT: the entry set, and even a fragmented file's chain, survive,
   which FAT32 never allows. ADR-0019 sets its scope and the test it must
   pass.
2. FAT12 and FAT16, where ten of the fifteen deleted files in EXP-0008
   were out of reach.
3. Long file names.

Each has, or will get, its own decision record, with the measurements it
must meet, before any code.

## Contributing

External code contributions are not accepted; see
[CONTRIBUTING.md](CONTRIBUTING.md) for why.

## License

Copyright (c) 2026 Fahad Bilal Saleem.

Taphonomy is distributed under the GNU General Public License, version 3 or
later. See [LICENSE](LICENSE).
