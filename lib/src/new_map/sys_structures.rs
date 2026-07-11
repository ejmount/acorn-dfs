/// Structures that represent bookkeeping that the program is doing but which
/// does not map immediately to disk structures
use std::borrow::Borrow;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::ops::Index;

use winnow::Parser;
use winnow::combinator::{opt, preceded, separated, terminated};
use winnow::stream::Stream;

use super::disc_structures::NewMap;
use super::filesystem::{Attributes, DirEntry, Directory};
use super::util::{DiscPosition, FaultableResult, FixedLenString, ParseResult, make_input};
use super::{Fault, FaultValue, IoError};

type FileTree = BTreeMap<Path, FileObject>;

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

/// Represents the parsed contents of a ADFS format-E disk.
///
/// The data between the fields of this structure are slightly redundant - the
/// `image` field contains the disk bytes, but other structures contain the same
/// data by value. This is to simplify lifetimes, and the disk is not expected
/// to ever be big enough for the redundancy to be a significant performance
/// problem.
#[derive(Clone)]
pub struct FormatE {
    /// The raw disk bytes.
    pub image: Vec<u8>,
    /// The parsed "Map" structure, effectively the superblock
    pub map: NewMap,
    /// A summarised copy of the filesystem tree - this does not directly
    /// correspond to any disk contents.
    pub(crate) tree: RefCell<FileTree>,
    /// A list of non-fatal faults encountered while parsing the disk data. This
    /// includes validation failures, etc.
    pub faults: Vec<Fault>,
}
impl Debug for FormatE {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FormatE")
            .field("map", &self.map)
            .field("tree", &self.tree)
            .field("image", &&self.image[..10.min(self.image.len())])
            .field("faults", &self.faults)
            .finish()
    }
}

