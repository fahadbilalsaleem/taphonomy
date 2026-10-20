#!/usr/bin/env bash
# Usage: exp-0011-run.sh WORKDIR
#
# Reruns every measurement EXP-0011 reports, from clean, and prints them.
# WORKDIR holds the inputs, none of which is committed:
#   nist/img/dfr-*-xfat.dd    NIST's twenty exFAT images, expanded
#   nist/img-sha256.txt       their digests, as EXP-0011 lists them
#   nist/setup-july-10-2012.pdf
#   img/exp9-A.vhd img/exp9-B.vhd     from fixtures/partition
#   linux/fs.exfat            forensics-samples' image, expanded
#   linux/original-files/     forensics-samples' originals
# Results go to WORKDIR/final. Every input is checked by digest before and
# after the runs. A failure prints a line starting FAIL and the run goes on,
# so a failure is never silent.
set -u
W=$1
T=$(cd "$(dirname "$0")" && pwd)
F=$W/final
rm -rf -- "${F:?}"
mkdir -p "$F"

check_inputs() {
    (cd "$W/nist" && sha256sum -c --quiet img-sha256.txt) || echo "FAIL $1: NIST image digests"
    while read -r want path; do
        got=$(sha256sum "$W/$path" 2>/dev/null | cut -c1-64)
        [ "$got" = "$want" ] || echo "FAIL $1: $path is ${got:-missing}, expected $want"
    done <<'EOF'
86b5a888e60f1127ac77e57929dd64bd4346af882f904ddd45ba7d90afbc803e nist/setup-july-10-2012.pdf
f9ad9b94a1ce38b45abfda1c17d78f0278e5fe5314f4556ea431439605dd545e img/exp9-A.vhd
a0ffa52069c0238f47201cea69df495a85c27ad589c1414299644339b9e4fc39 img/exp9-B.vhd
98d518601199a32054158bb3a759e12b554fd2ebcc5960541caf9e1a907198d0 linux/fs.exfat
EOF
    echo "inputs checked ($1)"
}

check_inputs before
sha256sum "$T"/exp-0011-* > "$F/scripts-sha256.txt"

run() { # output-file command...
    out=$1; shift
    "$@" > "$out" || echo "FAIL $(basename "$out"): exit $?"
}

# EXP-0009's images, against the generator.
for x in A B; do
    run "$F/exp9-$x.tsv" python3 -I "$T/exp-0011-scan.py" "$W/img/exp9-$x.vhd"
    run "$F/exp9-$x.score" python3 -I "$T/exp-0011-score-exp9.py" "$W/img/exp9-$x.vhd" "$F/exp9-$x.tsv"
    echo "== exp9-$x"
    cat "$F/exp9-$x.score"
done

