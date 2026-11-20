#!/usr/bin/env bash
# EXP-0012 trial runner (design v3).
# Usage: exp-0012-run.sh TREE SEEDS OUT QUEUE
# TREE is the canary tree exp-0012-tree.sh made, with both targets built;
# SEEDS holds full/ (exp-0012-seeds.sh) and empty/empty.seed; QUEUE is
# exp-0012-queue.txt. Each queue line: TRIAL_ID TARGET SEEDSET
# MUTATOR(on|off) RUNS SEED.
# A trial's budget is a number of executions, which a machine pause does
# not change; libFuzzer's own hang alarm is off during fuzzing (a pause
# makes it fire falsely). After each trial its final corpus is replayed with
# a 10 s limit on the same core: that is the hang check.
# Runs at most three trials at once, each pinned to its own core (1 to 3;
# core 0 is left for builds). Writes one directory per trial under
# OUT/trials/, and a machine heartbeat to OUT/heartbeat.log every 30 s.
# The run of record used this logic with the paths fixed in the script.
set -u
[ $# -eq 4 ] || { echo "usage: exp-0012-run.sh TREE SEEDS OUT QUEUE" >&2; exit 2; }
BIN=$1/fuzz/target/x86_64-unknown-linux-gnu/release
SEEDS=$2
E=$3
queue=$4
[ -s "$queue" ] || { echo "empty or missing queue: $queue" >&2; exit 2; }
for t in exfat_reader analyse_image; do
    [ -x "$BIN/$t" ] || { echo "missing binary $BIN/$t" >&2; exit 2; }
done
[ -n "$(ls -A "$SEEDS/full")" ] || { echo "no full seeds" >&2; exit 2; }
[ -s "$SEEDS/empty/empty.seed" ] || { echo "no empty seed" >&2; exit 2; }
mkdir -p "$E/trials" "$E/locks" || exit 2

( while :; do date +%s >> "$E/heartbeat.log"; sleep 30; done ) &
beat=$!
trap 'kill $beat 2>/dev/null' EXIT

run_trial() {
    local id=$1 target=$2 seedset=$3 mutator=$4 runs=$5 seed=$6 core=$7
    local d=$E/trials/$id
    if [ -e "$d/DONE" ]; then return 0; fi
    rm -rf -- "${d:?}"
    mkdir -p "$d/corpus" "$d/artifacts"
    cp "$SEEDS/$seedset"/* "$d/corpus/"
    local fix=()
    [ "$mutator" = off ] && fix=(env FUZZ_NO_FIX=1)
    printf 'id=%s target=%s seeds=%s mutator=%s runs=%s seed=%s core=%s start=%s\n' \
        "$id" "$target" "$seedset" "$mutator" "$runs" "$seed" "$core" "$(date +%s)" > "$d/META"
    "${fix[@]}" taskset -c "$core" "$BIN/$target" "$d/corpus" \
        -artifact_prefix="$d/artifacts/" -runs="$runs" -seed="$seed" \
        -max_len=131072 -rss_limit_mb=700 -malloc_limit_mb=700 -timeout=0 \
        -print_final_stats=1 > "$d/log" 2>&1
    echo "rc=$? end=$(date +%s)" >> "$d/META"
    taskset -c "$core" "$BIN/$target" "$d/corpus" -runs=0 -max_len=131072 \
        -timeout=10 -rss_limit_mb=700 -malloc_limit_mb=700 > "$d/hangcheck.log" 2>&1
    echo "hangcheck_rc=$? hangcheck_end=$(date +%s)" >> "$d/META"
    touch "$d/DONE"
}

pids=()
while read -r id target seedset mutator runs seed; do
    [ -z "${id:-}" ] && continue
    case $id in \#*) continue ;; esac
    while :; do
        for core in 1 2 3; do
            if mkdir "$E/locks/core$core" 2>/dev/null; then
                ( run_trial "$id" "$target" "$seedset" "$mutator" "$runs" "$seed" "$core"
                  rmdir "$E/locks/core$core" ) &
                pids+=($!)
                continue 3
            fi
        done
        sleep 5
    done
done < "$queue"
# Only the trials: a bare `wait` would also wait for the heartbeat loop.
wait "${pids[@]}"