impl FormatE {
    // Entry point for creating Format-E disks. The resulting structure does not
    // populate the file tree.
    pub fn parse<'a>(bytes: &'a [u8]) -> ParseResult<'a, Self> {
        let image = bytes.to_vec();
        let mut input = make_input(bytes);
        let FaultValue(map, mut faults) = NewMap::parse(&mut input, 1)?;

        // Retrieving the root directory object is special, so we do that upfront so
        // that other FS operations can assume the root is cached and walk down from
        // there.
        let FaultValue(tree, root_faults) = Self::create_initial_file_tree(bytes, &map)?;

        faults.extend(root_faults);

        Ok(FormatE {
            image,
            map,
            tree: RefCell::new(tree),
            faults,
        })
    }

    pub fn get_map_json(&self) -> String {
        serde_json::to_string_pretty(&self.map).unwrap()
    }

    fn create_initial_file_tree<'a>(
        image: &'a [u8],
        map: &NewMap,
    ) -> FaultableResult<'a, FileTree> {
        let mut tree = FileTree::new();
        let mut faults = vec![];

        let root_link = map.get_disc_record().root_dir;
        let sector_size = map.get_disc_record().sector_size_in_bytes();
        let FaultValue(root_dir, root_faults) = Self::retrieve_directory(
            map,
            image,
            root_link,
            sector_size,
            Fault::InvalidRoot {
                root_link,
                sector_size,
            },
        )?;
        faults.extend(root_faults);

        for child in root_dir.entries.iter().cloned() {
            let name = Path::from_segments(&[child.obj_name]);
            Self::insert_dir_entry(&mut tree, child, name);
        }
        tree.insert(Path::ROOT_PATH, FileObject::LoadedDir(Box::new(root_dir)));

        Ok(FaultValue(tree, faults))
    }

    /// Ensures that `self.tree` contains metadata for the given [`Path`],
    /// all of its ancestors, and at least sparse entries for all of its direct
    /// children.
    ///
    /// Can fail with
    /// - [`IoError::MissingTarget`]: Path does not exist on disk
    /// - [`IoError::InvalidTarget`]: there is a file midway through the given
    ///   Path.
    /// - [`ParseError`]: A directory structure on disk was invalid
    fn populate_path(&self, path: &Path) -> FaultableResult<'_, Result<(), IoError>> {
        let mut overall_faults = vec![];

        // Walk up down the parent folders for the given path, root first.
        // The root itself is already populated on construction so start from first
        // subfolder
        for components in 1..=path.len() {
            let stem = Path::from_segments(&path[..components]);

            // Try to look up the [`DirEntry`] pointer that will tell us where the
            // directory structure itself is located.
            let child_entry = match self.tree.borrow().get(&stem) {
                // Don't need to do anything if the folder is alredy cached and expanded.
                // We can immediately start looking at the next layer down.
                Some(FileObject::LoadedDir(_)) => continue,
                // If the pointer is cached, return it.
                Some(FileObject::SparseDir(entry)) => entry.clone(),

                Some(FileObject::File(_)) => {
                    // If the entry is a file, check whether we're at the bottom-most layer we're
                    // expecting and we found the target successfully, else abort with an error.
                    if stem == *path {
                        return Ok(FaultValue(Ok(()), vec![]));
                    } else {
                        return Ok(FaultValue(Err(IoError::InvalidTarget(stem)), vec![]));
                    }
                }
                // (Unclear if it's possible to take this branch in practice because we build the
                // cache incrementally from the root)
                None => {
                    // If it doesn't exist, take the current stem and split it into its ancestry and
                    // the name of the new item we need to add. If needed, we will
                    // have filled in the parent on the last iteration.
                    let (child_name, parent_path) = stem[..].split_last().unwrap();

                    // Grab the cached parent directory listing.
                    // (Can't hold open the borrow on self.tree)
                    let FileObject::LoadedDir(parent) = self.tree.borrow()[parent_path].clone()
                    else {
                        return Ok(FaultValue(Err(IoError::InvalidTarget(stem)), vec![]));
                    };

                    // Fill in any cache entries that refer to files since we have them at this
                    // point anyway
                    for child in parent.entries.iter().cloned() {
                        let child_path = Path::from_segments(parent_path).append(child.obj_name);
                        Self::insert_dir_entry(&mut self.tree.borrow_mut(), child, child_path);
                    }

                    // Having grabbed the parent directory listing, look for the child entry we
                    // actually need. If it's missing, abort.
                    let Some(child_entry) =
                        parent.entries.iter().find(|c| c.obj_name == *child_name)
                    else {
                        return Ok(FaultValue(Err(IoError::MissingTarget(stem)), vec![]));
                    };
                    child_entry.clone()
                }
            };

            // If the next path segment refers to a file, we're done either way, but this is
            // an error if we still have path left to look up
            if !child_entry.attrs.contains(Attributes::DIR) {
                if stem == *path {
                    return Ok(FaultValue(Ok(()), vec![]));
                } else {
                    return Ok(FaultValue(Err(IoError::InvalidTarget(stem)), vec![]));
                }
            }

            // Using the metadata from the child entry, look at the disk bytes for the
            // actual directory listing
            let FaultValue(child_directory, faults) = Self::retrieve_directory(
                &self.map,
                &self.image,
                child_entry.address,
                self.map.get_disc_record().sector_size_in_bytes(),
                Fault::InvalidDir { path: stem.clone() },
            )?;
            // If the new entry has fault codes, store those
            overall_faults.extend(faults);

            for child in &child_directory.entries {
                let child_path = stem.clone().append(child.obj_name);
                Self::insert_dir_entry(&mut self.tree.borrow_mut(), child.clone(), child_path);
            }

            // Upsert the cache entry.
            self.tree
                .borrow_mut()
                .insert(stem, FileObject::LoadedDir(Box::new(child_directory)));
        }
        Ok(FaultValue(Ok(()), overall_faults))
    }

    fn insert_dir_entry(tree: &mut FileTree, child: DirEntry, child_path: Path) {
        if child.attrs.contains(Attributes::DIR) {
            tree.insert(child_path, FileObject::SparseDir(child));
        } else {
            tree.insert(child_path, FileObject::File(child));
        }
    }

    /// Retrieve the section of the disk that corresponds to the given address
    /// and parse it as [`Directory`] object.
    fn retrieve_directory<'a>(
        map: &NewMap,
        image: &'a [u8],
        addr: DiscPosition,
        sector_size: usize,
        context: impl Into<Option<Fault>>,
    ) -> FaultableResult<'a, Directory> {
        let block = map.get_allocation(0).get_fragment(addr.fragment()).unwrap();
        let entry_region = block.disk_region();

        // Slightly convoluted but it means that parser errors have the right offset,
        // w.r.t the entire image
        let mut cursor = make_input(image);
        let offset = addr.sector_idx() as usize * sector_size;
        cursor.next_slice(entry_region.start + offset);

        if let Some(c) = context.into() {
            Directory::parse.context(c).parse_next(&mut cursor)
        } else {
            Directory::parse.parse_next(&mut cursor)
        }
    }
    /// Gets the metadata and contents of a given path
    pub fn get_file(
        &self,
        path: &Path,
    ) -> FaultableResult<'_, Result<(DirEntry, Vec<u8>), IoError>> {
        self.populate_path(path)?;

        // This should not be able to fail because we bailed if `populate_path` failed
        let fileobject = &self.tree.borrow()[path];

        let FileObject::File(dir_entry) = fileobject else {
            return Ok(FaultValue(
                Err(IoError::InvalidTarget(path.clone())),
                vec![],
            ));
        };

        let region = match self.map.get_file_region(dir_entry) {
            Some(region) => region,
            None => {
                return Ok(FaultValue(
                    Err(IoError::MissingFragment(dir_entry.address)),
                    vec![],
                ));
            }
        };

        let mut contents = Vec::with_capacity(region.end - region.start);
        contents.extend_from_slice(&self.image[region]);
        Ok(FaultValue(Ok((dir_entry.clone(), contents)), vec![]))
    }

    pub fn entries(&self, prefix: Option<Path>) -> impl Iterator<Item = Path> {
        let prefix = prefix.unwrap_or(Path::ROOT_PATH);

        loop {
            let sparse_entries: Vec<_> = self
                .tree
                .borrow()
                .iter()
                .skip_while(|&(k, _)| *k < prefix)
                .take_while(|&(k, _)| prefix.is_prefix(k))
                .filter(|(_, v)| matches!(v, FileObject::SparseDir(_)))
                .map(|(k, _)| k.clone())
                .collect();

            if sparse_entries.is_empty() {
                break;
            }
            for entry in sparse_entries {
                let res = self.populate_path(&entry);
                assert!(
                    matches!(res, Ok(FaultValue(Ok(_), _))),
                    "Unexpected fault populating file cache: {res:?}"
                );
            }
        }

        let entries: Vec<_> = self
            .tree
            .borrow()
            .keys()
            .skip_while(|&p| *p < prefix)
            .take_while(|p| prefix.is_prefix(p))
            .cloned()
            .collect();

        entries.into_iter()
    }
}

