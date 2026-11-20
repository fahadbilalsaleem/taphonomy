//! A sparse image built from the fuzz input, shared by every target.
//!
//! Input layout, all little-endian:
//!
//! * `u32` image length in sectors, capped at `MAX_SECTORS`;
//! * `u8` flags, bit 0 set runs with `recover` where a target uses it;
//! * then runs, each `u32` first sector, `u16` sector count, and the
//!   sectors' bytes (a run cut short by the input's end is zero-filled).
//!
//! Every sector no run covers reads as zero, and a read past the image's
//! length fails, as `EvidenceReader` requires.

use taphonomy::Error;
use taphonomy::evidence::EvidenceReader;

pub const SECTOR: u64 = 512;

/// 128 MiB, a little above the largest fixture (261,120 sectors). Without
/// a cap a few bytes could declare a volume no file of their size is.
pub const MAX_SECTORS: u32 = 1 << 18;

pub struct SparseImage<'a> {
    pub len: u64,
    pub flags: u8,
    runs: Vec<(u64, &'a [u8])>,
}

impl<'a> SparseImage<'a> {
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        let sectors = u32::from_le_bytes(data.get(0..4)?.try_into().ok()?);
        let flags = *data.get(4)?;
        let len = u64::from(sectors.min(MAX_SECTORS)) * SECTOR;

        let mut runs = Vec::new();
        let mut rest = &data[5..];
        while rest.len() >= 6 {
            let first = u32::from_le_bytes(rest[0..4].try_into().ok()?);
            let count = u16::from_le_bytes(rest[4..6].try_into().ok()?);
            rest = &rest[6..];
            let want = usize::from(count) * SECTOR as usize;
            let take = want.min(rest.len());
            runs.push((u64::from(first) * SECTOR, &rest[..take]));
            rest = &rest[take..];
        }
        Some(Self { len, flags, runs })
    }
}

/// Recomputes the set checksum of every exFAT File entry (85h, or 05h
/// once deleted) found at a 32-byte boundary of a run, over the entries it
/// declares, where the whole set lies in that run (section 6.3.3).
///
/// A mutation that changes a set's bytes almost never also writes its
/// checksum, so without this the set parser past the checksum is rarely
/// reached by mutation.
pub fn fix_checksums(data: &mut [u8]) {
    let mut at = 5;
    while at + 6 <= data.len() {
        let count = usize::from(u16::from_le_bytes([data[at + 4], data[at + 5]]));
        let start = at + 6;
        let end = (start + count * SECTOR as usize).min(data.len());
        let mut entry = start;
        while entry + 32 <= end {
            // A File entry (85h, or 05h deleted) followed by a Stream
            // Extension (C0h, or 40h): a byte that only looks like one,
            // in a boot sector or file data, is left alone.
            if data[entry] & 0x7F == 0x05 {
                let len = (1 + usize::from(data[entry + 1])) * 32;
                // Whatever count it declares, a count too small included.
                if entry + len.max(64) <= end && data[entry + 32] & 0x7F == 0x40 {
                    // A deleted set keeps the checksum it had in use: sum
                    // each entry's type byte with InUse (80h) set again.
                    let mut sum = 0u16;
                    for (i, &b) in data[entry..entry + len].iter().enumerate() {
                        let b = if i % 32 == 0 { b | 0x80 } else { b };
                        if i != 2 && i != 3 {
                            sum = sum.rotate_right(1).wrapping_add(u16::from(b));
                        }
                    }
                    data[entry + 2..entry + 4].copy_from_slice(&sum.to_le_bytes());
                }
            }
            entry += 32;
        }
        at = end;
    }
}

/// Rewrites the checksum sector of every exFAT boot region found at a
/// sector boundary of a run, where all twelve sectors lie in that run
/// (section 3.4: sectors 0 to 10, skipping VolumeFlags and PercentInUse).
/// Written here from the specification, not with the crate's own
/// `boot_checksum`, so a fault there is not copied into the input.
pub fn reseal_boot_regions(data: &mut [u8]) {
    let mut at = 5;
    while at + 6 <= data.len() {
        let count = usize::from(u16::from_le_bytes([data[at + 4], data[at + 5]]));
        let start = at + 6;
        let end = (start + count * SECTOR as usize).min(data.len());
        let mut sector = start;
        while sector + SECTOR as usize <= end {
            let shift = data[sector + 108];
            if &data[sector + 3..sector + 11] == b"EXFAT   " && (9..=12).contains(&shift) {
                let bytes = 1usize << shift;
                if sector + 12 * bytes <= end {
                    let mut sum = 0u32;
                    for (i, &b) in data[sector..sector + 11 * bytes].iter().enumerate() {
                        if i != 106 && i != 107 && i != 112 {
                            sum = sum.rotate_right(1).wrapping_add(u32::from(b));
                        }
                    }
                    for word in data[sector + 11 * bytes..sector + 12 * bytes].chunks_exact_mut(4) {
                        word.copy_from_slice(&sum.to_le_bytes());
                    }
                }
            }
            sector += SECTOR as usize;
        }
        at = end;
    }
}

/// The mutator's fix-ups, unless `FUZZ_NO_FIX` is set in the environment,
/// so one build can be measured with and without them.
pub fn fix_up(data: &mut [u8], seed: u32) {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *OFF.get_or_init(|| std::env::var_os("FUZZ_NO_FIX").is_some()) || seed % 2 != 0 {
        return;
    }
    fix_checksums(data);
    reseal_boot_regions(data);
}

impl EvidenceReader for SparseImage<'_> {
    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let end = match offset.checked_add(buf.len() as u64) {
            Some(end) if end <= self.len => end,
            _ => {
                return Err(Error::Io {
                    path: "<fuzz>".into(),
                    source: std::io::ErrorKind::UnexpectedEof.into(),
                });
            }
        };

        buf.fill(0);
        for &(start, bytes) in &self.runs {
            let lo = start.max(offset);
            let hi = (start + bytes.len() as u64).min(end);
            if lo < hi {
                let src = (lo - start) as usize;
                let dst = (lo - offset) as usize;
                let n = (hi - lo) as usize;
                buf[dst..dst + n].copy_from_slice(&bytes[src..src + n]);
            }
        }
        Ok(())
    }
}
