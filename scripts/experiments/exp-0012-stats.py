#!/usr/bin/env python3
"""EXP-0012 statistics, standard library only.

Usage:
  exp-0012-stats.py TRIALS_DIR HEARTBEAT CANARIES_TSV SEED_TRIGGERED OUT_DIR
  exp-0012-stats.py --selftest

Reads every finished trial (META, log). Design v3: a trial is valid when it
ran its whole execution budget; a heartbeat gap voids only its seconds.
Writes:
  trials.tsv     one row per trial: config, validity, canaries, execs, rc
  canaries.tsv   per configuration and canary: trials triggered, Kaplan-
                 Meier median executions to trigger and survival at the end
  tests.tsv      Mann-Whitney U (two-sided, asymptotic, tie-corrected,
                 continuity-corrected) and Vargha-Delaney A12 on canaries
                 per trial for C1 vs C2 and C4 vs C5
  crashes.tsv    trials that ended with a crash, with the panic line
  km.tsv         per configuration and canary: the Kaplan-Meier median
                 executions to trigger and its 95% interval (log-log
                 Greenwood band; an empty bound is beyond the budget)
  sooner.tsv     per canary, C1 against C2: each median and interval, and
                 which configuration triggered it sooner
  configs.tsv    per configuration, the canaries triggered in any trial;
                 and the canaries C3 (empty seed) alone triggered
  by_file.tsv    per configuration and file: sites, sites triggered in any
                 trial, median sites triggered per trial
  inputs.sha256  the digest of every input read; its own digest is printed
A median is the first execution count at which the estimate is at most
0.5.
"""
import hashlib
import math
import os
import re
import statistics
import sys

CANARY = re.compile(r"^CANARY (\d+) exec (\d+) secs ([0-9.]+)$")
EXECS = re.compile(r"stat::number_of_executed_units:\s*(\d+)")
PANIC = re.compile(r"panicked at ([^\s]+)")


# ---------- statistics ----------

def norm_sf(z):
    return 0.5 * math.erfc(z / math.sqrt(2))


def mann_whitney(x, y):
    """U for x, two-sided p; asymptotic with tie and continuity correction,
    as scipy.stats.mannwhitneyu(method='asymptotic', use_continuity=True)."""
    n1, n2 = len(x), len(y)
    pooled = sorted((v, i) for i, v in enumerate(x + y))
    ranks = [0.0] * (n1 + n2)
    ties = []
    i = 0
    while i < len(pooled):
        j = i
        while j + 1 < len(pooled) and pooled[j + 1][0] == pooled[i][0]:
            j += 1
        r = (i + j) / 2 + 1
        for k in range(i, j + 1):
            ranks[pooled[k][1]] = r
        ties.append(j - i + 1)
        i = j + 1
    r1 = sum(ranks[:n1])
    u1 = r1 - n1 * (n1 + 1) / 2
    n = n1 + n2
    mu = n1 * n2 / 2
    tie_term = sum(t ** 3 - t for t in ties) / (n * (n - 1))
    sigma = math.sqrt(n1 * n2 / 12 * ((n + 1) - tie_term))
    if sigma == 0:
        return u1, 1.0
    u = max(u1, n1 * n2 - u1)
    z = (u - mu - 0.5) / sigma
    return u1, min(1.0, 2 * norm_sf(z))


def mann_whitney_greater(x, y):
    """One-sided p for the alternative that x tends to be larger than y,
    asymptotic with tie and continuity correction, as
    scipy.stats.mannwhitneyu(alternative='greater', method='asymptotic')."""
    n1, n2 = len(x), len(y)
    u1, _ = mann_whitney(x, y)
    n = n1 + n2
    pooled = sorted(x + y)
    ties = [pooled.count(v) for v in sorted(set(pooled))]
    tie_term = sum(t ** 3 - t for t in ties) / (n * (n - 1))
    sigma = math.sqrt(n1 * n2 / 12 * ((n + 1) - tie_term))
    if sigma == 0:
        return 1.0
    return norm_sf((u1 - n1 * n2 / 2 - 0.5) / sigma)


def a12(x, y):
    """Vargha-Delaney A12: P(X > Y) + 0.5 P(X = Y)."""
    gt = sum(1 for a in x for b in y if a > b)
    eq = sum(1 for a in x for b in y if a == b)
    return (gt + 0.5 * eq) / (len(x) * len(y))


def kaplan_meier(samples):
    """samples: (time, observed) pairs. Returns [(time, survival)] at each
    event time, the Kaplan-Meier product-limit estimate."""
    times = sorted({t for t, e in samples if e})
    s = 1.0
    out = []
    for t in times:
        at_risk = sum(1 for u, _ in samples if u >= t)
        events = sum(1 for u, e in samples if u == t and e)
        s *= 1 - events / at_risk
        out.append((t, s))
    return out


