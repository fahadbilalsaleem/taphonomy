//! Condition 2 of ADR-0019 section 4: whether a later claim refuses a
//! deleted exFAT file, as ADR-0019 Appendix C and ADR-0022 Appendix C state
//! it.
//!
//! Pure: it takes claims the reader has already followed and reads no
//! evidence, so the rule can be tested apart from any volume.
//!
//! A claimant is a usable deleted entry set, with its creation and
//! modification keys, or an orphan Stream Extension, which has no time. A
//! set is refused where another claimant shares a cluster with it and that
//! claimant has no time, or holds a creation or modification key that cannot
//! be compared with this set's creation key, or one at or after it. The set's
//! own modification key does not decide its own case: what shows that it
//! wrote last is its creation against every rival's two times.
//!
//! # Algorithm
//!
//! A comparison of every pair is quadratic, and a hostile volume can make
//! a million sets claim one cluster. Each claimant's runs are first merged,
//! so a chain that reaches a cluster twice counts once; the run boundaries
//! are then compressed into stretches, over which a tree adds a count to a
//! range and answers the largest count in a range, each in O(log R) for R
//! runs:
//!
//! 1. With every claimant counted, a set whose creation key cannot be
//!    compared is refused where any other claimant shares a stretch with
//!    it. For each offset byte, the claimants whose two keys both carry it
//!    are taken away; a set whose creation key carries it is then refused
//!    where any claimant but itself is left on one of its stretches.
//! 2. Starting from none, for each offset byte, the claimants whose two keys
//!    both carry it are added in order of their later key, latest first.
//!    Before a set whose creation key carries the byte is looked at, every
//!    one whose later key is at or after its creation key has been added; it
//!    is refused where any but itself lies on one of its stretches.
//!
//! The whole is O(R log R).

use std::cmp::Reverse;

/// A creation or modification time, as condition 2 compares it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Key {
    /// The UtcOffset byte, OffsetValid set. Only keys with the same byte are
    /// compared.
    offset: u8,

    /// The timestamp and its 10 ms increment, in an order that is the order
    /// of the local times they state.
    value: u64,
}

/// A time as a set stores it (sections 7.4.5, 7.4.6 and 7.4.8 to 7.4.10),
/// as a key, where it can be compared.
///
/// ADR-0019 Appendix C: only where OffsetValid is set and every field is in
/// range: DoubleSeconds 0 to 29, Minute 0 to 59, Hour 0 to 23, Month 1 to 12,
/// Day from 1 to the last day of that month, 10msIncrement 0 to 199.
///
/// The timestamp's fields are stored most significant first (year, month,
/// day, hour, minute, DoubleSeconds), and an increment of at most 1.99 s
/// stays inside its two seconds, so for keys in range the integer
/// `(timestamp << 8) | ten_ms` orders as the local times do.
pub(crate) fn key(timestamp: u32, ten_ms: u8, utc_offset: u8) -> Option<Key> {
    const OFFSET_VALID: u8 = 0x80;
    if utc_offset & OFFSET_VALID == 0 || ten_ms > 199 {
        return None;
    }
    let double_seconds = timestamp & 0x1F;
    let minute = (timestamp >> 5) & 0x3F;
    let hour = (timestamp >> 11) & 0x1F;
    let day = (timestamp >> 16) & 0x1F;
    let month = (timestamp >> 21) & 0x0F;
    let year = 1980 + (timestamp >> 25);
    if double_seconds > 29 || minute > 59 || hour > 23 || !(1..=12).contains(&month) {
        return None;
    }
    if !(1..=days_in_month(year, month)).contains(&day) {
        return None;
    }
    Some(Key {
        offset: utc_offset,
        value: ((timestamp as u64) << 8) | ten_ms as u64,
    })
}

/// Days in `month` (1 to 12) of `year`, by the Gregorian rule: the field
/// reaches 2107, so 2100, which is not a leap year, is in range.
fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// One claimant: a deleted set's two keys, or an orphan Stream Extension,
/// which has none.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Claimant {
    /// A deleted set, which is decided; an orphan is only a rival.
    pub(crate) timed: bool,
    pub(crate) created: Option<Key>,
    pub(crate) modified: Option<Key>,
}

