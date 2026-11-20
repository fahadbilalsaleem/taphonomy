#!/usr/bin/env python3
"""Checks exp-0012-stats.py's statistics against scipy on random data.

Usage: exp-0012-stats-check.py SITE_DIR
SITE_DIR holds unpacked numpy and scipy wheels (scipy 1.18.1 was used);
neither is a project dependency. The script under test is loaded from this
file's directory. Checks the Mann-Whitney U and its p value, the
Kaplan-Meier estimate, its log-log band and the median's interval read
from that band. Exits 1 on any disagreement.
"""
import importlib.util
import os
import random
import sys

sys.path.insert(0, sys.argv[1])
import numpy as np  # noqa: E402
from scipy import stats  # noqa: E402

here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location(
    "exp0012stats", os.path.join(here, "exp-0012-stats.py"))
mine = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mine)


def first_at_most_half(times, values):
    for t, v in zip(times, values):
        if v <= 0.5:
            return t
    return None


rng = random.Random(20261113)
bad = 0
cases = 2000
for case in range(cases):
    n1, n2 = rng.randint(3, 30), rng.randint(3, 30)
    hi = rng.choice([5, 20, 100])  # small ranges give many ties
    x = [rng.randint(0, hi) for _ in range(n1)]
    y = [rng.randint(0, hi) for _ in range(n2)]
    u, p = mine.mann_whitney(x, y)
    ref = stats.mannwhitneyu(x, y, alternative="two-sided",
                             method="asymptotic", use_continuity=True)
    pr = 1.0 if np.isnan(ref.pvalue) else ref.pvalue
    if abs(u - ref.statistic) > 1e-9 or abs(p - pr) > 1e-9:
        bad += 1
        print("MWU", x, y, u, p, ref)
    ref1 = stats.mannwhitneyu(x, y, alternative="greater",
                              method="asymptotic", use_continuity=True)
    p1, pr1 = mine.mann_whitney_greater(x, y), ref1.pvalue
    pr1 = 1.0 if np.isnan(pr1) else pr1
    if abs(p1 - pr1) > 1e-9:
        bad += 1
        print("MWU greater", x, y, p1, ref1)

    times = [rng.randint(1, 50) for _ in range(rng.randint(2, 30))]
    obs = [rng.random() < 0.6 for _ in times]
    if not any(obs):
        obs[0] = True
    samples = list(zip(times, obs))
    data = stats.CensoredData(
        uncensored=[t for t, e in samples if e],
        right=[t for t, e in samples if not e])
    sf = stats.ecdf(data).sf
    curve = mine.kaplan_meier(samples)
    for t, s in curve:
        if abs(s - sf.evaluate(t)) > 1e-12:
            bad += 1
            print("KM", samples, t, s, sf.evaluate(t))
            break

    # The band where the estimate is strictly between 0 and 1 (scipy
    # leaves it undefined at 0), then the median's interval from each.
    band = mine.km_band(samples)
    ci = sf.confidence_interval(0.95, method="log-log")
    ev = [t for t, *_ in band]
    lo_ref = [float(ci.low.evaluate(t)) for t in ev]
    hi_ref = [float(ci.high.evaluate(t)) for t in ev]
    for (t, s, lo, hi), lr, hr in zip(band, lo_ref, hi_ref):
        if 0.0 < s < 1.0 and (abs(lo - lr) > 1e-9 or abs(hi - hr) > 1e-9):
            bad += 1
            print("BAND", samples, t, s, lo, hi, lr, hr)
            break
    lo_ref = [0.0 if np.isnan(v) else v for v in lo_ref]
    hi_ref = [0.0 if np.isnan(v) else v for v in hi_ref]
    ref_ci = (first_at_most_half(ev, [s for _, s, *_ in band]),
              first_at_most_half(ev, lo_ref), first_at_most_half(ev, hi_ref))
    if mine.km_median_ci(band) != ref_ci:
        bad += 1
        print("MEDIAN CI", samples, mine.km_median_ci(band), ref_ci)

print(f"{cases} cases, {bad} disagreements")
sys.exit(1 if bad else 0)
