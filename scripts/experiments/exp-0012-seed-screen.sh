#!/usr/bin/env bash
# EXP-0012 screen (a): the canaries the full seed set fires with no
# mutation, reported apart from the trials' results.
# Usage: exp-0012-seed-screen.sh TREE SEEDS OUT_FILE
# Runs every seed in SEEDS/full once, as its own file, on both targets of
# TREE's canary build, and writes the canaries fired, one per line, to
# OUT_FILE (exp-0012-stats.py's SEED_TRIGGERED). Each seed is passed as a
# file: a corpus directory replayed with -runs=0 and no -max_len cuts every
# input to 4 bytes. A failure prints a line starting FAIL.
set -u
[ $# -eq 3 ] || { echo "usage: exp-0012-seed-screen.sh TREE SEEDS OUT_FILE" >&2; exit 2; }
BIN=$1/fuzz/target/x86_64-unknown-linux-gnu/release; seeds=$2; out=$3
n=$(( 2 * $(ls "$seeds"/full/*.seed | wc -l) ))
for t in exfat_reader analyse_image; do
    for s in "$seeds"/full/*.seed; do
        "$BIN/$t" "$s" -runs=1 2>&1 | sed -n 's/^CANARY \([0-9]*\) .*/\1/p'
        [ "${PIPESTATUS[0]}" = 0 ] || echo "FAIL $t $s" >&2
    done
done | sort -n -u > "$out"
echo "$n replays; canaries fired: $(tr '\n' ' ' < "$out")"