def km_median(curve):
    for t, s in curve:
        if s <= 0.5:
            return t
    return None


Z95 = statistics.NormalDist().inv_cdf(0.975)


def km_band(samples, z=Z95):
    """samples: (time, observed) pairs. Returns [(time, survival, low,
    high)] at each event time: the Kaplan-Meier estimate and its pointwise
    log-log ("exponential Greenwood") band, as scipy.stats.ecdf's
    confidence_interval(method="log-log"). Where the estimate is 0 the band
    is 0."""
    times = sorted({t for t, e in samples if e})
    s = 1.0
    greenwood = 0.0
    out = []
    for t in times:
        at_risk = sum(1 for u, _ in samples if u >= t)
        events = sum(1 for u, e in samples if u == t and e)
        s *= 1 - events / at_risk
        if at_risk > events:
            greenwood += events / (at_risk * (at_risk - events))
        if s <= 0.0:
            out.append((t, 0.0, 0.0, 0.0))
            continue
        se = math.sqrt(greenwood) / abs(math.log(s))
        out.append((t, s, s ** math.exp(z * se), s ** math.exp(-z * se)))
    return out


def km_median_ci(band):
    """The median and its 95% interval from km_band: the first time at
    which the estimate, the band's lower edge (the interval's low end) and
    its upper edge (the high end) are each at most 0.5. None where no event
    time reaches it (beyond the budget)."""
    def first(index):
        for row in band:
            if row[index] <= 0.5:
                return row[0]
        return None
    return first(1), first(2), first(3)


def fmt(v):
    return "" if v is None else str(v)


def below(high, low):
    """An interval ending at `high` lies wholly before one starting at
    `low`; a missing `low` is beyond the budget, a missing `high` too."""
    if high is None:
        return False
    return low is None or high < low


def file_of(site):
    return site.split(":", 1)[0]


def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


# ---------- data ----------

def read_heartbeat(path):
    beats = []
    if os.path.exists(path):
        for line in open(path):
            line = line.strip()
            if line.isdigit():
                beats.append(int(line))
    return beats


def gap_inside(beats, start, end, limit=90):
    inside = [b for b in beats if start <= b <= end]
    if not inside:
        return True
    edges = [start] + inside + [end]
    return any(b - a > limit for a, b in zip(edges, edges[1:]))


def read_trial(d):
    meta = dict(kv.split("=", 1) for kv in open(os.path.join(d, "META")).read().split())
    hits = {}
    execs_end = 0
    panic = None
    for line in open(os.path.join(d, "log"), errors="replace"):
        m = CANARY.match(line.strip())
        if m:
            cid = int(m.group(1))
            hits.setdefault(cid, (int(m.group(2)), float(m.group(3))))
        m = EXECS.search(line)
        if m:
            execs_end = int(m.group(1))
        m = PANIC.search(line)
        if m and panic is None:
            panic = m.group(1)
    return meta, hits, execs_end, panic


def config_of(trial_id):
    return trial_id.split("-")[0]


