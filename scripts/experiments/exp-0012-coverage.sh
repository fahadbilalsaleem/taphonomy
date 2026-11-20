#!/usr/bin/env bash
# EXP-0012 secondary measure: line coverage per file of each trial's final
# corpus (design v3, "Measures").
# Usage: exp-0012-coverage.sh TREE TRIALS OUT
# TREE is the canary tree exp-0012-tree.sh made; TRIALS the trials
# directory exp-0012-run.sh wrote. For each trial, cargo fuzz coverage
# (cargo-fuzz 0.13.2) replays the final corpus on a coverage build of its
# target, and llvm-cov export (the pinned toolchain's llvm-tools) writes the
# per-file summary to OUT/json/<trial>.json; exp-0012-coverage.py then
# writes OUT/coverage.tsv. A failure prints a line starting FAIL.
set -u
[ $# -eq 3 ] || { echo "usage: exp-0012-coverage.sh TREE TRIALS OUT" >&2; exit 2; }
T=$1; trials=$2; C=$3; here=$(cd "$(dirname "$0")" && pwd)
LLVM=$(cd "$T" && rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin
[ -x "$LLVM/llvm-cov" ] || { echo "FAIL no llvm-cov in $LLVM" >&2; exit 2; }
mkdir -p "$C/json" || exit 2
cd "$T/fuzz" || exit 2
for d in "$trials"/*; do
    id=$(basename "$d")
    [ -s "$C/json/$id.json" ] && continue
    target=$(sed -n 's/.*target=\([a-z_]*\).*/\1/p' "$d/META")
    # cargo fuzz coverage writes one raw profile per batch of 100 inputs
    # (at most one per job) and merges every profile in its raw directory,
    # so a corpus with fewer batches than the trial before it would be
    # merged with that trial's leftovers. Each trial starts with the
    # directory empty; the profiles are kept in OUT/raw/<trial>/.
    raw=$T/fuzz/coverage/$target/raw
    if [ -d "$raw" ] && [ -n "$(ls -A "$raw")" ]; then
        mkdir -p "$C/raw/before-$id" && mv -- "$raw"/* "$C/raw/before-$id/" || { echo "FAIL clearing $raw"; exit 1; }
    fi
    if ! cargo fuzz coverage -s none --target-dir "$C/target" "$target" "$d/corpus" > "$C/json/$id.log" 2>&1; then
        echo "FAIL coverage $id"
        continue
    fi
    "$LLVM/llvm-cov" export -summary-only \
        -instr-profile="$T/fuzz/coverage/$target/coverage.profdata" \
        "$C/target/x86_64-unknown-linux-gnu/release/$target" > "$C/json/$id.json" 2> "$C/json/$id.err" ||
        echo "FAIL export $id"
    echo "done $id $target $(ls "$d/corpus" | wc -l) inputs"
done
python3 -I "$here/exp-0012-coverage.py" "$C/json" "$C/coverage.tsv" || echo "FAIL summary"
echo "ALL DONE"
