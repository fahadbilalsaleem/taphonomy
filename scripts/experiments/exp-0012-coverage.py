#!/usr/bin/env python3
"""EXP-0012 coverage summary: line coverage per file, per configuration.

Usage: exp-0012-coverage.py JSON_DIR OUT_TSV
Reads <trial>.json (llvm-cov export -summary-only) for every trial, keeps
the files the canaries were placed in, and writes per configuration and
file: trials, median, minimum and maximum of lines covered / lines.
"""
import json
import os
import statistics
import sys

FILES = ['src/partition.rs', 'src/fat32.rs', 'src/fat_directory.rs',
         'src/fat_recovery.rs', 'src/exfat_boot.rs', 'src/exfat_directory.rs',
         'src/exfat_claims.rs']


def per_file(path):
    data = json.load(open(path))['data'][0]['files']
    out = {}
    for f in data:
        for want in FILES:
            if f['filename'].endswith('/' + want.split('/', 1)[1]) and '/fuzz/' not in f['filename']:
                lines = f['summary']['lines']
                out[want] = (lines['covered'], lines['count'])
    return out


def main():
    jdir, out = sys.argv[1], sys.argv[2]
    by_config = {}
    for name in sorted(os.listdir(jdir)):
        if not name.endswith('.json') or os.path.getsize(os.path.join(jdir, name)) == 0:
            continue
        trial = name[:-5]
        by_config.setdefault(trial.split('-')[0], []).append(per_file(os.path.join(jdir, name)))
    with open(out, 'w') as f:
        f.write('config\tfile\ttrials\tlines\tmedian_covered\tmin_covered\tmax_covered\tmedian_percent\n')
        for config in sorted(by_config):
            for want in FILES:
                vals = [t[want] for t in by_config[config] if want in t]
                if not vals:
                    continue
                cov = [c for c, _ in vals]
                total = vals[0][1]
                assert all(n == total for _, n in vals), (config, want)
                med = statistics.median(cov)
                f.write(f'{config}\t{want}\t{len(vals)}\t{total}\t{med}\t{min(cov)}\t'
                        f'{max(cov)}\t{100 * med / total:.1f}\n')


if __name__ == '__main__':
    main()
