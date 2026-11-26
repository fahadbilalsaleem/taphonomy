#!/usr/bin/env bash
# Shows exactly how a change alters the report: runs two taphonomy
# binaries over the same images with no option (the report lists every
# decision) and prints, per image, both exit statuses and how many lines
# differ. A change that alters output is proved by this table and the
# differing lines it leaves in OUTDIR (diff OUTDIR/<image>.base .new);
# one that should not is proved by scripts/identity-proof.sh.
# Usage: report-diff.sh BASE_BINARY NEW_BINARY OUTDIR IMAGE...
# OUTDIR must not exist. Prints one row per image (name, base exit, new
# exit, differing lines) and a total; exits 1 if any exit status differs.
set -u
[ $# -ge 4 ] || { echo "usage: report-diff.sh BASE NEW OUTDIR IMAGE..." >&2; exit 2; }
base=$1; new=$2; out=$3; shift 3
for b in "$base" "$new"; do
    [ -x "$b" ] || { echo "FAIL not executable: $b" >&2; exit 2; }
done
[ -e "$out" ] && { echo "FAIL $out exists" >&2; exit 2; }
mkdir -p "$out" || exit 2
total=0; images=0; changed=0; exits=0
for img in "$@"; do
    [ -f "$img" ] || { echo "FAIL missing image: $img" >&2; exits=1; continue; }
    n=$(basename "$img")
    [ -e "$out/$n.base" ] && { echo "FAIL two images named $n" >&2; exit 2; }
    "$base" "$img" >"$out/$n.base" 2>&1; rb=$?
    "$new" "$img" >"$out/$n.new" 2>&1; rn=$?
    d=$(diff "$out/$n.base" "$out/$n.new" | /bin/grep -c '^[<>]')
    printf '%s\t%s\t%s\t%s\n' "$n" "$rb" "$rn" "$d"
    images=$((images + 1)); total=$((total + d))
    [ "$d" -gt 0 ] && changed=$((changed + 1))
    [ "$rb" = "$rn" ] || exits=1
done
echo "images $images, images that differ $changed, differing lines $total (each changed line counts twice: before and after)"
exit $exits