def main(argv):
    trials_dir, heartbeat, canaries_tsv, seed_file, out = argv
    os.makedirs(out, exist_ok=True)
    names = {}
    for line in open(canaries_tsv):
        cid, path, lineno, text = line.rstrip("\n").split("\t", 3)
        names[int(cid)] = f"{path}:{lineno}"
    seeded = {int(x) for x in open(seed_file).read().split()}
    beats = read_heartbeat(heartbeat)

    rows = []
    for tid in sorted(os.listdir(trials_dir)):
        d = os.path.join(trials_dir, tid)
        if not os.path.exists(os.path.join(d, "DONE")):
            continue
        meta, hits, execs_end, panic = read_trial(d)
        start, end = int(meta["start"]), int(meta["end"])
        # Design v3: budgets are executions, so a machine pause cannot
        # change a result. A trial is valid when it ran its whole budget
        # (or stopped on a crash, which is itself a result); a heartbeat
        # gap only voids its seconds, which are a secondary record.
        budget = int(meta["runs"])
        rc = int(meta["rc"])
        valid = execs_end >= budget or rc != 0
        secs_valid = not gap_inside(beats, start, end)
        rows.append(dict(id=tid, config=config_of(tid), valid=valid, hits=hits,
                         execs=execs_end, rc=rc, panic=panic,
                         secs=end - start, secs_valid=secs_valid, budget=budget))

    with open(os.path.join(out, "trials.tsv"), "w") as f:
        f.write("trial\tconfig\tvalid\tcanaries\tcanaries_not_seeded\texecs\tbudget\t"
                "seconds\tseconds_valid\trc\tpanic\n")
        for r in rows:
            f.write(f"{r['id']}\t{r['config']}\t{r['valid']}\t{len(r['hits'])}\t"
                    f"{len(set(r['hits']) - seeded)}\t{r['execs']}\t{r['budget']}\t"
                    f"{r['secs']}\t{r['secs_valid']}\t{r['rc']}\t{r['panic'] or ''}\n")

    with open(os.path.join(out, "crashes.tsv"), "w") as f:
        f.write("trial\trc\tpanic\n")
        for r in rows:
            if r["rc"] != 0:
                f.write(f"{r['id']}\t{r['rc']}\t{r['panic'] or ''}\n")

    configs = sorted({r["config"] for r in rows})
    with open(os.path.join(out, "canaries.tsv"), "w") as f:
        f.write("config\tcanary\tsite\tseeded\ttrials_valid\ttriggered\t"
                "km_median_execs\tkm_survival_end\n")
        for c in configs:
            group = [r for r in rows if r["config"] == c and r["valid"]]
            for cid in sorted(names):
                samples = [(r["hits"][cid][0], True) if cid in r["hits"]
                           else (r["execs"], False) for r in group]
                curve = kaplan_meier(samples)
                med = km_median(curve)
                end_s = curve[-1][1] if curve else 1.0
                trig = sum(1 for _, e in samples if e)
                f.write(f"{c}\t{cid}\t{names[cid]}\t{cid in seeded}\t{len(group)}\t"
                        f"{trig}\t{'' if med is None else med}\t{end_s:.3f}\n")

    with open(os.path.join(out, "tests.tsv"), "w") as f:
        f.write("a\tb\tn_a\tn_b\tmedian_a\tmedian_b\tU_a\tp_two_sided\tA12\n")
        for a, b in (("C1", "C2"), ("C4", "C5")):
            xa = [len(r["hits"]) for r in rows if r["config"] == a and r["valid"]]
            xb = [len(r["hits"]) for r in rows if r["config"] == b and r["valid"]]
            if not xa or not xb:
                continue
            u, p = mann_whitney(xa, xb)
            f.write(f"{a}\t{b}\t{len(xa)}\t{len(xb)}\t{median(xa)}\t{median(xb)}\t"
                    f"{u}\t{p:.6g}\t{a12(xa, xb):.4f}\n")
            print(f"{a} vs {b}: U {u}, two-sided p {p:.3g}, one-sided p "
                  f"({a} larger) {mann_whitney_greater(xa, xb):.3g}, A12 {a12(xa, xb):.2f}")

    for c in configs:
        counts = sorted(len(r["hits"]) for r in rows if r["config"] == c and r["valid"])
        print(f"{c}: n {len(counts)}, canaries per trial {' '.join(map(str, counts))}, "
              f"median {median(counts)}")

    write_extra(out, rows, configs, names, seeded)
    manifest = [(canaries_tsv, "canaries"), (seed_file, "seed-triggered"),
                (heartbeat, "heartbeat")]
    for r in rows:
        d = os.path.join(trials_dir, r["id"])
        manifest += [(os.path.join(d, "META"), f"{r['id']}/META"),
                     (os.path.join(d, "log"), f"{r['id']}/log")]
    with open(os.path.join(out, "inputs.sha256"), "w") as f:
        for path, label in manifest:
            f.write(f"{digest(path)}  {label}\n")
    print(f"inputs {digest(os.path.join(out, 'inputs.sha256'))} ({len(manifest)} files)")
    print(f"{len(rows)} trials read, {sum(1 for r in rows if not r['valid'])} invalid")


def km_of(rows, config, cid):
    group = [r for r in rows if r["config"] == config and r["valid"]]
    samples = [(r["hits"][cid][0], True) if cid in r["hits"]
               else (r["execs"], False) for r in group]
    trig = sum(1 for _, e in samples if e)
    return trig, len(group), km_median_ci(km_band(samples))


