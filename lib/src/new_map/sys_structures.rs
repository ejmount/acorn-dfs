use std::{collections::BTreeMap, fmt::Debug};

use winnow::{
    error::{AddContext, TreeError},
    stream::Stream,
};

use crate::new_map::{
    LoadErrors,
    disc_structures::NewMap,
    filesystem::{Attributes, DirEntry, Directory},
    util::{DiscPosition, FixedLenString, InputStream, ParseResult, make_input},
};

#[derive(Clone)]
pub struct FormatE {
    pub image: Vec<u8>,
    pub map: NewMap<0>,
    pub tree: Option<FileTree>,
}
impl Debug for FormatE {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FormatE")
            .field("map", &self.map)
            .field("tree", &self.tree)
            .field("image", &&self.image[..10.min(self.image.len())])
            .finish()
    }
}

impl FormatE {
    // Entry point for creating FormatE disks
    pub fn parse<'a>(bytes: &'a [u8]) -> ParseResult<'a, Self> {
        let mut input = make_input(bytes);
        let map = NewMap::parse(&mut input)?;

        Ok(FormatE {
            image: bytes.to_vec(),
            map,
            tree: None,
        })
    }

    pub fn expand_tree(&mut self) -> Result<(), TreeError<(), LoadErrors>> {
        let input = make_input(&self.image);
        let tree = FileTree::new(input, &self.map)
            .map_err(|e| e.into_inner().unwrap().map_input(|_| ()))?;
        self.tree = Some(tree);
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Path(Vec<FixedLenString>);

#[derive(Debug, Clone)]
enum FileObject {
    Dir(Box<Directory>),
    File(DirEntry),
}

#[derive(Debug, Clone)]
pub struct FileTree {
    files: std::collections::BTreeMap<Path, FileObject>,
}
impl FileTree {
    fn new<'a, const ZONES: usize>(
        mut input: InputStream<'a>,
        map: &NewMap<ZONES>,
    ) -> ParseResult<'a, FileTree> {
        input.reset_to_start();

        let files = Self::build_tree(map, input)?;

        Ok(FileTree { files })
    }

    fn build_tree<'a, const N: usize>(
        map: &NewMap<N>,
        input: InputStream<'a>,
    ) -> ParseResult<'a, BTreeMap<Path, FileObject>> {
        let dr = map.get_disc_record();
        let root_link = dr.root_dir;
        let root =
            Self::retrieve_directory(map, input, root_link, dr.sector_size()).map_err(|e| {
                let c = input.checkpoint();
                e.add_context(
                    &input,
                    &c,
                    crate::new_map::LoadErrors::InvalidRoot {
                        root_link,
                        sector_size: dr.sector_size(),
                    },
                )
            })?;
        let mut queue = vec![(Path::default(), root.clone())];

        let mut files = BTreeMap::new();
        files.insert(Path::default(), FileObject::Dir(Box::new(root)));

        while let Some((path, item)) = queue.pop() {
            eprintln!("Found {path:?}");
            for child in &item.entries {
                let mut new_path = path.0.clone();
                new_path.push(child.obj_name);
                let new_path = Path(new_path);
                eprintln!("Trying to find {new_path:?} at {:?}", child.address);
                if child.attrs.contains(Attributes::DIR) {
                    let dir =
                        match Self::retrieve_directory(map, input, child.address, dr.sector_size())
                        {
                            Ok(dir) => dir,
                            Err(e) => {
                                eprintln!("Failed");
                                continue;
                            }
                        };
                    queue.push((new_path.clone(), dir.clone()));
                    files.insert(new_path, FileObject::Dir(Box::new(dir)));
                } else {
                    files.insert(new_path, FileObject::File(child.clone()));
                }
            }
        }

        Ok(files)
    }

    fn retrieve_directory<'a, const N: usize>(
        map: &NewMap<N>,
        input: InputStream<'a>,
        addr: DiscPosition,
        sector_size: usize,
    ) -> ParseResult<'a, Directory> {
        let block = map.get_allocation(0).get_fragment(addr.fragment()).unwrap();
        let entry_region = block.disk_region();

        let mut cursor = input;
        let offset = addr.sector_idx() as usize * sector_size;
        cursor.next_slice(entry_region.start + offset);

        Directory::parse(&mut cursor)
    }
}
