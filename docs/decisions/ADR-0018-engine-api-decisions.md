# ADR-0018: The Engine Reports; the CLI Renders

* **Status:** Accepted
* **Date:** 2026-09-25
* **Decision owners:** Taphonomy project
* **Scope:** Where the analysis of a disk image runs, and what it hands to
  an interface
* **Related:** `docs/ARCHITECTURE.md` Invariant 8, §20, §26, §27, §47;
  `docs/development/KNOWN_ISSUES.md`, *Domain logic lives in the CLI
  binary*; `ADR-0017`

---

## 1. Context

The library parses, enumerates, assesses and extracts. The sequence that
joins those steps into a run does not live there. The partition loop, the
directory walk, the orphan search, the recovery loop and the run's counts
are in `src/main.rs`, interleaved with `println!`. Invariant 8 says the CLI
must not contain domain recovery logic, and §47 records that it does.

Two consequences follow. From outside the binary, the walk and the search
can be tested only by running it, and each run hashes a 64 MB image first.
And §27 requires that a desktop interface call the same layer the CLI
calls; today no such layer exists, so any second interface would repeat
the walk.

§20 leaves the result schema to be defined after implementation research.
This decision defines it.

---

## 2. Decisions

* **A.** The run moves into the library. `src/main.rs` keeps argument
  handling, the checks on an output directory it makes before the library
  is called, rendering, and the exit status.
* **B.** The library reports a run as an ordered stream of events, one per
  finding, delivered to a sink the caller supplies. The CLI's sink prints.
* **C.** The run's counts and its coverage move with it, and are returned
  when the run ends.
* **D.** Hashing the image stays outside the run. The run is given the
  image's length in bytes.
* **E.** The move changes no output. Its test is byte equality.

---

## 3. Why a stream, not a finished tree

A tree returned at the end would serve the CLI and a test equally well,
but not an interface. EXP-0008 timed about ten seconds on a 1 GiB image,
hashing included. A 32 GB card holds thirty-two times as much, and an
interface that shows nothing until the end is one an operator abandons. A
stream also keeps the CLI's output in the order it has today with no
reordering logic, which is what makes Decision E testable. A sink that
collects events into a tree is a few lines, and tests use one.

Every line the CLI prints today is rendered from an event's fields. No
event carries preformatted text.

---

## 4. Decision D: the image's length

`inspect` gives the partition parser the image's length as hashing
measured it, and the parser reports a partition that claims sectors past
that length. The run takes the length as an argument. The CLI passes the
hashed length, as now. A test that needs no digest passes the file's
reported size and skips the whole-image read, which is where its time
goes.

---

## 5. Decision E: how the move is proved

Before the move, the binary at the base commit is run on every fixture and
on the three CFReDS images of EXP-0008, with no options, with `--recover`,
and with `--recover --output` into the same empty directory. Standard
output, standard error, exit status and every written file are hashed.
After the move, the same runs must produce the same digests, every one.

Also required:

1. The test suite passes with no test file changed in the same commit.
2. `src/main.rs` calls none of `enumerate_root`, `enumerate_directory`,
   `enumerate_deleted_directory`, `begins_directory`, `assess_listed` or
   `extract`.
3. `Cargo.lock` is unchanged.

Once proved, the tests that run the binary only to reach the walk or the
search are rewritten against the library, in their own commit.

---

## 6. Not decided here

* **Machine-readable output.** JSON needs either a dependency or a
  hand-written encoder, and that choice is its own.
* **Extension points for private components.** A trait is added when its
  first implementation exists, not before.
* **Which components are published and which are not.** Decided with
  exFAT, whose patent status bears on it.
* **Any change to what a run reports.** This decision moves code only.

---

## Appendix A: a sink can stop the run (2026-11-21)

Decided by the planning session on 2026-11-21, under the
decision-making the author delegated on 2026-11-13 and restated on
2026-11-20.

