//! Writing a recovered artifact: the part of ADR-0015 that does not depend
//! on which filesystem the bytes came from.
//!
//! `ArtifactWriter` takes the artifact's bytes in whatever pieces the
//! caller has them, hashes every one, and, where a destination was asked
//! for, writes the same bytes in the same pass. The file is created with
//! `create_new`, flushed, closed and read back, and a partial file left by
//! a failed read is removed. A filesystem's recovery code decides which
//! bytes make up the artifact and in what order; it does not reimplement
//! any of this.
//!
//! # Which failures stop a run
//!
//! ADR-0015 Decision G. A read failure in the evidence voids the
//! extraction: the hasher did not see every byte, so there is no digest and
//! the caller calls `ArtifactWriter::abort`. A failure on the destination
//! does not. The run read the evidence and hashed it, and a full disk says
//! nothing about the evidence, so the digest is reported together with a
//! statement that nothing was delivered. Everything in [`Output`] is a
//! statement about the destination.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::hash::{Sha256Digest, Sha256Hasher};

/// Size of the buffer the read-back reads through.
///
/// The chunk size does not affect the digest, so it is not tied to any
/// filesystem's cluster size.
const READBACK_BYTES: usize = 64 * 1024;

/// Where an extracted artifact is to be written.
///
/// The directory is the caller's, and ADR-0015 Decision B as its Appendix
/// A.3 corrects it requires the caller to have established, before the
/// evidence was opened, that the directory is not the one holding the
/// evidence. Sharing a filesystem with an image file is permitted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Destination<'a> {
    /// Directory the artifact is created in.
    pub directory: &'a Path,

    /// MBR entry, 1 to 4, of the partition holding the volume.
    pub partition: u8,

    /// Directory cluster the entry being recovered was read from.
    pub cluster: u32,

    /// Slot of that entry within its directory cluster, counting from zero.
    pub slot: usize,
}

impl Destination<'_> {
    /// The path this artifact is written to.
    ///
    /// ADR-0015 Decision D, with the location ADR-0016 Decision F adds.
    /// Composed from integers the run established, so no byte of evidence
    /// reaches the path. The entry's cluster and slot locate it uniquely on
    /// the volume, where a slot alone repeats in every directory cluster,
    /// and the partition makes that location unique on the image, because
    /// cluster numbers restart in every volume.
    /// `SECURITY.md` section 7
    /// requires that a recovered filename never allow a write outside the
    /// destination; a name that cannot contain a separator, a `..` or a
    /// leading `/` has no such failure to get wrong.
    ///
    /// The extension states that the content was not identified. No
    /// validator exists, and ADR-0003 section 3.1 makes a level a property
    /// of an artifact a validator has seen.
    pub fn path(&self, first_cluster: u32) -> PathBuf {
        self.directory.join(format!(
            "p{}-c{}-s{}-first-{first_cluster}.bin",
            self.partition, self.cluster, self.slot
        ))
    }
}