impl Claimant {
    /// The offset byte both keys carry, where they carry the same one: the
    /// keys any set with that byte can be ordered against.
    fn class(&self) -> Option<u8> {
        match (self.timed, self.created, self.modified) {
            (true, Some(c), Some(m)) if c.offset == m.offset => Some(c.offset),
            _ => None,
        }
    }

    /// The later of the two keys, where both carry one byte.
    fn latest(&self) -> Option<u64> {
        self.class()?;
        Some(self.created?.value.max(self.modified?.value))
    }
}

/// Consecutive clusters one claimant claims.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ClaimRun {
    pub(crate) first: u32,
    pub(crate) clusters: u32,
    /// Index into the claimants.
    pub(crate) owner: u32,
}

/// What condition 2 decided for one deleted set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConditionTwo {
    /// No claimant sharing a cluster with it can be shown to be later, and
    /// every one can be ordered against it.
    Holds,

    /// A claimant sharing a cluster was created or last modified at the
    /// same time as this set's creation or later.
    Later,

    /// A claimant sharing a cluster has no time, or a time that cannot be
    /// compared with this set's creation: the two cannot be ordered.
    Unordered,
}

/// Decides condition 2 for every timed claimant: `None` for an orphan.
///
/// `runs` may be in any order, and one claimant's may overlap or repeat.
pub(crate) fn decide(claimants: &[Claimant], runs: &[ClaimRun]) -> Vec<Option<ConditionTwo>> {
    let mut verdicts: Vec<Option<ConditionTwo>> = claimants
        .iter()
        .map(|c| c.timed.then_some(ConditionTwo::Holds))
        .collect();

    let spans = Spans::new(claimants.len(), runs);
    let mut tree = Tree::new(spans.stretches());

    // Step 1: claimants that cannot be ordered against a set.
    for owner in 0..claimants.len() {
        spans.add(&mut tree, owner, 1);
    }
    // A set whose creation key cannot be compared, every set FatFs writes
    // among them, can be ordered against no one: it is refused wherever any
    // other claimant shares a stretch with it.
    for (owner, claimant) in claimants.iter().enumerate() {
        if claimant.timed && claimant.created.is_none() && spans.most(&tree, owner) > 1 {
            verdicts[owner] = Some(ConditionTwo::Unordered);
        }
    }
    for (byte, members) in by_created_offset(claimants) {
        let pure: Vec<usize> = members
            .iter()
            .copied()
            .filter(|&o| claimants[o].class() == Some(byte))
            .collect();
        for &o in &pure {
            spans.add(&mut tree, o, -1);
        }
        for &s in &members {
            // A set whose own two bytes differ is still counted: itself.
            let own = if claimants[s].class() == Some(byte) {
                0
            } else {
                1
            };
            if spans.most(&tree, s) > own {
                verdicts[s] = Some(ConditionTwo::Unordered);
            }
        }
        for &o in &pure {
            spans.add(&mut tree, o, 1);
        }
    }
    for owner in 0..claimants.len() {
        spans.add(&mut tree, owner, -1);
    }

    // Step 2: among claimants that can be ordered, a later one.
    for (byte, mut members) in by_created_offset(claimants) {
        let mut pure: Vec<usize> = members
            .iter()
            .copied()
            .filter(|&o| claimants[o].class() == Some(byte))
            .collect();
        pure.sort_by_key(|&o| Reverse(claimants[o].latest()));
        members.sort_by_key(|&s| Reverse(claimants[s].created.map(|k| k.value)));

        let mut added = 0;
        for &s in &members {
            let Some(created) = claimants[s].created else {
                continue;
            };
            while added < pure.len() && claimants[pure[added]].latest() >= Some(created.value) {
                spans.add(&mut tree, pure[added], 1);
                added += 1;
            }
            // A set whose two keys carry this byte was added: its later key
            // is at or after its creation.
            let own = if claimants[s].class() == Some(byte) {
                1
            } else {
                0
            };
            if verdicts[s] == Some(ConditionTwo::Holds) && spans.most(&tree, s) > own {
                verdicts[s] = Some(ConditionTwo::Later);
            }
        }
        for &o in &pure[..added] {
            spans.add(&mut tree, o, -1);
        }
    }

    verdicts
}

