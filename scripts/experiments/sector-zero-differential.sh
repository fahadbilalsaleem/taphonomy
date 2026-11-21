#!/usr/bin/env bash
# ADR-0024 acceptance: each volume with no partition table against its
# twin wrapped in an MBR (the same bytes as one partition at sector 2048).
# Usage: sector-zero-differential.sh BINARY LIST WORK
# LIST holds one image path per line; WORK must not exist. Each twin is
# made by mbr-wrap-sparse.py, then the binary runs on both with --recover
# --output. Prints one row per volume: exit status bare and wrapped, the
# report lines that still differ once the lines that must differ are
# normalised (the image's path, size and digest; the partition table
# lines; "whole image" against "partition 1"; p0- against p1-; the output
# directory), the recovered files bare and wrapped, and whether every one
# is byte-identical to its twin's. The last line counts volumes that
# differ. A failure prints a line starting FAIL.
set -u
[ $# -eq 3 ] || { echo "usage: sector-zero-differential.sh BINARY LIST WORK" >&2; exit 2; }
bin=$1; list=$2; work=$3; T=$(cd "$(dirname "$0")" && pwd)
[ -x "$bin" ] && [ -s "$list" ] || { echo "FAIL usage" >&2; exit 2; }
[ -n "$work" ] && [ ! -e "$work" ] || { echo "FAIL WORK exists: $work" >&2; exit 2; }
mkdir -p "$work/wrapped" || exit 2
norm() { # report outdir
    sed -e "s|$2|OUT|g" \
        -e '/^path /d; /^size /d; /^read /d; /^sha256 /d' \
        -e '/^disk signature/d; /^partition [0-9]   type /d; /^partition table /d' \
        -e 's/^whole image   filesystem/partition 1   filesystem/' \
        -e 's|p0-c|p1-c|g' "$1"
}
printf 'volume\texit_bare\texit_wrapped\treport_lines_differing\tfiles_bare\tfiles_wrapped\tbytes_equal\n'
differ=0
while read -r src; do
    [ -n "$src" ] || continue
    n=$(basename "$src"); w="$work/wrapped/mbr-$n"
    python3 -I "$T/mbr-wrap-sparse.py" "$src" "$w" > /dev/null || { echo "FAIL wrap $n"; differ=$((differ + 1)); continue; }
    ob="$work/$n.bare.out"; ow="$work/$n.wrap.out"; mkdir "$ob" "$ow" || exit 2
    "$bin" "$src" --recover --output "$ob" > "$work/$n.bare.txt" 2> "$work/$n.bare.err"; eb=$?
    "$bin" "$w" --recover --output "$ow" > "$work/$n.wrap.txt" 2> "$work/$n.wrap.err"; ew=$?
    norm "$work/$n.bare.txt" "$ob" > "$work/$n.bare.norm"
    norm "$work/$n.wrap.txt" "$ow" > "$work/$n.wrap.norm"
    d=$(diff "$work/$n.bare.norm" "$work/$n.wrap.norm" | /bin/grep -c '^[<>]')
    fb=$(ls "$ob" | wc -l); fw=$(ls "$ow" | wc -l)
    eq=yes
    [ "$fb" = "$fw" ] || eq=NO
    for f in "$ob"/*; do
        [ -e "$f" ] || continue
        cmp -s "$f" "$ow/$(basename "$f" | sed 's/^p0-/p1-/')" || eq=NO
    done
    { [ "$eb" = "$ew" ] && [ "$d" = 0 ] && [ "$eq" = yes ]; } || differ=$((differ + 1))
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$n" "$eb" "$ew" "$d" "$fb" "$fw" "$eq"
done < "$list"
echo "volumes differing $differ"
