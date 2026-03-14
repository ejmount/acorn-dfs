// Overall global metadata structures for NewMap formats defined in http://www.riscos.com/support/developers/prm/filecore.html
// This does not include structures for ordinary filesystem entries such as directory and file entries

use std::collections::HashMap;
use std::fmt::Debug;
use std::ops::Range;

use winnow::binary::bits::bits;
use winnow::binary::{le_u8, le_u16, le_u32};
use winnow::combinator::seq;
use winnow::combinator::trace;
use winnow::error::{ErrMode, FromExternalError};
use winnow::stream::Location;
use winnow::token::take;
use winnow::{ModalResult, Parser};

use crate::new_map::util::FragmentId;
use crate::new_map::util::InputStream;
use crate::new_map::util::{
    AllocationParsingParams, BitErr, BitInput, BitPosition, DiscPosition, FixedLenString,
    ParseError, ParseResult, take_ls_bit,
};
use crate::new_map::{LoadErrors, STRICT_MODE};

const ALLOCATION_MAP_START_IN_BITS: usize = (3 + 61) * 8;

#[derive(Debug, Clone)]
pub struct NewMap<const ZONE_COUNT: usize> {
    leading_block: LeadingMapBlock,
    blocks: [MapBlock; ZONE_COUNT],
}

impl<const ZONES: usize> NewMap<ZONES> {
    pub(crate) fn get_disc_record(&self) -> &DiscRecord {
        &self.leading_block.disc_record
    }

    pub(crate) fn get_allocation(&self, idx: usize) -> &AllocationBytes {
        match idx {
            0 => &self.leading_block.allocations,
            n => &self.blocks[n - 1].allocations,
        }
    }
}

impl NewMap<0> {
    pub(crate) fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, Self> {
        let leading_block = LeadingMapBlock::parse(true, input)?;
        Ok(NewMap {
            leading_block,
            blocks: [],
        })
    }
}

#[derive(Debug, Clone)]
struct LeadingMapBlock {
    header: Header,
    disc_record: DiscRecord,
    allocations: AllocationBytes,
    unused: Vec<u8>,
}

impl LeadingMapBlock {
    fn parse<'a>(includes_map: bool, input: &'_ mut InputStream<'a>) -> ParseResult<'a, Self> {
        let header = Header::parse(input)?;
        let disc_record = DiscRecord::parse(input)?;
        let params = AllocationParsingParams::new(includes_map, header.free_link, &disc_record);
        let allocations = AllocationBytes::parse(input, &params)?;
        let remainder =
            disc_record.sector_size() - (input.current_token_start() % disc_record.sector_size());
        let unused = Vec::from(take(remainder).parse_next(input)?);
        Ok(LeadingMapBlock {
            header,
            disc_record,
            allocations,
            unused,
        })
    }
}

#[derive(Debug, Clone)]
struct MapBlock {
    header: Header,
    allocations: AllocationBytes,
    unused: Vec<u8>,
}

impl MapBlock {
    fn parse<'a>(
        input: &mut InputStream<'a>,
        includes_map: bool,
        disc: &DiscRecord,
    ) -> ParseResult<'a, Self> {
        let header = Header::parse(input)?;
        let params = AllocationParsingParams::new(includes_map, header.free_link, disc);

        let allocations = AllocationBytes::parse(input, &params)?;
        let remainder = disc.sector_size() - (input.current_token_start() % disc.sector_size());
        let unused = Vec::from(take(remainder).parse_next(input)?);

        Ok(MapBlock {
            header,
            allocations,
            unused,
        })
    }
}

#[derive(Debug, Clone)]
struct Header {
    zone_check: u8,
    free_link: u16,
    cross_check: u8,
}