def write_extra(out, rows, configs, names, seeded):
    with open(os.path.join(out, "km.tsv"), "w") as f:
        f.write("config\tcanary\tsite\tseeded\ttrials_valid\ttriggered\t"
                "median_execs\tci95_low\tci95_high\n")
        for c in configs:
            for cid in sorted(names):
                trig, n, (med, lo, hi) = km_of(rows, c, cid)
                f.write(f"{c}\t{cid}\t{names[cid]}\t{cid in seeded}\t{n}\t{trig}\t"
                        f"{fmt(med)}\t{fmt(lo)}\t{fmt(hi)}\n")

    # Q1's "sooner": per canary not triggered by the seeds alone, the
    # configuration whose median executions to trigger is lower. A canary
    # with a median in one configuration only was triggered sooner there;
    # one triggered in neither, or in fewer than half the trials of both,
    # is "undecided".
    # "disjoint" marks a verdict whose two 95% intervals do not overlap (a
    # missing low end is beyond the budget); it is descriptive, one
    # interval pair per canary, with no correction for the many canaries.
    tally = {}
    disjoint_tally = {}
    with open(os.path.join(out, "sooner.tsv"), "w") as f:
        f.write("canary\tsite\tseeded\tC1_triggered\tC1_median\tC1_ci95\t"
                "C2_triggered\tC2_median\tC2_ci95\tsooner\tdisjoint\n")
        for cid in sorted(names):
            t1, _, (m1, l1, h1) = km_of(rows, "C1", cid)
            t2, _, (m2, l2, h2) = km_of(rows, "C2", cid)
            if cid in seeded:
                verdict = "seeded"
            elif m1 is None and m2 is None:
                verdict = "undecided" if t1 or t2 else "neither"
            elif m2 is None or (m1 is not None and m1 < m2):
                verdict = "C1"
            elif m1 is None or m2 < m1:
                verdict = "C2"
            else:
                verdict = "equal"
            tally[verdict] = tally.get(verdict, 0) + 1
            disjoint = ((verdict == "C1" and below(h1, l2))
                        or (verdict == "C2" and below(h2, l1)))
            if disjoint:
                disjoint_tally[verdict] = disjoint_tally.get(verdict, 0) + 1
            f.write(f"{cid}\t{names[cid]}\t{cid in seeded}\t{t1}\t{fmt(m1)}\t"
                    f"{fmt(l1)}-{fmt(h1)}\t{t2}\t{fmt(m2)}\t{fmt(l2)}-{fmt(h2)}\t"
                    f"{verdict}\t{disjoint}\n")
    print("sooner (C1 vs C2): " + ", ".join(f"{k} {tally[k]}" for k in sorted(tally)))
    print("  with disjoint 95% intervals: "
          + ", ".join(f"{k} {disjoint_tally[k]}" for k in sorted(disjoint_tally)))

    triggered = {c: set() for c in configs}
    for r in rows:
        if r["valid"]:
            triggered[r["config"]] |= set(r["hits"])
    others = set().union(*(triggered[c] for c in configs if c != "C3"))
    empty_only = sorted(triggered.get("C3", set()) - others)
    short = set().union(*(triggered[c] for c in configs if not c.startswith("L")))
    long_only = sorted(set().union(*(triggered[c] for c in configs if c.startswith("L")))
                       - short)
    with open(os.path.join(out, "configs.tsv"), "w") as f:
        f.write("config\tcanaries_triggered\tcanaries\n")
        for c in configs:
            f.write(f"{c}\t{len(triggered[c])}\t{' '.join(map(str, sorted(triggered[c])))}\n")
        f.write(f"C3-only\t{len(empty_only)}\t{' '.join(map(str, empty_only))}\n")
        f.write(f"long-only\t{len(long_only)}\t{' '.join(map(str, long_only))}\n")
    every = set().union(*triggered.values())
    print(f"triggered in some trial {len(every)}, never {len(set(names) - every)}, "
          f"C3 only {len(empty_only)}, long runs only {len(long_only)}")

    files = sorted({file_of(s) for s in names.values()})
    with open(os.path.join(out, "by_file.tsv"), "w") as f:
        f.write("config\tfile\tsites\ttriggered_any\tmedian_per_trial\n")
        for c in configs:
            group = [r for r in rows if r["config"] == c and r["valid"]]
            for name in files:
                ids = {cid for cid, s in names.items() if file_of(s) == name}
                per = [len(ids & set(r["hits"])) for r in group]
                f.write(f"{c}\t{name}\t{len(ids)}\t{len(ids & triggered[c])}\t"
                        f"{median(per)}\n")


def median(v):
    s = sorted(v)
    n = len(s)
    return s[n // 2] if n % 2 else (s[n // 2 - 1] + s[n // 2]) / 2


def selftest():
    """Values checked against scipy separately (exp-0012-stats-check.py)."""
    u, p = mann_whitney([55, 50, 61, 58], [30, 31, 29, 40])
    assert u == 16 and p < 0.05, (u, p)
    assert a12([2, 2], [1, 2]) == 0.75
    curve = kaplan_meier([(1, True), (2, False), (3, True), (4, True)])
    assert [round(s, 4) for _, s in curve] == [0.75, 0.375, 0.0], curve
    band = km_band([(t, True) for t in range(1, 11)])
    assert all(lo <= s <= hi for _, s, lo, hi in band), band
    # Ten events at 1..10: S = 0.5 at 5; the band reaches 0.5 later above
    # and earlier below.
    med, lo, hi = km_median_ci(band)
    assert med == 5 and lo < 5 < hi, (med, lo, hi)
    assert km_median_ci(km_band([(9, False)] * 10)) == (None, None, None)
    print("selftest ok")


if __name__ == "__main__":
    if sys.argv[1:] == ["--selftest"]:
        selftest()
    elif len(sys.argv) == 6:
        main(sys.argv[1:])
    else:
        sys.exit(__doc__)
