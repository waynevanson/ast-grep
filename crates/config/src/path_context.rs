//! The file a rule is being applied to.
//!
//! A rule matches a syntax tree, which does not know the path it came from.
//! Transforms that describe a directory rather than a node need it, so the scan
//! scopes the path around each file it hands to the rules.
//!
//! It is per thread, not global: files are scanned in parallel, one file per
//! thread at a time, and a thread that is not scanning a file has none.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

thread_local! {
  static CURRENT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Run `scan` with `path` as the file being scanned, then restore what was
/// current before it.
pub fn with_file<T>(path: &Path, scan: impl FnOnce() -> T) -> T {
  let previous = CURRENT.with(|cell| cell.replace(Some(path.to_path_buf())));
  let scanned = scan();
  CURRENT.with(|cell| *cell.borrow_mut() = previous);
  scanned
}

/// The file being scanned on this thread, if any.
pub fn current_file() -> Option<PathBuf> {
  CURRENT.with(|cell| cell.borrow().clone())
}