impl Header {
    fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, Self> {
        trace(
            "Header",
            seq! {
                Header {
                    zone_check: le_u8,
                    // offset in bits to first free space in zone, or 0 if none, with top bit always set
                    // https://www.chiark.greenend.org.uk/~theom/riscos/docs/ultimate/a252efmt.txt
                    free_link: le_u16.map(|n| n & 0x7FFF),
                    cross_check: le_u8,
                }
            },
        )
        .parse_next(input)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DiscRecord {
    pub(crate) log2_sec_size: u8,
    pub(crate) secs_per_track: u8,
    pub(crate) heads: u8,
    pub(crate) density: u8,
    pub(crate) idlen: u8,
    pub(crate) log2_bytes_per_mapbit: u8,
    pub(crate) skew: u8,
    pub(crate) boot_options: u8,
    pub(crate) low_sector: u8,
    pub(crate) num_zones: u8,
    pub(crate) zone_spare: u16,
    pub(crate) root_dir: DiscPosition,
    pub(crate) size: u32,
    pub(crate) disc_id: u16,
    pub(crate) disc_name: FixedLenString,
    pub(crate) disc_type: u32,
}

impl DiscRecord {
    pub(crate) fn fragment_block_size(&self) -> usize {
        self.log2_bytes_per_mapbit as _
    }
    pub(crate) fn sector_size(&self) -> usize {
        2u32.pow(self.log2_sec_size as _) as _
    }
    pub(crate) fn zone_size_in_bytes(&self) -> usize {
        (self.size / self.num_zones as u32) as _
    }
    fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, Self> {
        trace(
            "DiscRecord",
            seq! {
                DiscRecord {
                    log2_sec_size: le_u8.verify(|s| !STRICT_MODE || [8,9,10].contains(s) ),
                    secs_per_track: le_u8,
                    heads: le_u8,
                    density: le_u8,
                    idlen: le_u8,
                    log2_bytes_per_mapbit: le_u8,
                    skew: le_u8,
                    boot_options: le_u8,
                    low_sector: le_u8,
                    num_zones: le_u8,
                    zone_spare: le_u16,
                    root_dir: le_u32.map(DiscPosition),
                    size: le_u32,
                    disc_id: le_u16,
                    disc_name: FixedLenString::parse,
                    disc_type: le_u32,
                    _: take(24usize), // overall structure is 60 bytes long, tail end is reserved
                }
            },
        )
        .parse_next(input)
    }
}

#[derive(Clone)]
pub struct AllocationBytes {
    fragments: HashMap<BitPosition, FragmentBlock>,
}
impl AllocationBytes {
    fn parse<'a>(
        input: &mut InputStream<'a>,
        params: &AllocationParsingParams,
    ) -> ParseResult<'a, Self> {
        trace(
            "AllocationBytes",
            move |input: &mut InputStream<'a>| -> Result<AllocationBytes, ErrMode<ParseError<'a>>> {
                let mut bits_remaining = params.mapped_space_in_alloc_units;

                let mut fragments = bits(|input: &mut BitInput<'a>| {
                    let mut fragments = HashMap::new();
                    while bits_remaining > 0 {
                        let fragment_block = FragmentBlock::parse(input, params)?;

                        bits_remaining =
                            bits_remaining.saturating_sub(fragment_block.map_length + 1);

                        fragments.insert(fragment_block.position, fragment_block);
                    }
                    Result::<_, ErrMode<_>>::Ok(fragments)
                })
                .parse_next(input)?;

                Self::walk_free_chain(&mut fragments, params.free_link)
                    .map_err(|e| ErrMode::from_external_error(input, e))?;

                Ok(AllocationBytes { fragments })
            },
        )
        .parse_next(input)
    }

    fn walk_free_chain(
        fragments: &mut HashMap<BitPosition, FragmentBlock>,
        free_link: u16,
    ) -> Result<(), LoadErrors> {
        let free_link_from_zero = 8 + free_link; // Free link value on disc is counting from overall disk offset 1
        let free_link_position = BitPosition(free_link_from_zero as usize);
        let head_fragment = fragments
            .get_mut(&free_link_position)
            .ok_or(LoadErrors::InvalidFreeLink(free_link))?;
        head_fragment.free_space = true;

        let FragmentBlock {
            id: mut cursor_id,
            position: mut cursor_position,
            ..
        } = *head_fragment;

        while cursor_id != 0 {
            let dest_bit_offset = BitPosition(cursor_id as _) + cursor_position;

            let new_fragment = fragments
                .get_mut(&dest_bit_offset)
                .ok_or(LoadErrors::InvalidFreeLink(free_link))?;
            new_fragment.free_space = true;

            FragmentBlock {
                id: cursor_id,
                position: cursor_position,
                ..
            } = *new_fragment;
        }
        Ok(())
    }

    pub fn get_fragment(&self, id: FragmentId) -> Option<&FragmentBlock> {
        self.fragments
            .iter()
            .find_map(|(_, f)| (f.id == id).then_some(f))
    }
}

impl Debug for AllocationBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut keys: Vec<_> = self.fragments.keys().collect();
        keys.sort_by_key(|bp| bp.0);
        let mut f = f.debug_map();
        for k in keys {
            f.entry(&k.0, &self.fragments[k]);
        }
        f.finish()
    }
}

#[derive(Debug, Clone)]
pub struct FragmentBlock {
    id: FragmentId, // "...the fragment id cannot be more than 15 bits long."
    free_space: bool,
    map_length: usize,
    position: BitPosition,
    disk_region: Range<usize>,
}
impl FragmentBlock {
    fn position(&self) -> BitPosition {
        self.position
    }
    pub fn disk_region(&self) -> Range<usize> {
        self.disk_region.clone()
    }
    fn parse<'a>(
        input: &mut BitInput<'a>,
        params: &AllocationParsingParams,
    ) -> ModalResult<Self, BitErr<'a>> {
        trace("FragmentBlock", move |input: &mut BitInput<'a>| {
            let idlen = params.fragment_id_length;
            let position = BitPosition(8 * input.0.current_token_start() + input.1);
            let mut id = FragmentId::default();

            for n in 0..idlen {
                id |= if take_ls_bit(input)? { 1 } else { 0 } << n;
            }

            let mut map_length = idlen;
            while !take_ls_bit(input)? {
                map_length += 1;
            }
            map_length += 1; // Count the terminating 1 bit

            let position_from_start = position.0 - ALLOCATION_MAP_START_IN_BITS;
            let disk_start = position_from_start * params.bytes_per_alloc_unit();
            let disk_end = disk_start + map_length * params.bytes_per_alloc_unit();

            let byte_size = disk_end - disk_start;
            debug_assert!(
                byte_size.is_multiple_of(params.sector_size()),
                "{byte_size} % {} != 0",
                params.sector_size
            );

            Ok(FragmentBlock {
                id,
                free_space: false,
                map_length,
                position,
                disk_region: disk_start..disk_end,
            })
        })
        .parse_next(input)
    }
}