**Context.** Decision B gave a sink no way to say that it can no longer
record. The CLI's sink printed with `println!`, which panics when
standard output is closed, for example when the reader of a pipe stops
early (`KNOWN_ISSUES.md`, found 2026-11-19). Tried on 2026-11-20 with
the release build of v0.2.0: a reader that stopped in the middle of a
recovery left every file recovered so far on disk, complete, while the
lines reporting them never reached the reader, and the process ended in
a panic with an exit status no document defines. A sink that swallowed
the error instead would let the run go on writing files that no report
lists. `SAFETY.md` section 12: Taphonomy must fail closed, and "a
failure must never be silently converted into a partial success."

**Decisions.**

* **F.** `Sink` has a second method, `stopped`, which returns `false`
  unless a sink overrides it. Once it returns `true`, the run extracts
  no further file and leaves each of its loops at the next pass. Events
  the run still delivers are the sink's to drop.
* **G.** The CLI writes its report through one writer that keeps the
  first error a write returns, and writes nothing after it. Its sink
  then reports itself stopped (Decision F), and the process exits with
  status 4 and one line on standard error naming the error; when
  `--output` was given, the line adds that files already written there
  may be missing from the report. A write to standard error that fails
  is not reported: nothing is left to report it to, and the exit status
  still is.

The failure is seen at the first write after the reader has gone, and a
run can be ahead of its reader by what a pipe holds. A file written in
between is on disk while the line reporting it never reached the
reader; the line on standard error says so, and `KNOWN_ISSUES.md`
records it.

**Why 4.** Statuses 0 to 3 are taken (`README.md`; `ADR-0014`
Appendix B.10 added 3). ripgrep exits 0 when its reader stops, as a
graceful end (`crates/core/main.rs`, read 2026-11-20). For a search
that may be right; for this tool a run that stopped before it finished
has not done what was asked, and calling it success is the conversion
section 12 forbids, the reasoning `ADR-0014` Appendix B.10 applied to
status 3. 141 is what a shell reports for a process killed by `SIGPIPE`
(128 plus the signal's number). This process is not killed: Rust's
standard library ignores `SIGPIPE` before `main`, so a write fails with
an error instead. Restoring the signal needs `unsafe` code, which the
crate denies, or a new dependency; the compiler's `-Zon-broken-pipe`
flag is unstable.

**What does not change.** While the reader reads to the end, every
byte of output is the same. `scripts/identity-proof.sh`, section 5's
method as a script, ran the build before this change and the build
after it over the 31 fixtures, the three CFReDS images of EXP-0008 and
EXP-0009's A, B and zeroed A, in three modes each, from the same
working directory, and the two tables are equal byte for byte. A file
already written when the report fails stays where it is, and is whole:
an extraction finishes, or abandons its file, before the run records
its event, so no event is recorded while a file is open. That rests on
the code's order, not on a test.

**Held by tests** (`tests/closed_output.rs`). Three run the binary:
`a_closed_standard_output_stops_the_run_with_status_4`,
`a_closed_standard_output_writes_no_recovered_file` and
`a_full_standard_output_stops_the_run_with_status_4`. The rest run the
library with a sink that stops at a chosen event, each beside a sink
that never stops, to show that what the stopped one is denied exists:
`a_stopped_sink_gets_no_file_written`,
`a_stopped_sink_gets_no_exfat_file_written`,
`a_stopped_sink_is_handed_no_partition`,
`a_sink_stopped_by_a_recoverable_finding_gets_no_file_and_no_next_finding`,
`a_stopped_sink_is_handed_no_further_directory_of_the_walk`,
`a_stopped_sink_is_handed_no_further_orphaned_directory`,
`a_sink_stopped_by_a_recoverable_exfat_set_gets_no_file`,
`a_stopped_sink_is_handed_no_further_exfat_directory`,
`a_stopped_sink_is_handed_no_further_exfat_decision` and
`a_stopped_sink_reads_no_exfat_phase_two`. Removing any one check, or
the status, fails at least one of them. That the writer writes nothing
after its first error is argued, not tested: after a closed pipe or a
full device every later write fails as well.