/// What became of an artifact the caller asked to be written.
///
/// Every variant is a statement about the destination. A read failure in
/// the evidence never reaches here; it is returned as an error.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Output {
    /// Written, then read back and hashed.
    ///
    /// `readback` is the digest of the file on disk, not of what was sent
    /// to the kernel. ADR-0015 Decision H leaves the comparison against
    /// [`Extraction::digest`] to the caller to report: a difference is a
    /// finding about the destination, not an error here.
    Written {
        /// Path written.
        path: PathBuf,
        /// Digest of the bytes read back from that path.
        readback: Sha256Digest,
    },

    /// Written and flushed, and the file could not be read back.
    ///
    /// The file is left in place. Every write and the flush succeeded, so
    /// it may be sound, and removing a possibly-recovered artifact because
    /// the destination could not be re-read would destroy more than it
    /// protects. A failed flush is not this case; it is [`Output::Failed`].
    /// ADR-0015 section 10 does not cover this case and is owed an appendix
    /// recording it.
    Unverified {
        /// Path written.
        path: PathBuf,
        /// Why the read back failed.
        message: String,
    },

    /// A file of that name existed already and was not touched.
    ///
    /// ADR-0015 Decision C, which `SAFETY.md` section 15 requires: the
    /// default behaviour preserves existing output.
    Exists {
        /// Path that was left alone.
        path: PathBuf,
    },

    /// The file could not be created, so nothing was written.
    ///
    /// Distinct from [`Output::Failed`] because nothing reached the
    /// destination and nothing is left to remove. `CLAUDE.md` section 14
    /// requires a permission failure to be told apart from an I/O failure,
    /// and an unwritable directory is the ordinary cause of this one.
    NotCreated {
        /// Path that was attempted.
        path: PathBuf,
        /// Why it could not be created.
        message: String,
    },

    /// The file was created, and a write or the flush failed.
    ///
    /// ADR-0015 Decision G: the partial file is removed, because a
    /// truncated file on disk cannot be told apart from a short file that
    /// was recovered whole, and `SAFETY.md` section 12 forbids a failure
    /// becoming a silent partial success. A failed flush counts: after a
    /// writeback error the kernel may already have discarded the pages, so
    /// the file cannot be taken to hold what was written.
    Failed {
        /// Path that was attempted.
        path: PathBuf,
        /// Why it failed.
        message: String,
        /// Whether the partial file was successfully removed.
        ///
        /// Reported rather than assumed. A removal can fail too, and the
        /// tool states what it knows rather than claiming a cleanliness it
        /// did not achieve.
        removed: bool,
    },
}

/// The result of reading a run's content.
///
/// The content is not here. Extraction hashes what it read and reports;
/// where a [`Destination`] was supplied the bytes also went to a file, and
/// `output` says what became of it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Extraction {
    /// Digest of the file's bytes, and of nothing else.
    pub digest: Sha256Digest,

    /// Number of bytes hashed, counted by the hasher rather than assumed.
    ///
    /// Equal to the size the entry declared. It is reported so that the
    /// digest is never separated from a statement of what it covers.
    pub bytes_hashed: u64,

    /// Bytes of the final cluster that were read and not hashed.
    ///
    /// File slack. It belongs to whatever held the cluster before this file
    /// and is evidence in its own right, so its size is reported rather
    /// than silently dropped. Recovering it is a separate capability and is
    /// not this milestone's.
    pub slack_bytes: u32,

    /// What became of the written file, where one was asked for.
    ///
    /// `None` when the caller supplied no [`Destination`], which is the
    /// default invocation and the only behaviour before M10.
    pub output: Option<Output>,
}

/// A partial file that [`ArtifactWriter::abort`] could not remove.
///
/// ADR-0015 section 9 requires a failed removal to be reported beside the
/// evidence failure that caused the abort.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Leftover {
    /// The partial file that remains in the destination.
    pub(crate) path: PathBuf,
    /// Why it could not be removed.
    pub(crate) removal: String,
}

/// Where the bytes are going while an artifact is streamed.
///
/// Private. It exists so the loop has one thing to write to whether or not
/// a file was opened, and so a destination failure part way through stops
/// writing without stopping the hashing.
enum Sink {
    /// No destination was asked for.
    Absent,
    /// Open and being written.
    Open(fs::File),
    /// The path was taken; nothing was opened.
    Taken,
    /// Creating the file failed, with the reason. Nothing is on disk.
    Unopened(String),
    /// The file was created and a write failed, with the reason. A partial
    /// file is on disk and must be removed.
    Broken(String),
}

/// Hashes an artifact's bytes and, where asked, writes them to a file.
///
/// The caller supplies the bytes in order through [`push`](Self::push) and
/// ends with exactly one of [`finish`](Self::finish) or
/// [`abort`](Self::abort). There is deliberately no `Drop`: dropping an
/// open writer would leave a partial file with nobody told, and a removal
/// that fails has to be reported, which a destructor cannot do.
#[must_use = "an unfinished writer leaves its file behind; call finish or abort"]
pub(crate) struct ArtifactWriter {
    hasher: Sha256Hasher,
    sink: Sink,
    path: Option<PathBuf>,
}

