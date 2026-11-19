#!/usr/bin/env bash
# M15 acceptance (ADR-0022 section 14 item 3, Appendix F): NIST's eleven MBR exFAT
# images and forensics-samples' Linux image, binary against EXP-0011's
# scripts under rule moec. Usage: m15-acceptance.sh BINARY INPUTS WORK ORIGINALS
# INPUTS holds nist/img/*.dd, nist/img-sha256.txt and linux/fs.exfat as
# exp-0011-run.sh names them; ORIGINALS is forensics-samples' originals.
# A failure prints a line starting FAIL; the last line counts images failing.
set -u
bin=$1; in=$2; work=$3; orig=$4; T=$(cd "$(dirname "$0")" && pwd); A=$T/m15-acceptance.py
[ -x "$bin" ] && [ -d "$in/nist/img" ] || { echo "FAIL usage" >&2; exit 2; }
[ -n "$work" ] || exit 2
mkdir -p "$work" || exit 2
(cd "$in/nist" && sha256sum -c --quiet img-sha256.txt) || echo "FAIL NIST digests"
[ "$(sha256sum < "$in/linux/fs.exfat" | cut -c1-64)" = 98d518601199a32054158bb3a759e12b554fd2ebcc5960541caf9e1a907198d0 ] || echo "FAIL linux digest"
echo "binary $(sha256sum < "$bin" | cut -c1-64)"
fails=0
one() { # image originals
    name=$(basename "$1"); out=$work/out-$name
    [ -e "$out" ] && mv -- "$out" "$work/old-$name-$(date +%s%N)"
    mkdir "$out"
    python3 -I "$T/exp-0011-scan.py" "$1" --deleted-dirs > "$work/$name.tsv" || echo "FAIL scan $name"
    "$bin" "$1" --recover --output "$out" > "$work/$name.stdout" 2> "$work/$name.stderr"
    echo "  exit $? $(grep -E '^coverage|^artifacts' "$work/$name.stdout" | tr -s ' ' | tr '\n' ';')"
    python3 -I "$A" "$T" "$1" "$work/$name.tsv" "$work/$name.stdout" "$out" ${2:+"$2"} || fails=$((fails + 1))
}
for f in "$in"/nist/img/*.dd; do
    python3 -I -c "import sys; sys.exit(open(sys.argv[1],'rb').read(512)[450] == 0xEE)" "$f" || continue
    one "$f"
done
one "$in/linux/fs.exfat" "$orig"
echo "images failing: $fails"
