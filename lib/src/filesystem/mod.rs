//! Structures that represent bookkeeping that the program is doing but which
//! does not map immediately to disk structures
use std::collections::BTreeMap;
use std::fmt::Debug;

use super::directory::{DirEntry, Directory};

pub(crate) type FileTree = BTreeMap<Path, FileObject>;

mod path;
pub use path::Path;

mod fixed_len_string;
pub use fixed_len_string::FixedLenString;

/// An entry for the [`FileTree`], representing either a directory or a file.
///
/// This does not correspond neatly to disk structures, where a [`DirEntry`]
/// representing a file only exists as a field inside a [`Directory`]
#[derive(Debug, Clone)]
pub enum FileObject {
    SparseDir(DirEntry),
    LoadedDir(Box<Directory>),
    File(DirEntry),
}