impl ArtifactWriter {
    /// Starts an artifact, creating `path` if one was asked for.
    ///
    /// A path that cannot be opened is not an error here: it is recorded,
    /// hashing proceeds, and [`finish`](Self::finish) reports it in
    /// [`Output`].
    pub(crate) fn begin(path: Option<PathBuf>) -> Self {
        let sink = open_sink(path.as_deref());

        Self {
            hasher: Sha256Hasher::new(),
            sink,
            path,
        }
    }

    /// Adds `bytes` to the digest and, if the file is open, to the file.
    ///
    /// The same slice, in the same pass. A write failure stops the writing
    /// and not the hashing: the digest is a fact about the evidence and the
    /// destination has no say in it.
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        self.hasher.update(bytes);

        if let Sink::Open(file) = &mut self.sink
            && let Err(e) = file.write_all(bytes)
        {
            self.sink = Sink::Broken(e.to_string());
        }
    }

    /// Ends the artifact: finishes the digest, flushes, and reads back.
    ///
    /// `slack_bytes` is carried into the result unchanged; it was never
    /// hashed or written.
    pub(crate) fn finish(self, slack_bytes: u32) -> Extraction {
        let hashed = self.hasher.finish();

        Extraction {
            digest: hashed.digest,
            bytes_hashed: hashed.bytes_read,
            slack_bytes,
            output: settle(self.sink, self.path, &mut vec![0u8; READBACK_BYTES]),
        }
    }

    /// Abandons the artifact after the evidence failed, removing any file
    /// this writer created.
    ///
    /// ADR-0015 Decision G. Returns the file left behind where it could not
    /// be removed, so the caller can report both failures: section 9.
    pub(crate) fn abort(self) -> Option<Leftover> {
        match (discard(self.sink, self.path.as_deref()), self.path) {
            (Some(removal), Some(path)) => Some(Leftover { path, removal }),
            _ => None,
        }
    }
}

/// Opens the destination, if one was asked for.
///
/// `create_new` makes the existence check and the creation one operation,
/// so nothing can appear between them. ADR-0015 Decision C.
fn open_sink(path: Option<&Path>) -> Sink {
    let Some(path) = path else {
        return Sink::Absent;
    };

    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => Sink::Open(file),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Sink::Taken,
        Err(e) => Sink::Unopened(e.to_string()),
    }
}

/// Removes a partial file after the evidence failed, and says why not
/// where it could not.
///
/// Both sinks that created a file are removed. A write that failed before
/// the read did leaves a truncated file just as surely as an open one, and
/// ADR-0015 Decision G removes the file on any error after it is created.
/// Section 9 also requires that a failed removal be reported beside the
/// evidence failure, so its reason is returned for the caller to carry.
fn discard(sink: Sink, path: Option<&Path>) -> Option<String> {
    let path = path?;

    match sink {
        Sink::Open(file) => {
            // Closed before removal, so the file is not held open on
            // platforms that care.
            drop(file);
            remove_partial(path)
        }
        Sink::Broken(_) => remove_partial(path),
        Sink::Absent | Sink::Taken | Sink::Unopened(_) => None,
    }
}

/// Removes a file this run created, returning why it remains if it does.
///
/// A file already gone is not a failure: nothing is left behind, which is
/// what the removal was for.
fn remove_partial(path: &Path) -> Option<String> {
    match fs::remove_file(path) {
        Ok(()) => None,
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => Some(e.to_string()),
    }
}

/// Closes the file and states what became of it.
fn settle(sink: Sink, path: Option<PathBuf>, buffer: &mut [u8]) -> Option<Output> {
    let path = path?;

    match sink {
        Sink::Absent => None,
        Sink::Taken => Some(Output::Exists { path }),
        Sink::Unopened(message) => Some(Output::NotCreated { path, message }),
        Sink::Broken(message) => {
            let removed = fs::remove_file(&path).is_ok();
            Some(Output::Failed {
                path,
                message,
                removed,
            })
        }
        Sink::Open(file) => {
            // Flushed before it is read back, because dropping a file
            // ignores the errors closing it can report, and `sync_all` is
            // where they surface. Closed before it is read back, so what is
            // hashed is what the filesystem holds rather than what a buffer
            // still owes it.
            let flushed = file.sync_all();
            drop(file);

            // A failed flush is a failed write. ADR-0015 Decision G.
            if let Err(e) = flushed {
                let removed = fs::remove_file(&path).is_ok();
                return Some(Output::Failed {
                    path,
                    message: e.to_string(),
                    removed,
                });
            }

            // ADR-0015 Decision H. Hashing during the write proves what was
            // handed to the kernel; this proves what landed.
            match read_back(&path, buffer) {
                Ok(readback) => Some(Output::Written { path, readback }),
                Err(e) => Some(Output::Unverified {
                    path,
                    message: e.to_string(),
                }),
            }
        }
    }
}

