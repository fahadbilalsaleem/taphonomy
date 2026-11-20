#!/usr/bin/env bash
# EXP-0012: makes the canary tree the trials ran, from the repository.
# Usage: exp-0012-tree.sh REPO TREE [build]
# Exports src/, Cargo.toml, Cargo.lock and rust-toolchain.toml at d7ad2a0
# into TREE (which must not exist), places the canaries with
# exp-0012-canaries.py, checks the canary map against exp-0012-canaries.tsv,
# and copies the harness (exp-0012-harness/) into TREE/fuzz. With "build",
# builds both targets with `cargo fuzz build -s none` (cargo-fuzz 0.13.2,
# the pinned stable toolchain). Never run inside the repository: the
# harness calls the canary module, which only TREE has.
set -u
[ $# -ge 2 ] || { echo "usage: exp-0012-tree.sh REPO TREE [build]" >&2; exit 2; }
repo=$1; tree=$2; T=$(cd "$(dirname "$0")" && pwd)
[ -n "$tree" ] && [ ! -e "$tree" ] || { echo "FAIL TREE exists or is empty: $tree" >&2; exit 2; }
mkdir -p "$tree" || exit 2
git -C "$repo" archive d7ad2a0 src Cargo.toml Cargo.lock rust-toolchain.toml |
    tar -x -C "$tree" || { echo "FAIL export d7ad2a0" >&2; exit 2; }
python3 -I "$T/exp-0012-canaries.py" "$tree" "$tree/canaries.tsv" || exit 2
cmp "$tree/canaries.tsv" "$T/exp-0012-canaries.tsv" || { echo "FAIL canary map differs" >&2; exit 1; }
mkdir -p "$tree/fuzz/fuzz_targets" || exit 2
install -m 644 "$T/exp-0012-harness/Cargo.toml" "$T/exp-0012-harness/Cargo.lock" "$tree/fuzz/" || exit 2
install -m 644 "$T"/exp-0012-harness/fuzz_targets/*.rs "$tree/fuzz/fuzz_targets/" || exit 2
echo "canary map equal to exp-0012-canaries.tsv"
if [ "${3:-}" = build ]; then
    (cd "$tree/fuzz" && cargo fuzz build -s none) || { echo "FAIL build" >&2; exit 1; }
    sha256sum "$tree"/fuzz/target/x86_64-unknown-linux-gnu/release/{exfat_reader,analyse_image}
fi
