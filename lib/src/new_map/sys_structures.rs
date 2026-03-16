use std::{
    collections::BTreeMap,
    fmt::{Debug, Display},
};

use winnow::{
    LocatingSlice, Parser,
    error::{AddContext, EmptyError, TreeError},
    stream::{Offset, Stream},
    token::{rest, take_until},
};

use crate::new_map::{
    Fault, FaultValue,
    disc_structures::NewMap,
    filesystem::{Attributes, DirEntry, Directory, MAX_SEGMENT_LENGTH},
    util::{DiscPosition, FaultableResult, FixedLenString, InputStream, ParseResult, make_input},
};

const ROOT: u8 = b'$';
const DIR_SEPARATOR: u8 = b'.';

#[derive(Clone)]
pub struct FormatE {
    pub image: Vec<u8>,
    pub map: NewMap<0>,
    pub tree: Option<FileTree>,
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
    // Entry point for creating FormatE disks
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

    pub fn expand_tree(&mut self) -> Result<(), TreeError<(), Fault>> {
        let input = make_input(&self.image);
        let FaultValue(tree, faults) = FileTree::new(input, &self.map)
            .map_err(|e| e.into_inner().unwrap().map_input(|_| ()))?;
        self.tree = Some(tree);
        self.faults.extend(faults);
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Path(Vec<FixedLenString>);

impl Path {
    #[expect(
        const_item_mutation,
        reason = "Constant-parser shouldn't be mutated in practice"
    )]
    fn from_bytes(input: &[u8]) -> Result<Path, usize> {
        let mut input = LocatingSlice::new(input);
        let beginning = input.checkpoint();
        let cursor = &mut input;

        ROOT.parse_next(cursor)
            .map_err(|_: EmptyError| cursor.offset_from(&beginning))?;

        let mut segments = vec![];

        while !cursor.is_empty() {
            match DIR_SEPARATOR.parse_next(cursor) {
                Ok(_) => {}
                Err(()) => return Err(cursor.offset_from(&beginning)),
            }

            let segment = if cursor.contains(&DIR_SEPARATOR) {
                // Can't easily do a take_until with two possible ending points
                take_until(.., DIR_SEPARATOR)
                    .verify_map(FixedLenString::new)
                    .parse_next(cursor)
            } else {
                rest.verify_map(FixedLenString::new).parse_next(cursor)
            }
            .map_err(|_: EmptyError| cursor.offset_from(&beginning))?;

            segments.push(segment);
        }

        Ok(Path(segments))
    }
}

impl std::fmt::Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "$")?;
        for dir in &self.0 {
            write!(f, ".{dir}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
enum FileObject {
    Dir(Box<Directory>),
    File(DirEntry),
}

#[derive(Debug, Clone)]
pub struct FileTree {
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
    fn new<'a, const ZONES: usize>(
        mut input: InputStream<'a>,
        map: &NewMap<ZONES>,
    ) -> FaultableResult<'a, FileTree> {
        input.reset_to_start();

        let FaultValue(files, faults) = Self::build_tree(map, input)?;
        dbg!(&faults);
        Ok(FaultValue(FileTree { files }, faults))
    }

    fn build_tree<'a, const N: usize>(
        map: &NewMap<N>,
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
            //eprintln!("Found {path:?}");
            for child in &item.entries {
                let mut new_path = path.0.clone();
                new_path.push(child.obj_name);
                let new_path = Path(new_path);
                //eprintln!("Trying to find {new_path:?} at {:?}", child.address);
                if child.attrs.contains(Attributes::DIR) {
                    let FaultValue(dir, mut cur_faults) =
                        match Self::retrieve_directory(map, input, child.address, dr.sector_size())
                        {
                            Ok(dir) => dir,
                            Err(e) => {
                                eprintln!("Failed");
                                continue;
                            }
                        };
                    queue.push((new_path.clone(), dir.clone()));
                    files.insert(new_path.clone(), FileObject::Dir(Box::new(dir)));
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

    fn retrieve_directory<'a, const N: usize>(
        map: &NewMap<N>,
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
    use crate::new_map::{sys_structures::Path, util::FixedLenString};

    #[test]
    fn parse_paths() {
        assert_eq!(Path::from_bytes(b"$"), Ok(Path(vec![])));
        assert_eq!(
            Path::from_bytes(b"$.Utilities.!TeleRoute.Templates"),
            Ok(Path(vec![
                FixedLenString::new(b"Utilities").unwrap(),
                FixedLenString::new(b"!TeleRoute").unwrap(),
                FixedLenString::new(b"Templates").unwrap()
            ]))
        );
    }
}