/// The timed claimants with a creation key, grouped by its offset byte, in
/// byte order.
fn by_created_offset(claimants: &[Claimant]) -> Vec<(u8, Vec<usize>)> {
    let mut groups: Vec<(u8, Vec<usize>)> = Vec::new();
    let mut order: Vec<(u8, usize)> = claimants
        .iter()
        .enumerate()
        .filter(|(_, c)| c.timed)
        .filter_map(|(i, c)| Some((c.created?.offset, i)))
        .collect();
    order.sort_unstable();
    for (byte, owner) in order {
        match groups.last_mut() {
            Some((b, members)) if *b == byte => members.push(owner),
            _ => groups.push((byte, vec![owner])),
        }
    }
    groups
}

/// Every claimant's runs, merged and turned into ranges of stretches.
struct Spans {
    /// `ranges[starts[o]..starts[o + 1]]` are claimant `o`'s, disjoint.
    starts: Vec<usize>,
    ranges: Vec<(usize, usize)>,
    stretches: usize,
}

impl Spans {
    fn new(claimants: usize, runs: &[ClaimRun]) -> Self {
        // Half-open cluster ranges, in u64 so first + clusters cannot wrap.
        let mut sorted: Vec<(u32, u64, u64)> = runs
            .iter()
            .filter(|r| r.clusters > 0)
            .map(|r| (r.owner, r.first as u64, r.first as u64 + r.clusters as u64))
            .collect();
        sorted.sort_unstable();

        let mut merged: Vec<(u32, u64, u64)> = Vec::with_capacity(sorted.len());
        for (owner, lo, hi) in sorted {
            match merged.last_mut() {
                Some((o, _, h)) if *o == owner && lo <= *h => *h = (*h).max(hi),
                _ => merged.push((owner, lo, hi)),
            }
        }

        let mut points: Vec<u64> = merged.iter().flat_map(|&(_, lo, hi)| [lo, hi]).collect();
        points.sort_unstable();
        points.dedup();
        let at = |p: u64| points.partition_point(|&q| q < p);

        let mut starts = Vec::with_capacity(claimants + 1);
        let mut ranges = Vec::with_capacity(merged.len());
        let mut next = 0;
        for owner in 0..claimants {
            starts.push(ranges.len());
            while next < merged.len() && merged[next].0 as usize == owner {
                ranges.push((at(merged[next].1), at(merged[next].2)));
                next += 1;
            }
        }
        starts.push(ranges.len());

        Self {
            starts,
            ranges,
            stretches: points.len().saturating_sub(1),
        }
    }

    fn stretches(&self) -> usize {
        self.stretches
    }

    fn add(&self, tree: &mut Tree, owner: usize, delta: i32) {
        for &(lo, hi) in &self.ranges[self.starts[owner]..self.starts[owner + 1]] {
            tree.add(lo, hi, delta);
        }
    }

    /// The most claimants counted on any stretch of `owner`'s, 0 for none.
    fn most(&self, tree: &Tree, owner: usize) -> i32 {
        self.ranges[self.starts[owner]..self.starts[owner + 1]]
            .iter()
            .map(|&(lo, hi)| tree.most(lo, hi))
            .max()
            .unwrap_or(0)
    }
}

/// Counts over stretches: add to a range, and the largest count in a range.
struct Tree {
    /// Per node: the largest count below it, its own addition included.
    most: Vec<i32>,
    /// Per node: what was added to the whole of its range.
    added: Vec<i32>,
    stretches: usize,
}

impl Tree {
    fn new(stretches: usize) -> Self {
        let nodes = 2 * stretches.next_power_of_two().max(1);
        Self {
            most: vec![0; nodes],
            added: vec![0; nodes],
            stretches,
        }
    }

    fn add(&mut self, lo: usize, hi: usize, delta: i32) {
        if lo < hi {
            self.add_in(1, 0, self.stretches, lo, hi, delta);
        }
    }

    fn add_in(&mut self, node: usize, l: usize, r: usize, lo: usize, hi: usize, delta: i32) {
        if hi <= l || r <= lo {
            return;
        }
        if lo <= l && r <= hi {
            self.most[node] += delta;
            self.added[node] += delta;
            return;
        }
        let mid = l + (r - l) / 2;
        self.add_in(2 * node, l, mid, lo, hi, delta);
        self.add_in(2 * node + 1, mid, r, lo, hi, delta);
        self.most[node] = self.added[node] + self.most[2 * node].max(self.most[2 * node + 1]);
    }

