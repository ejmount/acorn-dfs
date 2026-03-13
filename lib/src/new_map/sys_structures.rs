use winnow::{Parser, combinator::trace, token::take};

use crate::new_map::{
    disc_structures::NewMap,
    filesystem::Directory,
    util::{ParseResult, make_input},
};

#[derive(Debug, Clone)]
pub struct FormatE {
    image: Vec<u8>,
    map: NewMap<0>,
    root_dir: Directory,
}

impl FormatE {
    // Entry point for creating FormatE disks
    pub fn parse<'a>(bytes: &'a [u8]) -> ParseResult<'a, Self> {
        let mut input = make_input(bytes);
        let map = NewMap::parse(&mut input)?;

        let dr = &map.get_disc_record();
        let root_link = dr.root_dir;

        let root_dir_region = map
            .get_allocation(0)
            .get_fragment(root_link.fragment())
            .unwrap()
            .disk_region();

        let mut clone = input;
        clone.reset_to_start();
        trace(
            "Jump",
            take(root_dir_region.start + (root_link.sector_idx() - 1) as usize * dr.sector_size()),
        )
        .parse_next(&mut clone)?;

        let root_dir = Directory::parse(&mut clone)?;
        dbg!(&root_dir);

        Ok(FormatE {
            image: bytes.to_vec(),
            map,
            root_dir,
        })
    }
}
