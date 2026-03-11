// Structures defined as in http://www.riscos.com/support/developers/prm/filecore.html

use std::fmt::Debug;
use std::ops::Add;

use winnow::binary::{bits::bits, le_u8, le_u16, le_u32};
use winnow::combinator::{seq, trace};
use winnow::error::{ErrMode, FromExternalError};
use winnow::stream::Location;
use winnow::token::take;
use winnow::{Bytes, LocatingSlice, Parser};

use crate::{BitErr, BitInput, InputStream, LoadErrors, ParseError, ParseResult, take_ls_bit};

const STRICT_MODE: bool = true;

#[derive(Debug)]
pub struct FormatE {
    map: NewMap<0>,
}

impl FormatE {
    // Entry point for creating FormatE disks
    pub fn parse<'a>(bytes: &'a [u8]) -> ParseResult<'a, Self> {
        let mut input = LocatingSlice::new(Bytes::new(bytes));
        let map = NewMap::parse(&mut input)?;
        dbg!(input.current_token_start());

        Ok(FormatE { map })
    }
}

#[derive(Debug)]
struct NewMap<const ZONE_COUNT: usize> {
    leading_block: LeadingMapBlock,
    blocks: [MapBlock; ZONE_COUNT],
}

impl NewMap<0> {
    fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, Self> {
        let leading_block = LeadingMapBlock::parse(true, input)?;
        Ok(NewMap {
            leading_block,
            blocks: [],
        })
    }
}

#[derive(Debug)]
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
        let allocations = AllocationBytes::make_parser(&params).parse_next(input)?;
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

#[derive(Debug)]
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

        let allocations = AllocationBytes::make_parser(&params).parse_next(input)?;
        let remainder = disc.sector_size() - (input.current_token_start() % disc.sector_size());
        let unused = Vec::from(take(remainder).parse_next(input)?);

        Ok(MapBlock {
            header,
            allocations,
            unused,
        })
    }
}

#[derive(Debug)]
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

#[derive(Debug)]
struct DiscRecord {
    log2_sec_size: u8,
    secs_per_track: u8,
    heads: u8,
    density: u8,
    idlen: u8,
    log2_bytes_per_mapbit: u8,
    skew: u8,
    boot_options: u8,
    low_sector: u8,
    num_zones: u8,
    zone_spare: u16,
    root: u32,
    size: u32,
    disc_id: u16,
    disc_name: FixedString,
    disc_type: u32,
}

impl DiscRecord {
    fn fragment_block_size(&self) -> usize {
        self.log2_bytes_per_mapbit as _
    }
    fn sector_size(&self) -> usize {
        2u32.pow(self.log2_sec_size as _) as _
    }
    fn zone_size_in_bytes(&self) -> usize {
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
                    root: le_u32,
                    size: le_u32,
                    disc_id: le_u16,
                    disc_name: FixedString::parse,
                    disc_type: le_u32,
                    _: take(24usize), // overall structure is 64 bytes long, tail end is reserved
                }
            },
        )
        .parse_next(input)
    }
}

#[derive(Debug, Clone, Copy)]
struct AllocationParsingParams {
    mapped_space_in_alloc_units: usize,
    fragment_id_length: usize,
    log_bytes_per_alloc: usize,
    sector_size: usize,
    free_link: u16,
}

impl AllocationParsingParams {
    fn new(zone_includes_map: bool, free_link: u16, disk: &DiscRecord) -> AllocationParsingParams {
        let orig_zone_size = disk.zone_size_in_bytes();
        let zone_size_in_bytes = if zone_includes_map {
            orig_zone_size - (disk.num_zones as usize * disk.sector_size())
        } else {
            orig_zone_size
        };
        let mapped_space_in_alloc_units =
            zone_size_in_bytes / 2usize.pow(disk.log2_bytes_per_mapbit as u32);

        AllocationParsingParams {
            mapped_space_in_alloc_units,
            fragment_id_length: disk.idlen as _,
            log_bytes_per_alloc: disk.log2_bytes_per_mapbit as _,
            sector_size: disk.sector_size(),
            free_link,
        }
    }
    fn sector_size(&self) -> usize {
        self.sector_size
    }
}

