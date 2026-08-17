use std::cell::RefCell;
use std::fmt::Debug;
use std::io::Write;
use std::range::Range;

use winnow::Parser;
use winnow::stream::Stream;

use super::super::error::Fault;
use super::super::filesystem::{Attributes, Directory};
use crate::error::{Error, FaultValue, IoError};
use crate::filesystem::DirEntry;
use crate::new_map::new_map::NewMap;
use crate::sys_structures::{FileObject, FileTree, Path};
use crate::util::{DiscPosition, make_input};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    E,
    F,
    Automatic,
}

/// Represents the parsed contents of a ADFS "new map" (format E or F) disk.
///
/// The data between the fields of this structure are slightly redundant - the
/// `image` field contains the disk bytes, but other structures contain the same
/// data by value. This is to simplify lifetimes, and the disk is not expected
/// to ever be big enough for the redundancy to be a significant performance
/// problem.
#[derive(Clone)]
pub struct NewMapDisk {
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

impl Debug for NewMapDisk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FormatE")
            .field("map", &self.map)
            .field("tree", &self.tree)
            .field("image", &&self.image[..10.min(self.image.len())])
            .field("faults", &self.faults)
            .finish()
    }
}

impl NewMapDisk {
    // Entry point for creating Format-E disks. The resulting structure does not
    // populate the file tree.
    pub fn parse<'a>(bytes: &'a [u8], format: Format) -> Result<Self, Error<'a>> {
        let image = bytes.to_vec();
        let mut input = make_input(bytes);

        let FaultValue(map, mut faults) = match format {
            Format::E => NewMap::parse_format_e(&mut input)?,
            Format::F => NewMap::parse_format_f(&mut input)?,
            Format::Automatic => {
                let as_e = NewMap::parse_format_e(&mut input);
                let as_f;
                if let Ok(e) = as_e {
                    e
                } else {
                    input.reset_to_start();
                    as_f = NewMap::parse_format_f(&mut input);
                    if let Ok(f) = as_f {
                        f
                    } else {
                        panic!("Failed to parse\nAs format E: {as_e:?}\nAs format F: {as_f:?}")
                    }
                }
            }
        };

        // Retrieving the root directory object is special, so we do that upfront so
        // that other FS operations can assume the root is cached and walk down from
        // there.
        let FaultValue(tree, root_faults) = Self::create_initial_file_tree(bytes, &map)?;

        faults.extend(root_faults);

        Ok(NewMapDisk {
            image,
            map,
            tree: RefCell::new(tree),
            faults,
        })
    }

    pub fn get_map_json(&self) -> String {
        serde_json::to_string_pretty(&self.map).unwrap()
    }

    pub(crate) fn create_initial_file_tree<'a>(
        image: &'a [u8],
        map: &NewMap,
    ) -> Result<FaultValue<FileTree>, Error<'a>> {
        let mut tree = FileTree::new();
        let mut faults = vec![];

        let root_link = map.get_disc_record().root_dir;
        let sector_size = map.get_disc_record().sector_size_in_bytes();
        let FaultValue(root_dir, root_faults) = Self::retrieve_directory(
            map,
            image,
            root_link,
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
    /// - [`IoError::InvalidTarget`]: some subset of the Path points to a file
    ///   rather than a folder
    /// - [`ParseError`]: A directory structure on disk was invalid
    pub(crate) fn populate_path(&self, path: &Path) -> Result<FaultValue<()>, Error<'_>> {
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
                        return Ok(FaultValue::from(()));
                    } else {
                        return Err(IoError::InvalidTarget(stem).into());
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
                        return Err(IoError::InvalidTarget(stem).into());
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
                        return Err(IoError::MissingTarget(stem).into());
                    };
                    child_entry.clone()
                }
            };

            // If the next path segment refers to a file, we're done either way, but this is
            // an error if we still have path left to look up
            if !child_entry.attrs.contains(Attributes::DIR) {
                if stem == *path {
                    return Ok(FaultValue::from(()));
                } else {
                    return Err(IoError::InvalidTarget(stem).into());
                }
            }

            // Using the metadata from the child entry, look at the disk bytes for the
            // actual directory listing
            let FaultValue(child_directory, faults) = Self::retrieve_directory(
                &self.map,
                &self.image,
                child_entry.address,
                Fault::InvalidDir { path: stem.clone() },
            )?;
            // If the new entry has fault codes, store those
            overall_faults.extend(faults);

            // Populate entries (potentially sparse ones) for the path's children
            for child in &child_directory.entries {
                let child_path = stem.clone().append(child.obj_name);
                Self::insert_dir_entry(&mut self.tree.borrow_mut(), child.clone(), child_path);
            }

            // Upsert the cache entry.
            self.tree
                .borrow_mut()
                .insert(stem, FileObject::LoadedDir(Box::new(child_directory)));
        }
        Ok(FaultValue((), overall_faults))
    }

    pub(crate) fn insert_dir_entry(tree: &mut FileTree, child: DirEntry, child_path: Path) {
        if child.attrs.contains(Attributes::DIR) {
            tree.insert(child_path, FileObject::SparseDir(child));
        } else {
            tree.insert(child_path, FileObject::File(child));
        }
    }

    /// Retrieve the section of the disk that corresponds to the given address
    /// and parse it as [`Directory`] object.
    pub(crate) fn retrieve_directory<'a>(
        map: &NewMap,
        image: &'a [u8],
        addr: DiscPosition,
        context: impl Into<Option<Fault>>,
    ) -> Result<FaultValue<Directory>, Error<'a>> {
        let disc_object = map.get_disc_object(addr)?;

        let entry_regions = disc_object
            .get_disc_regions(addr.sector_idx() as _, Directory::LENGTH)
            .ok_or(IoError::OutOfBounds {
                object: addr,
                length: Directory::LENGTH,
            })?;

        let Range { start, end } = *entry_regions.first().unwrap();

        // TODO: Still not dealing with discontinuous directory structures
        assert!(end - start >= Directory::LENGTH);

        // Slightly convoluted but it means that parser errors have the right offset,
        // w.r.t the entire image
        let mut cursor = make_input(image);
        cursor.next_slice(start);

        let result = if let Some(c) = context.into() {
            Directory::parse.context(c).parse_next(&mut cursor)
        } else {
            Directory::parse.parse_next(&mut cursor)
        };
        result.map_err(Into::into)
    }
    /// Gets the metadata and contents of a given path
    pub fn get_file(
        &self,
        path: &Path,
        dest: &mut dyn Write,
    ) -> Result<FaultValue<DirEntry>, Error<'_>> {
        self.populate_path(path)?;

        // This should not be able to fail because we bailed if `populate_path` failed
        let fileobject = &self.tree.borrow()[path];

        let FileObject::File(dir_entry) = fileobject else {
            return Err(IoError::InvalidTarget(path.clone()).into());
        };

        let regions = self.map.get_file_regions(dir_entry)?;

        for r in regions {
            dest.write_all(&self.image[r]).map_err(IoError::StdError)?;
        }

        Ok(FaultValue::from(dir_entry.clone()))
    }

    pub fn entries(&self, prefix: Option<Path>) -> Result<Vec<Path>, Error<'_>> {
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
                self.populate_path(&entry)?;
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

        Ok(entries)
    }
}
