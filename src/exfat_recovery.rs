//! Extracting one recovered exFAT deleted set (`ADR-0022` Decision H).
//!
//! Kept apart from FAT32's recovery (`CLAUDE.md` section 10): an exFAT set's
//! extent is read from its surviving entry set and chain, not inferred, so
//! its bytes come from [`ExFatReader::read_content`] and not from a run
//! computed from a size. What the two share, hashing and writing an
//! artifact, is `extraction`'s, and neither reimplements it.
//!
//! Recovery logic is here, not in the analysis or the command line
//! (`CLAUDE.md` section 11): the analysis decides which sets reach this
//! module, and the command line prints what it returns.

use std::fmt;
use std::path::PathBuf;

use crate::evidence::EvidenceReader;
use crate::exfat_directory::{ExFatContentError, ExFatFile, ExFatPosition, ExFatReader};
use crate::extraction::{ArtifactWriter, Destination, Extraction, Leftover};

/// Why a recovered exFAT set could not be extracted.
///
/// Every case voids the digest: the hasher did not see every byte, and any
/// file this run created for it was removed, or is named here where it
/// could not be (`ADR-0015` Decision G and section 9).
#[derive(Debug)]
pub enum ExFatExtractionError {
    /// Reading the content failed: the evidence could not be read, or the
    /// allocation, checked again before any byte was read, is not what it
    /// was when the set was decided.
    Content(ExFatContentError),

    /// Reading the content failed part way through, and the partial file
    /// could not be removed. The content failure is the cause and this
    /// error's source; the removal failure is a statement about the
    /// destination, carried beside it.
    PartialLeft {
        /// The content failure.
        cause: ExFatContentError,
        /// The partial file that remains in the destination.
        path: PathBuf,
        /// Why it could not be removed.
        removal: String,
    },
}

impl fmt::Display for ExFatExtractionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Content(e) => write!(f, "{e}"),
            Self::PartialLeft {
                cause,
                path,
                removal,
            } => write!(
                f,
                "{cause}; partial file {} left: {removal}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ExFatExtractionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Content(e) | Self::PartialLeft { cause: e, .. } => Some(e),
        }
    }
}