#[derive(Debug)]
struct AllocationBytes {
    fragments: Vec<FragmentBlock>,
}
impl AllocationBytes {
    fn make_parser<'a>(
        params: &AllocationParsingParams,
    ) -> impl Parser<InputStream<'a>, Self, ErrMode<ParseError<'a>>> {
        trace(
            "AllocationBytes",
            move |input: &mut InputStream<'a>| -> Result<AllocationBytes, ErrMode<ParseError<'a>>> {
                let mut bits_remaining = params.mapped_space_in_alloc_units;

                let mut fragments = bits(|input: &mut BitInput<'a>| {
                    let mut fragments = vec![];
                    while bits_remaining > 0 {
                        let fragment_block =
                            FragmentBlock::make_parser(params).parse_next(input)?;
                        dbg!(bits_remaining, fragment_block.total_length + 1);
                        bits_remaining =
                            bits_remaining.saturating_sub(fragment_block.total_length + 1);

                        fragments.push(fragment_block);
                    }
                    Result::<_, ErrMode<_>>::Ok(fragments)
                })
                .parse_next(input)?;

                Self::walk_free_chain(&mut fragments, params.free_link)
                    .map_err(|e| ErrMode::from_external_error(input, e))?;

                Ok(AllocationBytes { fragments })
            },
        )
    }
    fn walk_free_chain(fragments: &mut [FragmentBlock], free_link: u16) -> Result<(), LoadErrors> {
        let free_link_from_zero = 8 + free_link; // Free link value on disc is counting from overall disk offset 1
        let free_link_position = BitPosition(free_link_from_zero as usize);
        let head_idx = match fragments.binary_search_by_key(&free_link_position, |f| f.position) {
            Ok(idx) => idx,
            Err(byte_error) => return Err(LoadErrors::InvalidFreeLink(free_link)),
        };
        let head = &mut fragments[head_idx];
        head.free_space = true;

        let FragmentBlock {
            id: mut cursor_id,
            position: mut cursor_position,
            ..
        } = *head;

        while cursor_id != 0 {
            let dest_bit_offset = BitPosition(cursor_id as _) + cursor_position;
            let idx = fragments
                .binary_search_by_key(&dest_bit_offset, |f| f.position)
                .map_err(|_| LoadErrors::BrokenFreeChain {
                    origin: cursor_position,
                    dest_bit_offset,
                })?;
            let new_fragment = &mut fragments[idx];
            new_fragment.free_space = true;
            FragmentBlock {
                id: cursor_id,
                position: cursor_position,
                ..
            } = *new_fragment;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BitPosition(pub(crate) usize);
impl BitPosition {
    fn split(&self) -> (usize, usize) {
        (self.0 / 8, self.0 % 8)
    }
}
impl Add for BitPosition {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        BitPosition(self.0 + rhs.0)
    }
}
impl Debug for BitPosition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (bytes, bits) = self.split();
        f.debug_struct("BitPosition")
            .field("val", &self.0)
            .field("bytes", &bytes)
            .field("bits", &bits)
            .finish()
    }
}

#[derive(Debug, Clone, Copy)]
struct FragmentBlock {
    id: u16, // "...the fragment id cannot be more than 15 bits long."
    free_space: bool,
    total_length: usize,
    byte_size: usize,
    position: BitPosition,
}
impl FragmentBlock {
    fn make_parser<'a>(
        params: &AllocationParsingParams,
    ) -> impl Parser<BitInput<'a>, Self, BitErr<'a>> {
        trace("FragmentBlock", move |input: &mut BitInput<'a>| {
            let idlen = params.fragment_id_length;
            let position = BitPosition(8 * input.0.current_token_start() + input.1);
            let mut id = 0;

            for n in 0..idlen {
                id |= if take_ls_bit(input)? { 1 } else { 0 } << n;
            }

            let mut total_length = idlen;
            while !take_ls_bit(input)? {
                total_length += 1;
            }
            total_length += 1; // Count the terminating 1 bit

            let byte_size = total_length << params.log_bytes_per_alloc;
            debug_assert!(
                byte_size.is_multiple_of(params.sector_size()),
                "{byte_size} % {} != 0",
                params.sector_size
            );

            Ok(FragmentBlock {
                id,
                free_space: false,
                byte_size,
                total_length,
                position,
            })
        })
    }
}

const STRING_LEN: usize = 10;
struct FixedString([u8; STRING_LEN]);

impl Debug for FixedString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = String::from_utf8_lossy(&self.0);
        write!(f, "FixedString({s})")
    }
}

impl FixedString {
    fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, Self> {
        trace("FixedString", |input: &mut InputStream<'a>| {
            let o = *take(STRING_LEN).parse_next(input)?.first_chunk().unwrap();
            Ok(FixedString(o))
        })
        .parse_next(input)
    }
}
