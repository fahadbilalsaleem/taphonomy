#!/usr/bin/env bash
# The identity proof of ADR-0018 section 5, as a script.
# Usage: identity-proof.sh BINARY WORKDIR IMAGE...
# For each image, runs BINARY three ways (no option; --recover;
# --recover --output WORKDIR/out into an empty directory) and prints one
# row: image name, mode, exit status, SHA-256 of standard output, of
# standard error, and of the list of written files with their digests.
# A change that should not alter output is proved by running the build
# before it and the build after it with the same WORKDIR path (moved
# aside between the two runs, never deleted) and comparing the two tables
# byte for byte: standard output names the files written under WORKDIR,
# so tables made in two different directories differ for that reason
# alone. A failure prints a line starting FAIL.
set -u
[ $# -ge 3 ] || { echo "usage: identity-proof.sh BINARY WORKDIR IMAGE..." >&2; exit 2; }
bin=$1; work=$2; shift 2
[ -x "$bin" ] || { echo "FAIL not executable: $bin" >&2; exit 2; }
[ -n "$work" ] && mkdir -p "$work" || exit 2
out="$work/out"
for img in "$@"; do
    [ -f "$img" ] || { echo "FAIL missing image: $img" >&2; continue; }
    name=$(basename "$img")
    for mode in none recover output; do
        # Moved aside, never deleted: the next run gets an empty out at the
        # same path, so tables from two binaries stay comparable.
        [ -e "$out" ] && mv -- "$out" "$work/done-$(date +%s%N)"
        mkdir "$out"
        case $mode in
            none) "$bin" "$img" >"$work/o" 2>"$work/e"; rc=$? ;;
            recover) "$bin" "$img" --recover >"$work/o" 2>"$work/e"; rc=$? ;;
            output) "$bin" "$img" --recover --output "$out" >"$work/o" 2>"$work/e"; rc=$? ;;
        esac
        files=$(cd "$out" && /usr/bin/find . -type f -print0 | sort -z | xargs -0 -r sha256sum | sha256sum | cut -c1-64)
        printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$name" "$mode" "$rc" \
            "$(sha256sum <"$work/o" | cut -c1-64)" "$(sha256sum <"$work/e" | cut -c1-64)" "$files"
    done
done
