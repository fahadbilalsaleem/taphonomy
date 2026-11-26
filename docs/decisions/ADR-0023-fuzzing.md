# ADR-0023: The Parsers Are Fuzzed With cargo-fuzz, and CI Replays the Seeds

* **Status:** Accepted
* **Date:** 2026-11-21
* **Decision owners:** the author (decision of 2026-11-03); the details
  by the planning session under the decision-making the author
  delegated on 2026-11-13 and restated on 2026-11-20
* **Scope:** How Taphonomy's parsers are fuzzed: the tool, the harness,
  the seeds, the regression inputs, and what CI runs
* **Related:** `ADR-0001` sections 3.1 and 13; `SECURITY.md` sections 35
  and 36; EXP-0012; `KNOWN_ISSUES.md`; `ADR-0024`

---

## 1. Context

`SECURITY.md` section 36 asks for fuzzing of the binary parsers to find
crashes, panics, hangs and memory exhaustion, and `ADR-0001` section 13
says it will be introduced. Until now none existed.

On 2026-11-03 the author decided to adopt cargo-fuzz in `fuzz/`,
recorded by an ADR, with the fuzz crate's own `Cargo.lock` committed,
seeds made by a script, and a replay in CI, where research and tests
here back it. EXP-0012 then measured a harness against the code at
`d7ad2a0`, as the published evaluations measure fuzzers: 52 trials, no
crash and no hang, and a mutator that reseals exFAT checksums
triggering more refusal sites than libFuzzer alone in every paired
trial. It also found that the analysis target never reached the exFAT
directory reader, because every exFAT seed was a volume with no
partition table, which the analysis did not read.

## 2. Decisions

* **A. cargo-fuzz and libFuzzer, on the pinned stable toolchain.**
  `fuzz/` holds the EXP-0012 harness without its canaries: two targets,
  `analyse_image` (the whole analysis, writing nothing) and
  `exfat_reader` (the exFAT reader's walk, bitmap check and claims), a
  sparse input format capped at 2^18 sectors, and the mutator that
  reseals exFAT set and boot checksums on half the mutations. It is its
  own workspace with its own `Cargo.lock`, so the crate's lock file, its
  tests and its release binary do not change. That lock is EXP-0012's
  harness lock with one line changed, the crate's own version, from
  0.1.0 to 0.2.0: `--locked` refuses a lock that names a version the
  crate no longer has.
* **B. Seeds are made by `scripts/fuzz-seeds.sh`**, from the generated
  fixtures (the volume with no partition table of `ADR-0024` among
  them) and, when given, the six volumes of EXP-0011 Appendix C, each
  both bare and wrapped in an MBR by
  `scripts/experiments/mbr-wrap-sparse.py`, so the analysis target
  reaches exFAT through a partition as well as alone.
* **C. Regression inputs are committed** in `fuzz/regressions/`: the
  three inputs of EXP-0012's screen (b) that each reach a refusal no
  trial reached (#17, #36, #58), and the input that showed #92 cannot
  be reached, which reaches the bad-cluster refusal #98 before it.
* **D. CI replays, it does not fuzz.** A CI job builds `fuzz/` with
  plain cargo, debug assertions and overflow checks on, and runs each
  target once on every seed it can make from the fixtures and on every
  regression input, each passed as a file, failing on any non-zero exit.
* **E. Campaigns are local**, run at night by the author's rule of
  2026-11-19, with the harness's own scripts.

## 3. Alternatives

From the fuzzing audit of 2026-10-29: afl.rs (needs system
configuration with administrator rights and builds AFL++), honggfuzz-rs,
bolero (a front end over several engines, with more code pulled in),
proptest (property testing, not coverage-guided), and extending the
existing deterministic never-panic tests (blind to coverage). cargo-fuzz
was tried on 2026-11-03 and builds both targets on the pinned stable
toolchain with `-s none`, no nightly; the others were not tried.

## 4. Licences (files read 2026-11-21)

* `libfuzzer-sys` 0.4.13: `license = "(MIT OR Apache-2.0) AND NCSA"` in
  its manifest; each of the 47 libFuzzer source files it vendors carries
  `SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception`. Its build
  dependencies `arbitrary`, `cc`, `jobserver`, `shlex` and
  `find-msvc-tools`: `MIT OR Apache-2.0` each.
* The FSF's licence list: the Apache License 2.0 is "compatible with
  version 3 of the GNU GPL"; the NCSA licence and the Expat licence are
  "compatible with the GNU GPL". The list has no entry for the LLVM
  exception. The exception (`llvm.org/LICENSE.txt`, "LLVM Exceptions to
  the Apache 2.0 License") only adds permissions; that it changes
  nothing for GPL-3.0-or-later is reasoning, not a published statement.
* Nothing of the fuzz crate is in Taphonomy's release binary; it is a
  development tool.

## 5. Consequences

* Every change to a parser can be replayed against the seeds and the
  regression inputs in CI. A crash input found by a campaign is added
  to `fuzz/regressions/` with the fix that makes it pass.
* CI gains a job that compiles libFuzzer's C++ sources with the
  runner's compiler.
* The night schedule's campaigns use these targets, not EXP-0012's
  canary build, from the next campaign on.
* `KNOWN_ISSUES.md`'s entry "No fuzz testing exists" is rewritten to
  what remains.

## 6. Held by

The CI job `Fuzz targets replay their seeds` runs on every push. It was
shown able to fail on 2026-11-21 with two panics planted in a scratch
copy of the code: one behind the TexFAT refusal, one behind the FAT32
directory chain's bound. The replay of the seeds alone passed with
both; with the regression inputs it exited non-zero on
`exfat_reader-36-texfat.seed` and on
`analyse_image-17-fat-directory-chain-bound.seed`. The other two
regression inputs were not shown to fail a planted panic.

## 7. Not decided here

* A campaign in CI, or a scheduled one in the cloud.
* Targets for the command line's rendering, for GPT, FAT12, FAT16 or
  NTFS: the crate analyses none of them yet, except the rendering, which
  is reached by no target.
* Whether to reseal the up-case table's checksum in the mutator
  (EXP-0012, threat to validity).