# NIST, both scopes.
# Two tables per image: deleted directories not read ("unread"), and read
# ("read"). Claims-only, what M15 does, is a variant on the "read" table.
for scope in unread read; do
    flag=""
    [ "$scope" = read ] && flag="--deleted-dirs"
    for f in "$W"/nist/img/*.dd; do
        b=$(basename "$f" .dd)
        run "$F/$b.$scope.tsv" python3 -I "$T/exp-0011-scan.py" "$f" $flag
        run "$F/$b.$scope.score" python3 -I "$T/exp-0011-score-nist.py" "$f" "$F/$b.$scope.tsv"
    done
done

count() { # file variant decision truth
    awk -v v="$2" -v d="$3" -v t="$4" '$1=="TALLY" && $2==v && $3==d && $4==t {n+=$5} END{print n+0}' "$1"
}
echo "== NIST per image, recovered right/wrong"
printf '%-18s %5s  %-9s %-9s %-9s %-9s\n' image sets 'unread' 'unread' 'claims' 'read'
printf '%-18s %5s  %-9s %-9s %-9s %-9s\n' '' '' 'adr' 'moe' 'moec' 'moe'
for f in "$W"/nist/img/*.dd; do
    b=$(basename "$f" .dd)
    n=$(grep -o '[0-9]* deleted sets' "$F/$b.unread.score" | cut -d' ' -f1)
    printf '%-18s %5s' "$b" "$n"
    for sv in unread:adr unread:moe read:moec read:moe; do
        scope=${sv%%:*}; v=${sv#*:}
        printf '  %4s/%-3s' "$(count "$F/$b.$scope.score" $v recover right)" "$(count "$F/$b.$scope.score" $v recover wrong)"
    done
    echo
done
echo "== NIST totals"
for scope in unread read; do
    for v in adr m o e mo moe u rec lit adrc moec; do
        printf '%s %-4s ' "$scope" "$v"
        cat "$F"/*."$scope".score | awk -v v="$v" '$1=="TALLY" && $2==v {n[$3" "$4]+=$5}
            END {printf "recovered right %d wrong %d unjudged %d | refused right %d wrong %d unjudged %d\n",
                 n["recover right"], n["recover wrong"], n["recover unjudged"],
                 n["refuse right"], n["refuse wrong"], n["refuse unjudged"]}'
    done
done

echo "== NIST: decisions that differ between adrc and moec, and every recovery not right under moec or moe"
python3 -I - "$T/exp-0011-score-nist.py" "$F" "$W"/nist/img/*.dd <<'PY' || echo "FAIL decision differences"
import contextlib, io, runpy, sys
scorer, final, images = sys.argv[1], sys.argv[2], sys.argv[3:]
for img in images:
    b = img.rsplit('/', 1)[-1][:-3]
    sys.argv = ['x', img, f'{final}/{b}.read.tsv']
    with contextlib.redirect_stdout(io.StringIO()):
        g = runpy.run_path(scorer)
    for r in g['dels']:
        tr = g['truth'](r)
        a, m = g['decide'](r, 'adrc'), g['decide'](r, 'moec')
        if a != m:
            print(f"DIFFERS {b} {r['path']}/{r['name']} adrc={a} moec={m} truth={tr}")
        for v in ('moec', 'moe'):
            if g['decide'](r, v) == 'recover' and tr not in ('own', 'own~', 'format-own'):
                print(f"RECOVERED-NOT-RIGHT {v} {b} {r['path']}/{r['name']} truth={tr}")
PY

echo "== NIST offset bytes of deleted sets, per image ("read" tables)"
for f in "$F"/*.read.tsv; do
    awk -F'\t' -v f="$(basename "$f" .read.tsv)" '!/^#/ && $1!="path" {d=($3==1||$18==1)?"deleted":"live"; c[d" "$17]++} END{for(k in c) print f, k, c[k]}' "$f"
done | sort

echo "== NIST orphan secondaries and unread deleted directories"
grep -H -e orphan -e deleted_dir_unread "$F"/*.tsv | sed "s|$F/||"

echo "== NIST sector lists (second method, rule moe, "read" tables)"
run "$F/setup.txt" python3 -I "$T/exp-0011-pdf-text.py" "$W/nist/setup-july-10-2012.pdf"
for f in "$W"/nist/img/*.dd; do
    b=$(basename "$f" .dd)
    python3 -I "$T/exp-0011-sector-list.py" "$F/setup.txt" "$T/exp-0011-score-nist.py" "$f" "$F/$b.read.tsv" || echo "FAIL sector list $b"
done | tee "$F/sector-list.txt" | grep -v '^TALLY'
awk '$1=="TALLY" {for(i=3;i<=NF;i++){split($i,kv,"="); n[kv[1]]+=kv[2]}} END{printf "sector lists: match %d differ %d unlisted %d ambiguous %d\n", n["match"], n["differ"], n["unlisted"], n["ambiguous"]}' "$F/sector-list.txt"

echo "== Linux (forensics-samples fs.exfat), deleted directories read"
run "$F/linux.tsv" python3 -I "$T/exp-0011-scan.py" "$W/linux/fs.exfat" --deleted-dirs
run "$F/linux.score" python3 -I "$T/exp-0011-score-linux.py" "$W/linux/fs.exfat" "$F/linux.tsv" "$W/linux/original-files"
grep TALLY "$F/linux.score"
awk -F'\t' '!/^#/ && $1!="path" {c[$17]++} END{for(k in c) print "linux offset bytes", k, c[k]}' "$F/linux.tsv"

echo "== Clusters claimed by deleted sets, against each volume's cluster count"
python3 -I - "$F"/exp9-A.tsv "$F"/exp9-B.tsv "$F"/*.read.tsv "$F"/linux.tsv <<'PY' || echo "FAIL claim density"
import sys
worst = (0, '')
for t in sys.argv[1:]:
    lines = open(t).read().splitlines()
    g = dict(kv.split('=', 1) for kv in lines[0][2:].split(' ') if '=' in kv)
    rows = [l.split('\t') for l in lines if not l.startswith('#')]
    h = rows[0]
    claimed = 0
    for r in (dict(zip(h, x)) for x in rows[1:]):
        if r['deleted'] == '1' or r.get('in_deleted_dir') == '1':
            claimed += sum(int(b) - int(a) + 1 for a, b in (x.split('-') for x in r['clusters'].split(',') if x))
    ratio = claimed / int(g['clusters'])
    worst = max(worst, (ratio, t.rsplit('/', 1)[-1]))
print(f'largest claimed/clusters: {worst[0]:.2f} ({worst[1]})')
PY

echo "== Bitmap chain and structure clusters"
for f in "$W"/img/exp9-A.vhd "$W"/img/exp9-B.vhd "$W"/nist/img/*.dd "$W"/linux/fs.exfat; do
    python3 -I "$T/exp-0011-check-bitmap.py" "$f" || echo "FAIL bitmap $(basename "$f")"
done

check_inputs after
