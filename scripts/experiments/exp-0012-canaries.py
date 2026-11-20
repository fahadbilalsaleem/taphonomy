#!/usr/bin/env python3
"""Places EXP-0012's canaries in a throwaway copy of the crate. Never run
on the repository.

Usage: exp-0012-canaries.py TREE MAP_OUT

One canary before every `return Err(` / `return Ok(Err(` line outside
`#[cfg(test)]` in the parser modules, and before each condition-2 verdict
assignment in `src/exfat_claims.rs`. Writes `src/fuzz_canary.rs`, declares
it in `src/lib.rs`, and writes MAP_OUT: id, file, line in the unmodified
file, the line's text. Stops if TREE looks like the repository.
"""
import os
import re
import sys

MODULES = ["src/partition.rs", "src/fat32.rs", "src/fat_directory.rs",
           "src/fat_recovery.rs", "src/exfat_boot.rs",
           "src/exfat_directory.rs"]
REFUSAL = re.compile(r"^(\s*)return (Ok\()?Err\(")
VERDICT = re.compile(r"^(\s*)verdicts\[\w+\] = Some\(ConditionTwo::(Unordered|Later)\);")

CANARY_MODULE = '''//! EXP-0012 canaries (trial build only, never landed).
//!
//! `hit` records, once per process, a canary's id with the harness's
//! execution count and the seconds since the first execution, on stderr.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

pub const COUNT: usize = @COUNT@;

/// Executions so far; the harness adds one per input.
pub static EXECS: AtomicU64 = AtomicU64::new(0);

static START: OnceLock<Instant> = OnceLock::new();
static SEEN: [AtomicBool; COUNT] = [const { AtomicBool::new(false) }; COUNT];

/// Called by the harness before each input.
pub fn exec() {
    START.get_or_init(Instant::now);
    EXECS.fetch_add(1, Ordering::Relaxed);
}

pub fn hit(id: usize) {
    if !SEEN[id].swap(true, Ordering::Relaxed) {
        let secs = START.get().map_or(0.0, |s| s.elapsed().as_secs_f64());
        eprintln!(
            "CANARY {id} exec {} secs {secs:.3}",
            EXECS.load(Ordering::Relaxed)
        );
    }
}
'''


def test_start(lines):
    for i, line in enumerate(lines):
        if line.startswith("#[cfg(test)]"):
            return i
    return len(lines)


def main():
    tree, map_out = sys.argv[1], sys.argv[2]
    if os.path.isdir(os.path.join(tree, ".git")):
        sys.exit("refusing: TREE has a .git directory")
    entries = []
    plan = [(m, REFUSAL) for m in MODULES] + [("src/exfat_claims.rs", VERDICT)]
    for path, pattern in plan:
        full = os.path.join(tree, path)
        lines = open(full, encoding="utf-8").read().split("\n")
        end = test_start(lines)
        out = []
        for i, line in enumerate(lines):
            m = pattern.match(line) if i < end else None
            if m:
                cid = len(entries)
                entries.append((cid, path, i + 1, line.strip()))
                out.append(f"{m.group(1)}crate::fuzz_canary::hit({cid});")
            out.append(line)
        open(full, "w", encoding="utf-8").write("\n".join(out))
    lib = os.path.join(tree, "src/lib.rs")
    text = open(lib, encoding="utf-8").read()
    anchor = "pub mod fat_recovery;\n"
    if text.count(anchor) != 1:
        sys.exit("lib.rs anchor not found once")
    open(lib, "w", encoding="utf-8").write(
        text.replace(anchor, anchor + "pub mod fuzz_canary;\n"))
    open(os.path.join(tree, "src/fuzz_canary.rs"), "w", encoding="utf-8").write(
        CANARY_MODULE.replace("@COUNT@", str(len(entries))))
    with open(map_out, "w", encoding="utf-8") as f:
        for cid, path, line, text in entries:
            f.write(f"{cid}\t{path}\t{line}\t{text}\n")
    print(f"{len(entries)} canaries")


main()
