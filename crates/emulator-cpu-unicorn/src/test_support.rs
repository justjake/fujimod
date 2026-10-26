//! Explicit native execution input for source-authored instruction tests.
use crate::Library;

pub(crate) fn library() -> Library {
    Library::linked().expect("use the crate's pinned Unicorn build")
}