/// Represents a Path on the ADFS filesystem.
///
/// ADFS paths are not necessarily valid UTF-8, so cannot be represented by
/// aggregates of [`String`].
///
/// The default empty value corresponds to the root directory `$`.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Path(Vec<FixedLenString>);

impl Path {
    pub const ROOT_PATH: Path = Path(vec![]);
    pub const ROOT_SYMBOL: u8 = b'$';
    pub const DIR_SEPARATOR: u8 = b'.';
    /// Create a path from a byte-string representing the entire path.
    ///
    /// Paths do not exist in this form anywhere within ADFS disk structures, so
    /// this should not be used to read disk content. Instead, it is used for,
    /// e.g. human-provided input referring to file locations within the ADFS
    /// disk.
    ///
    /// Will return `None` if the provided path is invalid. This can be because
    /// the path is ill-formed, or because a single segment is too long to
    /// actually be encoded on the disk.
    fn from_bytes(input: &[u8]) -> Option<Path> {
        let input = make_input(input);

        let segments_parser = preceded(
            Self::DIR_SEPARATOR,
            separated(
                1..,
                FixedLenString::parse_from_byte_str,
                Self::DIR_SEPARATOR,
            ),
        );
        let segments = preceded(
            Self::ROOT_SYMBOL,
            terminated(opt(segments_parser), opt(Self::DIR_SEPARATOR)),
        )
        .parse(input)
        .ok()?;

        Some(Path(segments.unwrap_or_default()))
    }
    fn is_prefix(&self, other: &Self) -> bool {
        let mut zipper: Vec<_> = self.0.iter().zip(other.0.iter()).collect();
        let last = zipper.pop();
        for (a, b) in zipper {
            if a != b {
                return false;
            }
        }
        if let Some((a, b)) = last {
            a.is_prefix(b)
        } else {
            true
        }
    }