/// Reads a recovered set's content, hashes it and, where `destination` is
/// given, writes it (`ADR-0022` Decision H).
///
/// The file is named `p<partition>-c<cluster>-s<slot>-first-<n>.bin` from
/// the partition in `destination`, the File entry's `position`, and the
/// set's first cluster: the cluster and slot are always the position's,
/// whatever `destination` carries, so no other set's name can be taken. No
/// byte of the set's name reaches the path.
///
/// The bytes are [`ExFatReader::read_content`]'s, which checks the
/// allocation again before reading, and hands over zeros past
/// ValidDataLength without reading them (`ADR-0019` Decision F). Any error
/// ends in `abort`, which removes only a file this call created: one that
/// already existed was never opened, and is reported as `Output::Exists`.
pub fn extract<R: EvidenceReader>(
    reader: &mut ExFatReader<'_, R>,
    position: ExFatPosition,
    file: &ExFatFile,
    destination: Option<Destination<'_>>,
) -> Result<Extraction, ExFatExtractionError> {
    let path = destination.map(|d| {
        Destination {
            cluster: position.cluster,
            slot: position.slot,
            ..d
        }
        .path(file.first_cluster)
    });
    let mut writer = ArtifactWriter::begin(path);

    if let Err(cause) = reader.read_content(file, &mut |bytes| writer.push(bytes)) {
        return Err(match writer.abort() {
            Some(Leftover { path, removal }) => ExFatExtractionError::PartialLeft {
                cause,
                path,
                removal,
            },
            None => ExFatExtractionError::Content(cause),
        });
    }

    // No slack: `read_content` hands over exactly DataLength bytes and reads
    // nothing of the last cluster past them.
    Ok(writer.finish(0))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::Error;
    use crate::evidence::tests::MemoryImage;
    use crate::exfat_directory::Limits;
    use crate::exfat_directory::tests::{Vol, cluster_data, extent, fill_clusters};
    use crate::extraction::Output;
    use crate::hash::Sha256Hasher;

    /// An empty directory of this test's own, `exfat-recovery-{name}` in the
    /// target directory's `tmp`, found from the test binary's path
    /// (`target/<profile>/deps/<binary>`). Unit tests get no
    /// `CARGO_TARGET_TMPDIR`. The name is fixed and the directory emptied
    /// first, so runs never add to it, and it goes with the target
    /// directory.
    pub(crate) fn scratch(name: &str) -> PathBuf {
        let exe = std::env::current_exe().expect("the test binary's path");
        let target = exe
            .ancestors()
            .nth(3)
            .expect("the test binary lies in target/<profile>/deps");
        let dir = target.join("tmp").join(format!("exfat-recovery-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("creating the scratch directory");
        dir
    }

    fn file(first_cluster: u32, data_length: u64, valid_data_length: u64) -> ExFatFile {
        ExFatFile {
            // A name that would escape the directory, were any byte of it
            // used.
            name: "../../escaped".encode_utf16().collect(),
            attributes: 0x20,
            first_cluster,
            data_length,
            valid_data_length,
            no_fat_chain: false,
        }
    }

    const POSITION: ExFatPosition = ExFatPosition {
        cluster: 7,
        slot: 9,
    };

    /// A destination whose own cluster and slot are not the set's.
    fn destination(directory: &Path) -> Destination<'_> {
        Destination {
            directory,
            partition: 1,
            cluster: 999,
            slot: 999,
        }
    }

    /// Clusters 30 and 31 in a chain, holding [`cluster_data`]'s bytes.
    fn vol() -> Vol {
        let mut vol = Vol::new();
        vol.chain(&[30, 31]);
        fill_clusters(&mut vol, &[30, 31]);
        vol
    }

    fn digest_of(bytes: &[u8]) -> crate::hash::Sha256Digest {
        let mut hasher = Sha256Hasher::new();
        hasher.update(bytes);
        hasher.finish().digest
    }

    fn content_read(vol: &mut Vol, file: &ExFatFile) -> Vec<u8> {
        let boot = vol.boot.clone();
        let mut reader = ExFatReader::new(&mut vol.image, &boot, extent(), Limits::DEFAULT);
        let mut bytes = Vec::new();
        reader
            .read_content(file, &mut |b| bytes.extend_from_slice(b))
            .expect("the content reads");
        bytes
    }

    fn extracted<R: EvidenceReader>(
        image: &mut R,
        vol_boot: &crate::exfat_boot::ExFatBootSector,
        file: &ExFatFile,
        destination: Option<Destination<'_>>,
    ) -> Result<Extraction, ExFatExtractionError> {
        let mut reader = ExFatReader::new(image, vol_boot, extent(), Limits::DEFAULT);
        extract(&mut reader, POSITION, file, destination)
    }

    /// The file written is named by the entry's position, never by its
    /// name, and holds exactly what `read_content` hands over: 1000 bytes of
    /// two clusters, no slack.
    #[test]
    fn the_bytes_written_are_read_contents_under_the_positions_name() {
        let dir = scratch("bytes");
        let mut vol = vol();
        let file = file(30, 1000, 1000);
        let expected = content_read(&mut vol, &file);
        assert_eq!(
            expected,
            [cluster_data(30), cluster_data(31)].concat()[..1000]
        );

        let boot = vol.boot.clone();
        let extraction =
            extracted(&mut vol.image, &boot, &file, Some(destination(&dir))).expect("extracts");
        let path = dir.join("p1-c7-s9-first-30.bin");
        assert_eq!(
            extraction.output,
            Some(Output::Written {
                path: path.clone(),
                readback: digest_of(&expected),
            })
        );
        assert_eq!(fs::read(&path).expect("written"), expected);
        assert_eq!(extraction.digest, digest_of(&expected));
        assert_eq!(extraction.bytes_hashed, 1000);
        assert_eq!(extraction.slack_bytes, 0);
        assert_eq!(fs::read_dir(&dir).expect("listed").count(), 1);
    }

    /// Past ValidDataLength the file holds zeros, though the cluster does
    /// not.
    #[test]
    fn bytes_past_valid_data_length_are_written_as_zeros() {
        let dir = scratch("zeros");
        let mut vol = vol();
        let file = file(30, 1024, 600);
        let boot = vol.boot.clone();
        extracted(&mut vol.image, &boot, &file, Some(destination(&dir))).expect("extracts");

        let written = fs::read(dir.join("p1-c7-s9-first-30.bin")).expect("written");
        assert_eq!(written.len(), 1024);
        let data = [cluster_data(30), cluster_data(31)].concat();
        assert_eq!(written[..600], data[..600]);
        assert!(written[600..].iter().all(|&b| b == 0));
    }

    /// Evidence that fails every read of one cluster.
    struct Failing<'a> {
        image: &'a mut MemoryImage,
        cluster: u64,
    }

    impl EvidenceReader for Failing<'_> {
        fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
            if offset == self.cluster {
                return Err(Error::from_io(
                    Path::new("<memory>"),
                    std::io::Error::other("the evidence failed"),
                ));
            }
            self.image.read_exact_at(offset, buf)
        }
    }

    /// The second cluster cannot be read: the partial file, which held the
    /// first cluster's bytes, is removed, and the error says why
    /// (`ADR-0022` section 14 item 5).
    #[test]
    fn an_evidence_error_part_way_through_removes_the_partial_file() {
        let dir = scratch("partial");
        let mut vol = vol();
        let boot = vol.boot.clone();
        let mut failing = Failing {
            image: &mut vol.image,
            cluster: Vol::cluster_offset(31),
        };

        let error = extracted(
            &mut failing,
            &boot,
            &file(30, 1024, 1024),
            Some(destination(&dir)),
        )
        .expect_err("the evidence failed");
        assert!(
            matches!(
                error,
                ExFatExtractionError::Content(ExFatContentError::Evidence(_))
            ),
            "{error:?}"
        );
        assert!(!error.to_string().is_empty());
        assert!(std::error::Error::source(&error).is_some());
        assert_eq!(fs::read_dir(&dir).expect("listed").count(), 0);
    }

    /// A file of that name is there already: it is left as it was, and the
    /// digest still stands.
    #[test]
    fn an_existing_file_is_left_alone() {
        let dir = scratch("exists");
        let path = dir.join("p1-c7-s9-first-30.bin");
        fs::write(&path, b"the operator's").expect("planting a file");
        let mut vol = vol();
        let boot = vol.boot.clone();

        let extraction = extracted(
            &mut vol.image,
            &boot,
            &file(30, 1000, 1000),
            Some(destination(&dir)),
        )
        .expect("extracts");
        assert_eq!(
            extraction.output,
            Some(Output::Exists { path: path.clone() })
        );
        assert_eq!(fs::read(&path).expect("still there"), b"the operator's");
    }

    /// The evidence fails with a file of that name already there: the run
    /// did not create it, so it is not removed.
    #[test]
    fn a_file_the_run_did_not_create_is_never_removed() {
        let dir = scratch("not-ours");
        let path = dir.join("p1-c7-s9-first-30.bin");
        fs::write(&path, b"the operator's").expect("planting a file");
        let mut vol = vol();
        let boot = vol.boot.clone();
        let mut failing = Failing {
            image: &mut vol.image,
            cluster: Vol::cluster_offset(31),
        };

        extracted(
            &mut failing,
            &boot,
            &file(30, 1024, 1024),
            Some(destination(&dir)),
        )
        .expect_err("the evidence failed");
        assert_eq!(fs::read(&path).expect("still there"), b"the operator's");
    }

    /// Without a destination nothing is written, and the digest is the
    /// content's.
    #[test]
    fn without_a_destination_the_content_is_hashed_and_nothing_written() {
        let mut vol = vol();
        let file = file(30, 1000, 1000);
        let expected = content_read(&mut vol, &file);
        let boot = vol.boot.clone();

        let extraction = extracted(&mut vol.image, &boot, &file, None).expect("extracts");
        assert_eq!(extraction.output, None);
        assert_eq!(extraction.digest, digest_of(&expected));
    }

    /// The error carries its cause, and a left partial file, in its text.
    #[test]
    fn a_partial_file_left_is_reported_beside_its_cause() {
        let error = ExFatExtractionError::PartialLeft {
            cause: ExFatContentError::Directory,
            path: PathBuf::from("left.bin"),
            removal: "permission denied".to_string(),
        };
        let text = error.to_string();
        assert!(
            text.contains("left.bin") && text.contains("permission denied"),
            "{text}"
        );
        assert!(std::error::Error::source(&error).is_some());
    }
}
