//! What a recovered artifact's classification states.
//!
//! `ADR-0003` defines six confidence levels. `ADR-0014` section 3 records
//! that three of them have no reachable code path in this tool, that one
//! misdescribes the case it would be assigned to, and that one of M8's
//! three validator outcomes has no level at all. `ADR-0014` Decision E
//! implements only what is reachable, on `ADR-0003` section 8's reasoning
//! that implementing a level no code path can produce is speculative.
//!
//! `ADR-0022` Decision A makes a second level reachable, recorded in
//! `ADR-0003` Appendix C: an exFAT artifact's extent is read from surviving
//! metadata, so it is `UNVERIFIED`, and a FAT32 artifact stays
//! `RECONSTRUCTED`.
//!
//! This module is filesystem-independent, as `validation` is. A level
//! states how content was obtained and names no filesystem. It is
//! kept apart from `validation` because `ADR-0003` section 5 and
//! `SAFETY.md` section 14 require the two axes not be merged: this module
//! says how the bytes were obtained, `validation` says what comparing them
//! against a reference established.

use std::fmt;

/// How a recovered artifact's content was obtained.
///
/// `ADR-0003` section 4.1 assigns exactly one level to every artifact.
/// `ADR-0014` Decision A: a FAT32 artifact's level is always
/// [`Confidence::Reconstructed`]; `ADR-0022` Decision A: an exFAT
/// artifact's is always [`Confidence::Unverified`]. In both, a digest match
/// is reported on the validator axis rather than raising the level.
///
/// Two variants. The other four levels are absent because no code path
/// produces them, not because they were overlooked: `ADR-0014` section 3
/// names each and the evidence it would require, section 11 records what
/// would make another reachable, and `ADR-0003` Appendix C records the
/// second.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Confidence {
    /// Content assembled using inference rather than surviving filesystem
    /// metadata.
    ///
    /// `ADR-0003` section 3.2's definition. EXP-0003 measured that deletion
    /// zeroes the cluster chain in every FAT, so a deleted entry states a
    /// first cluster and a size and nothing about the extent between them.
    /// The run is computed from those two values on an assumption of
    /// contiguity the evidence cannot confirm. The CFTT specification's
    /// term for this is estimating content: recovering beyond what residual
    /// metadata explicitly identifies, recorded in `ADR-0013` Appendix A.
    ///
    /// `ADR-0003` section 4.5: this names the method used and is not a
    /// position in an ordering. The validator result is recorded
    /// separately, which is what `validation::Validation` carries.
    Reconstructed,

    /// Content read from an extent surviving filesystem metadata states,
    /// with no format-aware validator to check it.
    ///
    /// `ADR-0003` section 3.2's level, read as `ADR-0022` section 3 records:
    /// extraction succeeded, and the validator section 3.2 means, the
    /// format-aware one `STRUCTURALLY_VALID` requires, does not exist. An
    /// exFAT deleted set's entry set, chain and lengths survive deletion
    /// (EXP-0009, EXP-0011), so its extent is read, not inferred. That
    /// still does not make the bytes the file's: metadata passing every
    /// check can lie over stale data, which is why a match never raises
    /// this level (`ADR-0022` Decision A).
    Unverified,
}

impl Confidence {
    /// The level an artifact carries when its extent was inferred rather
    /// than read from surviving metadata.
    ///
    /// Takes no argument. Under `ADR-0014` Decision A the level follows
    /// from the method, which is fixed for every extraction this tool
    /// performs, and not from any value a caller could supply. The name
    /// states the premise so that a path which one day establishes an
    /// extent from evidence calls something else rather than reusing this.
    ///
    /// EXP-0004 measured why the premise holds even where a recovery is
    /// correct: on `fat32-fragmented-deleted.img` three of five deleted
    /// entries recover content that is not their own file's, and the two
    /// that are correct are correct because nothing reused their clusters,
    /// which is not a property of the method.
    pub const fn of_inferred_run() -> Self {
        Self::Reconstructed
    }

    /// The level an artifact carries when its extent was read from
    /// surviving filesystem metadata, and no format-aware validator exists.
    ///
    /// Takes no argument, for the reason [`Self::of_inferred_run`] gives:
    /// the level follows from the method. `ADR-0022` Decision A.
    pub const fn of_metadata_extent() -> Self {
        Self::Unverified
    }
}

/// Renders the level as `ADR-0003` section 3.2 names it.
///
/// The name belongs to the type rather than to a format string at the call
/// site, so that a reader of the output and a reader of `ADR-0003` are
/// reading the same word for the same reason.
impl fmt::Display for Confidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reconstructed => f.write_str("RECONSTRUCTED"),
            Self::Unverified => f.write_str("UNVERIFIED"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ADR-0014` Decision A. The premise the constructor names is the one
    /// every extraction satisfies, so it yields the one reachable level.
    #[test]
    fn an_inferred_run_is_reconstructed() {
        assert_eq!(Confidence::of_inferred_run(), Confidence::Reconstructed);
    }

    /// The rendered name is `ADR-0003` section 3.2's, so that output and
    /// ADR agree on the word.
    #[test]
    fn the_rendered_name_is_the_adrs() {
        assert_eq!(Confidence::Reconstructed.to_string(), "RECONSTRUCTED");
    }

    /// `ADR-0022` Decision A: an extent read from metadata, with no
    /// format-aware validator, is `UNVERIFIED`, rendered as `ADR-0003`
    /// section 3.2 names it.
    #[test]
    fn a_metadata_extent_is_unverified() {
        assert_eq!(Confidence::of_metadata_extent(), Confidence::Unverified);
        assert_eq!(Confidence::Unverified.to_string(), "UNVERIFIED");
    }
}
