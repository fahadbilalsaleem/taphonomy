#!/usr/bin/env bash
# EXP-0012 Q3: crashes, hangs and slow inputs.
# Usage: exp-0012-q3.sh TREE TRIALS
# Counts, over every finished trial in TRIALS, the fuzzing exit status and
# the hang check's (each trial's final corpus replayed with a 10 s limit),
# and the artifacts libFuzzer left, by kind. Then replays every slow-unit
# artifact alone, once, on core 1, on the target its trial ran (TREE's
# canary build), and prints its size, seconds and exit status. A failure
# prints a line starting FAIL.
set -u
[ $# -eq 2 ] || { echo "usage: exp-0012-q3.sh TREE TRIALS" >&2; exit 2; }
BIN=$1/fuzz/target/x86_64-unknown-linux-gnu/release; trials=$2
n=$(ls -d "$trials"/*/DONE 2>/dev/null | wc -l)
[ "$n" -gt 0 ] || { echo "FAIL no finished trial in $trials" >&2; exit 2; }
echo "finished trials $n"
echo "first start $(cat "$trials"/*/META | sed -n 's/.* start=\([0-9]*\)$/\1/p' | sort -n | head -1 | xargs -I{} date -d @{} -Is)"
echo "last end $(cat "$trials"/*/META | sed -n 's/^rc=[0-9]* end=\([0-9]*\)$/\1/p' | sort -n | tail -1 | xargs -I{} date -d @{} -Is)"
echo "fuzzing exit status: $(cat "$trials"/*/META | sed -n 's/^rc=\([0-9]*\) .*/\1/p' | sort | uniq -c | tr -s ' ' | tr '\n' ';')"
echo "hang check exit status: $(cat "$trials"/*/META | sed -n 's/^hangcheck_rc=\([0-9]*\) .*/\1/p' | sort | uniq -c | tr -s ' ' | tr '\n' ';')"
echo "artifacts by kind: $(ls "$trials"/*/artifacts | /bin/grep -E '^[a-z]+-' | sed 's/-[0-9a-f]*$//' | sort | uniq -c | tr -s ' ' | tr '\n' ';')"
# The time is libFuzzer's own for the one execution ("Executed ... in N
# ms"), the measure its slow-unit report uses (default threshold 10 s).
printf 'trial\ttarget\tunit\tbytes\tms\trc\n'
for f in "$trials"/*/artifacts/slow-unit-*; do
    [ -e "$f" ] || continue
    d=$(dirname "$(dirname "$f")")
    target=$(sed -n 's/.*target=\([a-z_]*\).*/\1/p' "$d/META")
    log=$(taskset -c 1 "$BIN/$target" "$f" -runs=1 2>&1)
    rc=$?
    ms=$(printf '%s\n' "$log" | sed -n 's/^Executed .* in \([0-9]*\) ms$/\1/p')
    [ "$rc" = 0 ] && [ -n "$ms" ] || echo "FAIL $f exits $rc, time '${ms}'"
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$(basename "$d")" "$target" "$(basename "$f")" \
        "$(stat -c %s "$f")" "$ms" "$rc"
done
