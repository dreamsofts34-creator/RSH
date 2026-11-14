//! The testable seam for shared-memory access (`TELEMETRY-READER.md` §2
//! rule 5): readers never touch a Windows API directly. They read through
//! [`MapAccessor`], which on Windows is a read-only view of the game's
//! named file mapping and in tests is a byte slice ([`SliceMap`]) or a
//! hostile accessor that tears reads.
//!
//! Fail-safe (SEC-H-05a): every multi-byte read goes through
//! [`read_consistent`], which reads the same range twice and only accepts
//! the bytes when two consecutive reads agree — the game may be writing
//! the block while we read it. A torn read is retried once, then rejected
//! (`None`); a torn read is never surfaced as a snapshot.

/// Maximum bytes one [`read_consistent`] call will allocate.
///
/// Every real read is far below this (AC/ACC identity: 200 B; iRacing
/// session YAML: at most 256 KiB by spec). The cap bounds allocation when
/// a length comes from an untrusted map header.
pub const MAX_SINGLE_READ_BYTES: usize = 1024 * 1024;

/// A read-only view of a shared-memory block.
///
/// Implementations must be total: out-of-bounds reads return `false`,
/// they never panic. The Windows implementation copies from a mapped view
/// (see `windows.rs`); the test implementation wraps a byte slice.
pub trait MapAccessor {
    /// Length of the mapped block in bytes.
    fn len(&self) -> usize;

    /// Whether the mapped block is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Copies `buf.len()` bytes starting at `offset` into `buf`.
    ///
    /// Returns `false` (and leaves `buf` untouched in spirit — callers
    /// must not use `buf` when this returns `false`) when the range is
    /// out of bounds.
    fn read_into(&self, offset: usize, buf: &mut [u8]) -> bool;
}

/// A [`MapAccessor`] over an in-memory byte slice.
///
/// This is what fixture tests and the recorded shared-memory fixtures use:
/// a reader parses exactly the same bytes a live game would expose.
pub struct SliceMap<'a> {
    data: &'a [u8],
}

impl<'a> SliceMap<'a> {
    /// Wraps `data` as a read-only map.
    #[must_use]
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data }
    }
}

impl MapAccessor for SliceMap<'_> {
    fn len(&self) -> usize {
        self.data.len()
    }

    fn read_into(&self, offset: usize, buf: &mut [u8]) -> bool {
        let Some(end) = offset.checked_add(buf.len()) else {
            return false;
        };
        let Some(slice) = self.data.get(offset..end) else {
            return false;
        };
        buf.copy_from_slice(slice);
        true
    }
}

/// Reads `len` bytes at `offset` twice and returns them only when two
/// consecutive reads agree (SEC-H-05a double-read).
///
/// On a mismatch the read is retried once; a second mismatch returns
/// `None`. Out-of-bounds ranges and over-long reads also return `None`.
/// Never panics.
#[must_use]
pub fn read_consistent(map: &dyn MapAccessor, offset: usize, len: usize) -> Option<Vec<u8>> {
    if len == 0 || len > MAX_SINGLE_READ_BYTES {
        return None;
    }
    if offset.checked_add(len)? > map.len() {
        return None;
    }
    for _ in 0..2 {
        let mut first = vec![0_u8; len];
        let mut second = vec![0_u8; len];
        if !map.read_into(offset, &mut first) || !map.read_into(offset, &mut second) {
            return None;
        }
        if first == second {
            return Some(first);
        }
    }
    None
}

/// Reads a little-endian `i32` at `offset`, double-read for consistency.
pub(crate) fn read_i32(map: &dyn MapAccessor, offset: usize) -> Option<i32> {
    let bytes = read_consistent(map, offset, 4)?;
    let arr: [u8; 4] = bytes.try_into().ok()?;
    Some(i32::from_le_bytes(arr))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An accessor that returns different bytes on every read: simulates a
    /// game writing the block mid-read (torn read).
    struct TearingMap {
        len: usize,
        flip: std::cell::Cell<u8>,
    }

    impl MapAccessor for TearingMap {
        fn len(&self) -> usize {
            self.len
        }

        fn read_into(&self, _offset: usize, buf: &mut [u8]) -> bool {
            let value = self.flip.get();
            self.flip.set(value.wrapping_add(1));
            buf.fill(value);
            true
        }
    }

    #[test]
    fn consistent_read_accepts_stable_slice() {
        let map = SliceMap::new(b"hello world");
        assert_eq!(read_consistent(&map, 0, 5), Some(b"hello".to_vec()));
        assert_eq!(read_consistent(&map, 6, 5), Some(b"world".to_vec()));
    }

    #[test]
    fn consistent_read_rejects_out_of_bounds() {
        let map = SliceMap::new(b"short");
        assert_eq!(read_consistent(&map, 3, 10), None);
        assert_eq!(read_consistent(&map, usize::MAX, 4), None);
        assert_eq!(read_consistent(&map, 0, 0), None);
    }

    #[test]
    fn consistent_read_rejects_torn_read() {
        let map = TearingMap {
            len: 64,
            flip: std::cell::Cell::new(0),
        };
        // Every read returns different bytes, so both attempts disagree.
        assert_eq!(read_consistent(&map, 0, 8), None);
    }

    #[test]
    fn slice_map_read_into_bounds() {
        let map = SliceMap::new(&[1, 2, 3, 4]);
        let mut buf = [0_u8; 2];
        assert!(map.read_into(2, &mut buf));
        assert_eq!(buf, [3, 4]);
        assert!(!map.read_into(3, &mut buf));
        assert!(!map.is_empty());
        assert!(SliceMap::new(&[]).is_empty());
    }

    #[test]
    fn read_i32_decodes_little_endian() {
        let map = SliceMap::new(&[0x02, 0x00, 0x00, 0x00]);
        assert_eq!(read_i32(&map, 0), Some(2));
        assert_eq!(read_i32(&map, 1), None); // only 3 bytes left
    }
}