    /// Creates a new Path that appends the given segment to the end of `self`
    pub(crate) fn append(&self, segment: FixedLenString) -> Path {
        let mut segments = self.0.clone();
        segments.push(segment);
        Path(segments)
    }

    /// Creates a Path directly out of a set of segments
    pub(crate) fn from_segments(segments: &[FixedLenString]) -> Path {
        Path(segments.to_vec())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Attempts to construct a Path out of a given string that was, e.g.
    /// provided by a user.
    ///
    /// This is intended for user-facing calling, it
    /// currently does nothing to mitigate UTF-8 not matching the ADFS
    /// character set
    pub fn try_from_str(path: &str) -> Result<Path, String> {
        Path::from_bytes(path.as_bytes()).ok_or(format!("Could not convert '{path}' to ADFS path"))
    }

    /// The path as a sequence of variable-length byte-strings.
    ///
    /// The segments are trimmed to only include valid characters.
    pub fn to_byte_segments(&self) -> Vec<Vec<u8>> {
        let mut output = vec![];
        for segment in &self.0 {
            output.push(segment.valid_range().to_vec());
        }
        output
    }
}

impl Borrow<[FixedLenString]> for Path {
    fn borrow(&self) -> &[FixedLenString] {
        &self.0
    }
}

impl std::fmt::Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", Self::ROOT_SYMBOL as char)?;
        for dir in &self.0 {
            write!(f, "{}{dir}", Self::DIR_SEPARATOR as char)?;
        }
        Ok(())
    }
}

impl<I> Index<I> for Path
where
    I: std::slice::SliceIndex<[FixedLenString]>,
{
    type Output = <I as std::slice::SliceIndex<[FixedLenString]>>::Output;
    fn index(&self, index: I) -> &Self::Output {
        &self.0[index]
    }
}

impl<'a> Extend<&'a Path> for Path {
    fn extend<T: IntoIterator<Item = &'a Path>>(&mut self, iter: T) {
        for p in iter {
            self.0.extend(p.0.iter().copied());
        }
    }
}

#[cfg(test)]
mod test {
    use super::Path;
    use crate::new_map::util::FixedLenString;

    #[test]
    fn parse_paths() {
        assert_eq!(Path::from_bytes(b"$"), Some(Path(vec![])));
        assert_eq!(Path::from_bytes(b"$."), Some(Path(vec![])));
        assert_eq!(
            Path::from_bytes(b"$.Utilities.!TeleRoute.Templates"),
            Some(Path(vec![
                FixedLenString::from_bytes_dynamic(b"Utilities"),
                FixedLenString::from_bytes_dynamic(b"!TeleRoute"),
                FixedLenString::from_bytes_dynamic(b"Templates"),
            ]))
        );
        assert_eq!(Path::from_bytes(b"$.AAAAAAAAAAAAAAAAAA"), None);
        assert_eq!(
            Path::from_bytes(b"$.AAAA.BBB."),
            Some(Path(vec![
                FixedLenString::from_bytes_dynamic(b"AAAA"),
                FixedLenString::from_bytes_dynamic(b"BBB")
            ]))
        );
        assert_eq!(
            Path::from_bytes(b"$.Utilities.!TeleRoute.Templates")
                .unwrap()
                .to_string(),
            "$.Utilities.!TeleRoute.Templates"
        );
        assert_eq!(Path::from_bytes(b"$.Foo\0o.Bar"), None);
    }

    #[test]
    fn path_as_bytes() {
        let p = Path::from_bytes(b"$.Utilities.!TeleRoute.Templates").unwrap();
        assert_eq!(
            p.to_byte_segments(),
            vec![b"Utilities" as &[u8], b"!TeleRoute", b"Templates"]
        );
    }

    #[test]
    fn prefixes() {
        let cases = [
            ("$", "$.A", true),
            ("$.A", "$.A", true),
            ("$.A", "$.AB", true),
            ("$.A", "$.A.B", true),
            ("$.B", "$.A", false),
            ("$.B.A", "$.A.B", false),
        ];

        for (prefix, haystack, expected) in cases {
            let needle = Path::from_bytes(prefix.as_bytes()).unwrap();
            let path = Path::from_bytes(haystack.as_bytes()).unwrap();
            let test = needle.is_prefix(&path);
            let test = test == expected;
            assert!(test, "'{prefix}' is not a prefix of '{haystack}'");
        }
    }
}