    fn most(&self, lo: usize, hi: usize) -> i32 {
        self.most_in(1, 0, self.stretches, lo, hi).unwrap_or(0)
    }

    fn most_in(&self, node: usize, l: usize, r: usize, lo: usize, hi: usize) -> Option<i32> {
        if hi <= l || r <= lo {
            return None;
        }
        if lo <= l && r <= hi {
            return Some(self.most[node]);
        }
        let mid = l + (r - l) / 2;
        let below = match (
            self.most_in(2 * node, l, mid, lo, hi),
            self.most_in(2 * node + 1, mid, r, lo, hi),
        ) {
            (Some(a), Some(b)) => a.max(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => return None,
        };
        Some(self.added[node] + below)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-03-01 10:00:00 plus `seconds`, as a timestamp.
    fn at(seconds: u32) -> u32 {
        let s = 10 * 3600 + seconds;
        (46 << 25)
            | (3 << 21)
            | (1 << 16)
            | ((s / 3600) << 11)
            | (((s / 60) % 60) << 5)
            | ((s % 60) / 2)
    }

    fn timed(created: Option<Key>, modified: Option<Key>) -> Claimant {
        Claimant {
            timed: true,
            created,
            modified,
        }
    }

    fn orphan() -> Claimant {
        Claimant {
            timed: false,
            created: None,
            modified: None,
        }
    }

    fn k(seconds: u32, offset: u8) -> Option<Key> {
        key(at(seconds), 0, offset)
    }

    fn run(owner: u32, first: u32, clusters: u32) -> ClaimRun {
        ClaimRun {
            first,
            clusters,
            owner,
        }
    }

    use ConditionTwo::{Holds, Later, Unordered};

    // --- keys --------------------------------------------------------------

    #[test]
    fn a_key_needs_offset_valid_and_every_field_in_range() {
        assert!(key(at(0), 0, 0x90).is_some());
        assert_eq!(key(at(0), 0, 0x10), None, "OffsetValid clear");
        assert_eq!(key(at(0), 0, 0x00), None, "FatFs writes 00h");
        assert_eq!(key(at(0), 200, 0x90), None, "10msIncrement 200");
        assert!(key(at(0), 199, 0x90).is_some());
        let fields = |y: u32, mo: u32, d: u32, h: u32, mi: u32, ds: u32| {
            ((y - 1980) << 25) | (mo << 21) | (d << 16) | (h << 11) | (mi << 5) | ds
        };
        assert!(key(fields(2026, 12, 31, 23, 59, 29), 0, 0x90).is_some());
        assert_eq!(
            key(fields(2026, 1, 1, 0, 0, 30), 0, 0x90),
            None,
            "DoubleSeconds 30"
        );
        assert_eq!(
            key(fields(2026, 1, 1, 0, 60, 0), 0, 0x90),
            None,
            "minute 60"
        );
        assert_eq!(key(fields(2026, 1, 1, 24, 0, 0), 0, 0x90), None, "hour 24");
        assert_eq!(key(fields(2026, 0, 1, 0, 0, 0), 0, 0x90), None, "month 0");
        assert_eq!(key(fields(2026, 13, 1, 0, 0, 0), 0, 0x90), None, "month 13");
        assert_eq!(key(fields(2026, 1, 0, 0, 0, 0), 0, 0x90), None, "day 0");
        assert_eq!(key(fields(2026, 4, 31, 0, 0, 0), 0, 0x90), None, "31 April");
        assert!(
            key(fields(2024, 2, 29, 0, 0, 0), 0, 0x90).is_some(),
            "2024 is leap"
        );
        assert_eq!(key(fields(2026, 2, 29, 0, 0, 0), 0, 0x90), None);
        assert!(
            key(fields(2000, 2, 29, 0, 0, 0), 0, 0x90).is_some(),
            "2000 is leap"
        );
        assert_eq!(
            key(fields(2100, 2, 29, 0, 0, 0), 0, 0x90),
            None,
            "2100 is not"
        );
    }

    #[test]
    fn keys_order_as_local_times_with_the_increment() {
        let a = key(at(0), 199, 0x90).unwrap();
        let b = key(at(2), 0, 0x90).unwrap();
        let c = key(at(2), 1, 0x90).unwrap();
        assert!(a.value < b.value && b.value < c.value);
    }

    // --- the rule ----------------------------------------------------------

    #[test]
    fn a_set_alone_holds() {
        let v = decide(&[timed(k(0, 0x90), k(10, 0x90))], &[run(0, 5, 3)]);
        assert_eq!(v, vec![Some(Holds)]);
    }

    /// The normal case: a set modified after it was created does not refuse
    /// itself, and the earlier of two is refused.
    #[test]
    fn the_later_of_two_holds_and_the_earlier_is_refused() {
        let c = [
            timed(k(0, 0x90), k(4, 0x90)),
            timed(k(8, 0x90), k(12, 0x90)),
        ];
        let v = decide(&c, &[run(0, 5, 3), run(1, 7, 3)]);
        assert_eq!(v, vec![Some(Later), Some(Holds)]);
    }

    /// A rival modified after this set was created, though created before.
    #[test]
    fn a_rival_modified_at_or_after_this_creation_refuses_it() {
        let c = [timed(k(4, 0x90), k(4, 0x90)), timed(k(0, 0x90), k(4, 0x90))];
        let v = decide(&c, &[run(0, 5, 1), run(1, 5, 1)]);
        assert_eq!(v, vec![Some(Later), Some(Later)]);
    }

    #[test]
    fn equal_creations_refuse_both() {
        let c = [timed(k(4, 0x90), k(4, 0x90)), timed(k(4, 0x90), k(4, 0x90))];
        let v = decide(&c, &[run(0, 5, 1), run(1, 5, 1)]);
        assert_eq!(v, vec![Some(Later), Some(Later)]);
    }

    #[test]
    fn sets_that_share_no_cluster_do_not_meet() {
        let c = [timed(k(0, 0x90), k(0, 0x90)), timed(k(8, 0x90), k(8, 0x90))];
        let v = decide(&c, &[run(0, 5, 3), run(1, 8, 3)]);
        assert_eq!(v, vec![Some(Holds), Some(Holds)]);
    }

    /// ADR-0022 Appendix C: a set's own modification under another offset
    /// byte does not decide its own case; it leaves its rival unordered.
    #[test]
    fn a_sets_own_modification_under_another_offset_does_not_refuse_it() {
        let c = [
            timed(k(0, 0x90), k(0, 0x90)),
            timed(k(8, 0x90), k(12, 0x94)),
        ];
        let v = decide(&c, &[run(0, 5, 1), run(1, 5, 1)]);
        assert_eq!(v, vec![Some(Unordered), Some(Holds)]);
    }

    #[test]
    fn a_rival_under_another_offset_or_none_leaves_the_set_unordered() {
        for rival in [
            timed(k(0, 0x94), k(0, 0x94)),
            timed(k(0, 0x90), k(0, 0x94)),
            timed(None, k(0, 0x90)),
            timed(k(0, 0x90), None),
        ] {
            let c = [timed(k(8, 0x90), k(8, 0x90)), rival];
            let v = decide(&c, &[run(0, 5, 1), run(1, 5, 1)]);
            assert_eq!(v[0], Some(Unordered), "{rival:?}");
        }
    }

    #[test]
    fn a_set_without_a_creation_key_is_unordered_wherever_it_meets_another() {
        let c = [timed(None, k(8, 0x90)), timed(k(0, 0x90), k(0, 0x90))];
        assert_eq!(decide(&c, &[run(0, 5, 1), run(1, 6, 1)])[0], Some(Holds));
        assert_eq!(
            decide(&c, &[run(0, 5, 1), run(1, 5, 1)])[0],
            Some(Unordered)
        );
    }

    #[test]
    fn an_orphan_refuses_every_set_it_meets_and_is_not_decided() {
        let c = [
            timed(k(8, 0x90), k(8, 0x90)),
            orphan(),
            timed(k(9, 0x90), k(9, 0x90)),
        ];
        let v = decide(&c, &[run(0, 5, 2), run(1, 6, 1), run(2, 20, 1)]);
        assert_eq!(v, vec![Some(Unordered), None, Some(Holds)]);
    }

    /// A chain that reaches cluster 5 twice, and runs given out of order,
    /// count once: the set does not meet itself.
    #[test]
    fn a_claimant_meeting_its_own_clusters_again_counts_once() {
        let c = [timed(k(0, 0x90), k(4, 0x90))];
        let v = decide(
            &c,
            &[run(0, 5, 2), run(0, 9, 1), run(0, 5, 1), run(0, 4, 3)],
        );
        assert_eq!(v, vec![Some(Holds)]);
    }

    #[test]
    fn no_runs_and_no_claimants_are_handled() {
        assert_eq!(decide(&[], &[]), Vec::<Option<ConditionTwo>>::new());
        let c = [timed(k(0, 0x90), k(0, 0x90)), timed(k(0, 0x90), k(0, 0x90))];
        assert_eq!(decide(&c, &[]), vec![Some(Holds), Some(Holds)]);
    }

    #[test]
    fn the_last_cluster_of_a_full_heap_does_not_wrap() {
        let c = [timed(k(0, 0x90), k(0, 0x90)), timed(k(8, 0x90), k(8, 0x90))];
        let v = decide(&c, &[run(0, u32::MAX - 1, 1), run(1, u32::MAX - 1, 1)]);
        assert_eq!(v, vec![Some(Later), Some(Holds)]);
    }

    // --- against a direct pairwise check -----------------------------------

    /// The rule as ADR-0022 Appendix C states it, pair by pair.
    fn pairwise(claimants: &[Claimant], runs: &[ClaimRun]) -> Vec<Option<ConditionTwo>> {
        let clusters = |o: usize| -> std::collections::BTreeSet<u64> {
            runs.iter()
                .filter(|r| r.owner as usize == o)
                .flat_map(|r| r.first as u64..r.first as u64 + r.clusters as u64)
                .collect()
        };
        let all: Vec<_> = (0..claimants.len()).map(clusters).collect();
        (0..claimants.len())
            .map(|s| {
                let me = claimants[s];
                if !me.timed {
                    return None;
                }
                let mut verdict = Holds;
                for (o, rival) in claimants.iter().enumerate() {
                    if o == s || all[s].is_disjoint(&all[o]) {
                        continue;
                    }
                    let keys = if rival.timed {
                        vec![rival.created, rival.modified]
                    } else {
                        vec![None]
                    };
                    for t in keys {
                        match (me.created, t) {
                            (Some(c), Some(t)) if c.offset == t.offset => {
                                if t.value >= c.value && verdict == Holds {
                                    verdict = Later;
                                }
                            }
                            _ => verdict = Unordered,
                        }
                    }
                }
                Some(verdict)
            })
            .collect()
    }

    /// A small deterministic generator, so the comparison needs no
    /// dependency and every failure can be replayed from its case number.
    struct XorShift(u64);

    impl XorShift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// Times drawn from few values and two offset bytes, with keys that
    /// cannot be compared, so ties, differing bytes and modification after
    /// creation all occur; runs drawn on few clusters, so they overlap, and
    /// some repeat within one claimant.
    #[test]
    fn the_sweep_agrees_with_a_direct_pairwise_check() {
        let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
        let mut verdicts = [0u64; 3];
        for case in 0..20_000 {
            let n = 1 + rng.below(8) as usize;
            let draw_key = |rng: &mut XorShift| match rng.below(10) {
                0 => None,
                1 => k(rng.below(4) as u32 * 2, 0x94),
                _ => k(rng.below(4) as u32 * 2, 0x90),
            };
            let claimants: Vec<Claimant> = (0..n)
                .map(|_| {
                    if rng.below(10) == 0 {
                        orphan()
                    } else {
                        let created = draw_key(&mut rng);
                        let modified = draw_key(&mut rng);
                        timed(created, modified)
                    }
                })
                .collect();
            let mut runs = Vec::new();
            for owner in 0..n {
                for _ in 0..1 + rng.below(3) {
                    runs.push(run(
                        owner as u32,
                        2 + rng.below(20) as u32,
                        rng.below(4) as u32,
                    ));
                }
            }
            let fast = decide(&claimants, &runs);
            assert_eq!(fast, pairwise(&claimants, &runs), "case {case}");
            for v in fast.into_iter().flatten() {
                verdicts[v as usize] += 1;
            }
        }
        // Each verdict occurred often enough for the comparison to mean
        // something.
        assert!(verdicts.iter().all(|&count| count > 1000), "{verdicts:?}");
    }
}
