#!/usr/bin/env bash
# EXP-0012: makes the seed sets the trials used.
# Usage: exp-0012-seeds.sh FIXTURES MADE OUT
# FIXTURES is fixtures/partition (generated, checked by its manifest);
# MADE holds the six volumes exp-0011-claims-images.sh makes
# (WORKDIR/claims/img). OUT/full gets one seed per image, made with
# exp-0012-seed.py --recover: the 29 fixtures other than the two
# fragmented ones (almost every sector non-zero, so a seed of 66 MB, far
# over the trials' -max_len of 131,072 bytes), and the six made volumes.
# OUT/empty gets
# the empty seed: 5 bytes declaring 8,192 sectors, flags 0. Every seed is
# then checked against exp-0012-seeds.sha256.
# The four FatFs volumes are the same on every run of that script; relan's
# two carry the clock's times, so their seeds match only when MADE holds
# the volumes of the run of record (exp-0012-made-volumes.sha256).
# A failure prints a line starting FAIL.
set -u
[ $# -eq 3 ] || { echo "usage: exp-0012-seeds.sh FIXTURES MADE OUT" >&2; exit 2; }
fx=$1; made=$2; out=$3; T=$(cd "$(dirname "$0")" && pwd)
[ -n "$out" ] && [ ! -e "$out" ] || { echo "FAIL OUT exists or is empty: $out" >&2; exit 2; }
(cd "$fx" && sha256sum -c --quiet MANIFEST.sha256) || echo "FAIL fixture manifest"
(cd "$made" && sha256sum -c --quiet "$T/exp-0012-made-volumes.sha256") ||
    echo "FAIL made volumes differ from the run of record (relan's carry the clock)"
mkdir -p "$out/full" "$out/empty" || exit 2
sed -n 's|^[0-9a-f]*  full/\(.*\)\.seed$|\1|p' "$T/exp-0012-seeds.sha256" | while read -r name; do
    src=$fx/$name
    [ -e "$src" ] || src=$made/$name
    python3 -I "$T/exp-0012-seed.py" "$src" "$out/full/$name.seed" --recover > /dev/null ||
        echo "FAIL seed $name"
done
printf '\000\040\000\000\000' > "$out/empty/empty.seed"
(cd "$out" && sha256sum -c --quiet "$T/exp-0012-seeds.sha256") && echo "36 seeds equal to exp-0012-seeds.sha256" ||
    echo "FAIL seeds differ"
