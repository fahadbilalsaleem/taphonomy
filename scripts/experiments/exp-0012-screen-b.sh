#!/usr/bin/env bash
# EXP-0012 screen (b): the hand-made inputs for canaries no trial triggered.
# Usage: exp-0012-screen-b.sh TREE SEEDS OUT
# TREE is the built canary tree (exp-0012-tree.sh ... build); SEEDS holds
# full/ (exp-0012-seeds.sh). exp-0012-screen-b.py makes one raw image per
# canary from a seed, exp-0012-seed.py turns each into an input, and each
# input runs once on the target named. #17, #36 and #58 must fire; #18 (the
# dead check) and #92 must not, and #92's input must fire #98, the refusal
# that precedes it. A failure prints a line starting FAIL.
set -u
[ $# -eq 3 ] || { echo "usage: exp-0012-screen-b.sh TREE SEEDS OUT" >&2; exit 2; }
tree=$1; seeds=$2; out=$3; T=$(cd "$(dirname "$0")" && pwd)
BIN=$tree/fuzz/target/x86_64-unknown-linux-gnu/release
[ -n "$out" ] && [ ! -e "$out" ] || { echo "FAIL OUT exists or is empty: $out" >&2; exit 2; }
python3 -I "$T/exp-0012-screen-b.py" "$seeds/full" "$out/raw" > /dev/null || { echo "FAIL build"; exit 1; }
check() { # canary target must-fire must-not-fire
    python3 -I "$T/exp-0012-seed.py" "$out/raw/$1.img" "$out/$1.seed" > /dev/null || echo "FAIL seed $1"
    fired=" $("$BIN/$2" "$out/$1.seed" -runs=1 2>&1 |
        sed -n 's/^CANARY \([0-9]*\) .*/\1/p' | sort -n | tr '\n' ' ')"
    echo "#$1 on $2 ($(sha256sum < "$out/$1.seed" | cut -c1-16)): fires${fired% }"
    for c in $3; do case $fired in *" $c "*) ;; *) echo "FAIL #$1: #$c did not fire" ;; esac; done
    for c in $4; do case $fired in *" $c "*) echo "FAIL #$1: #$c fired" ;; esac; done
}
check 17 analyse_image 17 ""
check 18 analyse_image "" 18
check 36 exfat_reader 36 ""
check 58 exfat_reader 58 ""
check 92 exfat_reader 98 92