/// Hashes a written file through `buffer`.
fn read_back(path: &Path, buffer: &mut [u8]) -> io::Result<Sha256Digest> {
    use std::io::Read;

    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256Hasher::new();

    loop {
        let read = file.read(buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(hasher.finish().digest)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Bytes in one cluster of the synthetic volume the moved tests were
    /// written against. Any buffer length would do; this keeps them as they
    /// were.
    const CLUSTER_BYTES: usize = 512;

    /// An empty directory of this test's own under the system temporary
    /// directory, removed first so a previous run cannot decide this one.
    pub(crate) fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("taphonomy-unit-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("creating the scratch directory");
        dir
    }

    /// ADR-0015 Decision G. A write that failed earlier left a file behind,
    /// and a later evidence failure must remove it too.
    #[test]
    fn a_file_whose_write_had_failed_is_removed_when_the_evidence_then_fails() {
        let dir = scratch("broken-then-read");
        let path = dir.join("p1-c2-s1-first-4.bin");
        fs::write(&path, b"truncated").expect("planting a partial file");

        let left = discard(
            Sink::Broken("no space left".to_string()),
            Some(path.as_path()),
        );

        assert!(!path.exists(), "the partial file survived");
        assert!(
            left.is_none(),
            "a removal that succeeded was reported: {left:?}"
        );
    }

    /// A file of that name that this run did not create is not this run's
    /// to remove, whichever way the run failed.
    ///
    /// ADR-0015 Decision C and `SAFETY.md` section 15.
    #[test]
    fn a_file_this_run_did_not_create_is_never_removed() {
        let dir = scratch("not-ours");
        let path = dir.join("p1-c2-s1-first-4.bin");
        fs::write(&path, b"not this tool's").expect("planting a file");

        let taken = discard(Sink::Taken, Some(path.as_path()));
        let unopened = discard(Sink::Unopened("denied".to_string()), Some(path.as_path()));
        assert!(
            taken.is_none() && unopened.is_none(),
            "{taken:?} {unopened:?}"
        );
        let output = settle(
            Sink::Unopened("denied".to_string()),
            Some(path.clone()),
            &mut [0u8; CLUSTER_BYTES],
        );

        assert!(
            matches!(output, Some(Output::NotCreated { .. })),
            "expected NotCreated, got {output:?}"
        );
        assert_eq!(
            fs::read(&path).expect("reading the planted file"),
            b"not this tool's"
        );
    }

    /// A file that could not be created is reported as such, and not as a
    /// failure that may have left something behind.
    ///
    /// A missing directory is used rather than permission bits, so the
    /// result does not depend on whether the tests run as root.
    #[test]
    fn a_file_that_could_not_be_created_is_reported_not_created() {
        let dir = scratch("uncreatable");
        let path = dir.join("missing").join("p1-c2-s1-first-4.bin");

        let sink = open_sink(Some(&path));
        assert!(
            matches!(sink, Sink::Unopened(_)),
            "expected the open to fail"
        );

        let output = settle(sink, Some(path.clone()), &mut [0u8; CLUSTER_BYTES]);

        assert!(
            matches!(output, Some(Output::NotCreated { .. })),
            "expected NotCreated, got {output:?}"
        );
        assert!(!path.exists());
    }

    /// ADR-0015 Decision G. A write failure removes what was written and
    /// says whether the removal succeeded.
    #[test]
    fn a_write_failure_removes_the_partial_file_and_says_so() {
        let dir = scratch("write-failure");
        let path = dir.join("p1-c2-s1-first-4.bin");
        fs::write(&path, b"truncated").expect("planting a partial file");

        let output = settle(
            Sink::Broken("no space left".to_string()),
            Some(path.clone()),
            &mut [0u8; CLUSTER_BYTES],
        );

        assert!(
            matches!(output, Some(Output::Failed { removed: true, .. })),
            "expected Failed with the file removed, got {output:?}"
        );
        assert!(!path.exists(), "the partial file survived");
    }

    /// ADR-0015 section 9. A partial file the run could not remove is
    /// reported, not assumed gone.
    ///
    /// Unix-specific because it relies on permission bits, as
    /// `tests/read_only.rs` does. Removing a file needs the same write
    /// permission on its directory as creating one, so the control proves
    /// the directory refuses both; running as root would defeat it, and the
    /// control makes that a failure rather than a vacuous pass.
    #[cfg(unix)]
    #[test]
    fn a_partial_file_that_cannot_be_removed_is_reported() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch("unremovable");
        let path = dir.join("p1-c2-s1-first-4.bin");
        fs::write(&path, b"truncated").expect("planting a partial file");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555))
            .expect("making the destination read-only");

        let accepted = fs::File::create(dir.join("probe")).is_ok();
        let left = discard(
            Sink::Broken("no space left".to_string()),
            Some(path.as_path()),
        );

        // Restored before any assertion, so a failure cannot leave behind a
        // directory the next run's scratch() is unable to remove.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755))
            .expect("restoring the destination");

        assert!(
            !accepted,
            "control failed: the directory accepted a new file, so this test proves nothing"
        );
        assert!(left.is_some(), "a failed removal was not reported");
        assert!(path.exists(), "the file was removed after all");
    }

    // Tests of ArtifactWriter itself. The five above were moved with the
    // functions they exercise.

    /// The digest covers every byte pushed, including those pushed after
    /// the file stopped being written. Written as the expected value, not
    /// as the writer computes it.
    #[test]
    fn the_digest_covers_every_byte_pushed_after_a_write_failure() {
        let dir = scratch("writer-digest");
        let path = dir.join("artifact.bin");

        let mut writer = ArtifactWriter::begin(Some(path.clone()));
        writer.push(b"abc");
        // Stands in for a failed write; a real one needs a full disk.
        writer.sink = Sink::Broken("no space left".to_string());
        writer.push(b"def");
        let extraction = writer.finish(0);

        let expected = crate::hash::hash_reader(&mut &b"abcdef"[..]).expect("hashing a slice");
        assert_eq!(extraction.digest, expected.digest);
        assert_eq!(extraction.bytes_hashed, 6);
        assert!(
            matches!(
                extraction.output,
                Some(Output::Failed { removed: true, .. })
            ),
            "got {:?}",
            extraction.output
        );
        assert!(!path.exists(), "the partial file survived");
    }

    #[test]
    fn no_destination_gives_no_output() {
        let mut writer = ArtifactWriter::begin(None);
        writer.push(b"abc");
        let extraction = writer.finish(7);

        assert_eq!(extraction.output, None);
        assert_eq!(extraction.bytes_hashed, 3);
        assert_eq!(extraction.slack_bytes, 7);
    }

    #[test]
    fn an_existing_path_is_reported_and_left_untouched() {
        let dir = scratch("writer-exists");
        let path = dir.join("artifact.bin");
        fs::write(&path, b"already here").expect("planting a file");

        let mut writer = ArtifactWriter::begin(Some(path.clone()));
        writer.push(b"new bytes");
        let extraction = writer.finish(0);

        assert_eq!(
            extraction.output,
            Some(Output::Exists { path: path.clone() })
        );
        assert_eq!(fs::read(&path).expect("reading it"), b"already here");
    }

    #[test]
    fn abort_removes_the_file_of_an_open_writer() {
        let dir = scratch("writer-abort");
        let path = dir.join("artifact.bin");

        let mut writer = ArtifactWriter::begin(Some(path.clone()));
        writer.push(b"partial");
        assert!(path.exists(), "control failed: nothing was created");

        assert_eq!(writer.abort(), None);
        assert!(!path.exists(), "the partial file survived");
    }
}
