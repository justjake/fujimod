//! Self-contained, sparse page images. The decoder checks the constructor's
//! extent before allocating; no live backing or previous keyframe is needed.
use super::{Image, PAGE_BYTES, zero};
use std::sync::Arc;

const MAGIC: &[u8; 8] = b"FJMPAGE1";
const HEADER: usize = 24;

impl Image {
    /// Encode immutable pages, omitting zero pages. The final page contains
    /// only the bytes inside the backing's extent.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let nonzero = self
            .pages
            .iter()
            .enumerate()
            .filter(|(_, page)| !Arc::ptr_eq(page, zero()) && page.iter().any(|&byte| byte != 0))
            .collect::<Vec<_>>();
        let size = nonzero.iter().try_fold(HEADER, |size, (index, _)| {
            size.checked_add(8 + PAGE_BYTES.min(self.len - index * PAGE_BYTES))
                .ok_or("snapshot page image size overflow")
        })?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|_| "cannot allocate snapshot page image")?;
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(self.len as u64).to_le_bytes());
        bytes.extend_from_slice(&(nonzero.len() as u64).to_le_bytes());
        for (index, page) in nonzero {
            bytes.extend_from_slice(&(index as u64).to_le_bytes());
            bytes.extend_from_slice(&page[..PAGE_BYTES.min(self.len - index * PAGE_BYTES)]);
        }
        Ok(bytes)
    }

    /// Decode against an extent supplied by the fresh machine constructor.
    /// Validate the entire payload before allocating its page directory.
    pub fn from_bytes(bytes: &[u8], expected_len: usize) -> Result<Self, String> {
        if bytes.get(..8) != Some(MAGIC) {
            return Err("unknown snapshot page image format".into());
        }
        let word = |at: usize| -> Result<u64, String> {
            Ok(u64::from_le_bytes(
                bytes
                    .get(at..at + 8)
                    .ok_or("truncated snapshot page image")?
                    .try_into()
                    .unwrap(),
            ))
        };
        if word(8)? != expected_len as u64 {
            return Err("snapshot page image extent differs".into());
        }
        let count = usize::try_from(word(16)?).map_err(|_| "snapshot page count exceeds target")?;
        let page_count = expected_len.div_ceil(PAGE_BYTES);
        if count > page_count {
            return Err("snapshot page count exceeds extent".into());
        }
        let mut cursor = HEADER;
        let mut previous = None;
        for _ in 0..count {
            let index =
                usize::try_from(word(cursor)?).map_err(|_| "snapshot page index exceeds target")?;
            if index >= page_count || previous.is_some_and(|old| index <= old) {
                return Err(
                    "snapshot page indices must be unique, ordered and inside the extent".into(),
                );
            }
            cursor += 8;
            let size = PAGE_BYTES.min(expected_len - index * PAGE_BYTES);
            let page = bytes
                .get(
                    cursor
                        ..cursor
                            .checked_add(size)
                            .ok_or("snapshot page image size overflow")?,
                )
                .ok_or("truncated snapshot page image")?;
            if page.iter().all(|&byte| byte == 0) {
                return Err("snapshot page image stores a zero page".into());
            }
            cursor += size;
            previous = Some(index);
        }
        if cursor != bytes.len() {
            return Err("snapshot page image has trailing bytes".into());
        }
        let mut pages = Vec::new();
        pages
            .try_reserve_exact(page_count)
            .map_err(|_| "cannot allocate snapshot page directory")?;
        pages.resize(page_count, zero().clone());
        cursor = HEADER;
        for _ in 0..count {
            let index = word(cursor)? as usize;
            cursor += 8;
            let size = PAGE_BYTES.min(expected_len - index * PAGE_BYTES);
            let mut page = [0; PAGE_BYTES];
            page[..size].copy_from_slice(&bytes[cursor..cursor + size]);
            pages[index] = Arc::new(page);
            cursor += size;
        }
        Ok(Self {
            len: expected_len,
            pages: pages.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::pages::Tracker;

    #[test]
    fn sparse_image_restores_into_fresh_backing_without_its_capture_owner() {
        let mut original = vec![0; PAGE_BYTES * 4 + 7];
        original[PAGE_BYTES + 42] = 99;
        original[PAGE_BYTES * 4 + 6] = 11;
        let encoded = {
            let tracker = Tracker::new(original.len());
            let image = unsafe { tracker.capture(original.as_ptr()) };
            image.to_bytes().unwrap()
        };
        assert_eq!(encoded.len(), HEADER + 16 + PAGE_BYTES + 7);
        let decoded = Image::from_bytes(&encoded, original.len()).unwrap();
        let mut restored = vec![0xff; original.len()];
        let tracker = Tracker::new(restored.len());
        unsafe { tracker.restore(&decoded, restored.as_mut_ptr()) }.unwrap();
        assert_eq!(restored, original);
        assert_eq!(decoded.to_bytes().unwrap(), encoded);
        assert!(Arc::ptr_eq(&decoded.pages[0], &decoded.pages[2]));
        let empty = unsafe { Tracker::new(0).capture(std::ptr::null()) };
        assert!(
            Image::from_bytes(&empty.to_bytes().unwrap(), 0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn corrupt_page_images_are_rejected_before_restore() {
        let mut original = vec![0; PAGE_BYTES * 2 + 1];
        original[0] = 1;
        original[PAGE_BYTES * 2] = 2;
        let image = unsafe { Tracker::new(original.len()).capture(original.as_ptr()) };
        let encoded = image.to_bytes().unwrap();
        for end in [0, 7, 8, 16, HEADER, HEADER + 8, encoded.len() - 1] {
            assert!(Image::from_bytes(&encoded[..end], original.len()).is_err());
        }
        assert!(Image::from_bytes(&encoded, original.len() + 1).is_err());
        for (at, value) in [(16, 4u64), (HEADER, 3), (HEADER + 8 + PAGE_BYTES, 0)] {
            let mut bad = encoded.clone();
            bad[at..at + 8].copy_from_slice(&value.to_le_bytes());
            assert!(Image::from_bytes(&bad, original.len()).is_err());
        }
        let mut bad = encoded.clone();
        bad[HEADER + 8] = 0;
        assert!(Image::from_bytes(&bad, original.len()).is_err());
        let mut bad = encoded;
        bad.push(0);
        assert!(Image::from_bytes(&bad, original.len()).is_err());
    }
}
