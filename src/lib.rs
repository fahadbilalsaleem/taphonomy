//! Taphonomy: local-first digital evidence recovery and analysis.
//!
//! # Status
//!
//! Milestones M1 to M9 of ADR-0002 section 8, and M10 to M12 as ADR-0015,
//! ADR-0016 and ADR-0017 define them. Taphonomy can open a RAW evidence
//! image read-only and hash it, parse an MBR partition table, identify the
//! filesystem in a partition from its own structure, validate a FAT32 boot
//! sector against the extent it occupies, read every directory the root
//! reaches, a live one along its cluster chain and a deleted one from its
//! first cluster, find directories nothing names, identify the deleted
//! entries in each, including those beyond the terminator, recover the data
//! of an unfragmented deleted file, write it to a directory the operator
//! names, and validate it against a reference digest the operator supplies.
//!
//! Recovery extracts to memory and reports a digest. With `--output` the
//! same bytes are also written to a file in a directory the operator names,
//! which may not be the one holding the evidence, then read back and
//! compared with what was read: ADR-0015. Nothing is written without it,
//! and the evidence is never written.
//!
//! Deletion zeroes the cluster chain, so nothing in the evidence
//! establishes that a deleted file was unfragmented. The run its entry
//! implies is checked against the active FAT and refused where any cluster
//! in it is in use. A run of free clusters is the absence of contrary
//! evidence and is not treated as more than that.
//!
//! A digest match against a reference is the only evidence available that
//! the run read was the file's clusters, and it is evidence about that one
//! recovery rather than about the assumption: ADR-0013 section 8. A
//! reference is supplied by the operator and never discovered. Where none
//! is supplied nothing is validated, which is reported rather than left as
//! a silence.
//!
//! A reconstructed artifact carries the one confidence level ADR-0003's
//! model leaves reachable, reported once for the run rather than against
//! each entry: ADR-0014 Decision A and Appendix A.10.
//!
//! A run also reports what it did not analyse. A directory that could not
//! be read, a partition entry with no sectors or reaching past the end of
//! the image, and a partition whose filesystem is neither FAT32 nor exFAT
//! are not analysed, and each such gap is counted and stated: coverage is
//! complete, incomplete, or none. A run that analysed nothing past the
//! evidence digest exits 3, so a caller cannot take a digest and a silent
//! stdout for a whole analysis: ADR-0014 Appendix B.
//!
//! Coverage is about reach and not about correctness. A run can cover
//! everything it could reach and still recover, for a fragmented deleted
//! file, content that is not that file's.
//!
//! A deleted entry's long name is not decoded.
//!
//! # Safety model
//!
//! Source evidence is immutable. This crate contains no code that opens a
//! source path for writing. See `docs/SAFETY.md`.

pub mod analysis;
pub mod confidence;
pub mod error;
pub mod evidence;
pub mod exfat_boot;
pub mod exfat_directory;
pub mod extraction;
pub mod fat;
pub mod fat32;
pub mod fat_directory;
pub mod fat_recovery;
pub mod filesystem;
pub mod hash;
pub mod partition;
pub mod validation;

pub use error::Error;
pub use evidence::EvidenceFile;
pub use hash::{DIGEST_LEN, HashResult, Sha256Digest};
