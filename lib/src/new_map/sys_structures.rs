/// Structure that represent bookkeeping that the program is doing but which
/// does not map immediately to disk structures
use std::collections::BTreeMap;
use std::fmt::{Debug, Display};

use winnow::Parser;
use winnow::combinator::{opt, preceded, separated, terminated};
use winnow::error::{AddContext, TreeError};
use winnow::stream::Stream;

use super::disc_structures::NewMap;
use super::filesystem::{Attributes, DirEntry, Directory};
use super::util::{
    DiscPosition,
    FaultableResult,
    FixedLenString,
    InputStream,
    ParseResult,
    make_input,
};
use super::{Fault, FaultValue};

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
    pub map: NewMap<0>,
    /// A summarised copy of the filesystem tree - this does not directly
    /// correspond to any disk contents.
    pub tree: Option<FileTree>,
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
        let mut input = make_input(bytes);
        let map = NewMap::parse(&mut input)?;

        Ok(FormatE {
            image: bytes.to_vec(),
            map,
            tree: None,
            faults: vec![],
        })
    }

    /// Reads the directory heirachy and populates the `tree` field of the
    /// structure.
    pub fn expand_tree(&mut self) -> Result<(), TreeError<(), Fault>> {
        let input = make_input(&self.image);
        let FaultValue(tree, faults) = FileTree::new(&self.map, input)
            .map_err(|e| e.into_inner().unwrap().map_input(|_| ()))?;
        self.tree = Some(tree);
        self.faults.extend(faults);
        Ok(())
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
    const ROOT_SYMBOL: u8 = b'$';
    const DIR_SEPARATOR: u8 = b'.';
    /// Create a path from a byte-string representing the entire path.
    ///
    /// Paths do not exist in this form anywhere within ADFS disk structures, so
    /// this should not be used to read disk content. Instead, it is used for,
    /// e.g. human-provided input referring to file locations within the ADFS
    /// disk.
    ///
    /// Will return `None` if the provided path is invalid. This can be because
    /// the path is ill-formed, because a single segment is too long.
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

    pub(crate) fn join(&self, segment: FixedLenString) -> Path {
        let mut segments = self.0.clone();
        segments.push(segment);
        Path(segments)
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

/// An entry for the [`FileTree`], representing either a directory or a file.
///
/// This does not correspond neatly to disk structures, where a [`DirEntry`]
/// representing a file only exists as a field inside a [`Directory`]
#[derive(Debug, Clone)]
enum FileObject {
    Dir(Box<Directory>),
    File(DirEntry),
}

/// A list of every file and directory entry on the disk
#[derive(Debug, Clone)]
pub struct FileTree {
    /// A BTree ordered by Path lets us pull entries from a subdirectory easily
    files: std::collections::BTreeMap<Path, FileObject>,
}
impl Display for FileTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (k, v) in &self.files {
            writeln!(f, "{}: {:#?}", k, v)?;
        }
        Ok(())
    }
}

impl FileTree {
    /// Produce the entire FileTree
    ///
    /// Expects the entire disk image as input
    fn new<'a, const ZONES: usize>(
        map: &NewMap<ZONES>,
        mut input: InputStream<'a>,
    ) -> FaultableResult<'a, FileTree> {
        input.reset_to_start();

        let FaultValue(files, faults) = Self::build_tree(map, input)?;
        dbg!(&faults);
        Ok(FaultValue(FileTree { files }, faults))
    }

    fn build_tree<'a, const ZONES: usize>(
        map: &NewMap<ZONES>,
        input: InputStream<'a>,
    ) -> FaultableResult<'a, BTreeMap<Path, FileObject>> {
        let dr = map.get_disc_record();
        let root_link = dr.root_dir;
        let FaultValue(root, mut faults) =
            Self::retrieve_directory(map, input, root_link, dr.sector_size()).map_err(|e| {
                let c = input.checkpoint();
                e.add_context(
                    &input,
                    &c,
                    Fault::InvalidRoot {
                        root_link,
                        sector_size: dr.sector_size(),
                    },
                )
            })?;

        // Attach paths to invalid-attribute faults if any were raised
        // This specifically applies to any in the root directory
        faults.iter_mut().for_each(|f| {
            if let Fault::InvalidAttr { path, .. } = f {
                *path = Path::default();
            }
        });
        dbg!(&faults);

        let mut queue = vec![(Path::default(), root.clone())];

        let mut files = BTreeMap::new();
        files.insert(Path::default(), FileObject::Dir(Box::new(root)));

        while let Some((path, item)) = queue.pop() {
            for child in &item.entries {
                let new_path = path.join(child.obj_name);
                if child.attrs.contains(Attributes::DIR) {
                    let FaultValue(dir, mut cur_faults) =
                        match Self::retrieve_directory(map, input, child.address, dr.sector_size())
                        {
                            Ok(dir) => dir,
                            Err(_) => {
                                eprintln!("Failed");
                                continue;
                            }
                        };
                    queue.push((new_path.clone(), dir.clone()));
                    files.insert(new_path.clone(), FileObject::Dir(Box::new(dir)));
                    // Attach paths to fault codes again for general files
                    cur_faults.iter_mut().for_each(|f| {
                        if let Fault::InvalidAttr { path, .. } = f {
                            *path = new_path.clone()
                        }
                    });
                    faults.extend(cur_faults);
                } else {
                    files.insert(new_path, FileObject::File(child.clone()));
                }
            }
        }

        Ok(FaultValue(files, faults))
    }

    /// Retrieve the section of the disk that corresponds to the given address
    /// and parse it as [`Directory`] object.
    fn retrieve_directory<'a, const ZONES: usize>(
        map: &NewMap<ZONES>,
        input: InputStream<'a>,
        addr: DiscPosition,
        sector_size: usize,
    ) -> FaultableResult<'a, Directory> {
        let block = map.get_allocation(0).get_fragment(addr.fragment()).unwrap();
        let entry_region = block.disk_region();

        let mut cursor = input;
        let offset = addr.sector_idx() as usize * sector_size;
        cursor.next_slice(entry_region.start + offset);

        Directory::parse(&mut cursor)
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
}
