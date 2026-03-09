// Structures defined as in http://www.riscos.com/support/developers/prm/filecore.html

use std::fmt::Debug;

use crate::{BitInput, InputStream, LenParser, LoadErrors, ParseError, ParseResult, take_ls_bit};
use winnow::binary::{bits, le_u8, le_u16, le_u32};
use winnow::combinator::{seq, trace};
use winnow::error::{EmptyError, ErrMode, TreeError};
use winnow::stream::Location;
use winnow::token::take;
use winnow::{Bytes, LocatingSlice, Parser};

const STRICT_MODE: bool = true;

#[derive(Debug)]
pub struct FormatE {
    map: NewMap<0>,
}

impl FormatE {
    // Entry point for creating FormatE disks
    pub fn parse<'a>(bytes: &'a [u8]) -> ParseResult<'a, Self> {
        let mut input = LocatingSlice::new(Bytes::new(bytes));

        Ok(FormatE {
            map: NewMap::parse(&mut input)?,
        })
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
        let (disc_record, dr_len) = DiscRecord::parse.with_len().parse_next(input)?;
        let params =
            AllocationParsingParams::new(includes_map, header.free_link as _, &disc_record);
        println!("Before allocation: {}", input.current_token_start());
        let (allocations, alloc_len) = AllocationBytes::make_parser(&params)
            .with_len()
            .parse_next(input)?;
        let remainder = disc_record
            .sector_size()
            .saturating_sub_signed(dr_len as _)
            .saturating_sub(alloc_len);
        //let unused = take(remainder).parse_next(input)?.to_vec();
        Ok(LeadingMapBlock {
            header,
            disc_record,
            allocations,
            unused: vec![],
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
        let params = AllocationParsingParams::new(includes_map, header.free_link as _, disc);

        let (allocations, alloc_len) = AllocationBytes::make_parser(&params)
            .with_len()
            .parse_next(input)?;
        let remainder = disc.sector_size() - alloc_len;
        //let unused = take(remainder).parse_next(input)?.to_vec();
        Ok(MapBlock {
            header,
            allocations,
            unused: vec![],
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
    zone_size_in_bytes: usize,
    log_bytes_per_alloc: usize,
    zone_spare: usize,
    free_link: usize,
}

impl AllocationParsingParams {
    fn new(
        zone_includes_map: bool,
        free_link: usize,
        disk: &DiscRecord,
    ) -> AllocationParsingParams {
        let orig_zone_size = disk.zone_size_in_bytes();
        let zone_size_in_bytes = if zone_includes_map {
            orig_zone_size - (disk.num_zones as usize * disk.sector_size())
        } else {
            orig_zone_size
        };
        let mapped_space_in_alloc_units =
            zone_size_in_bytes / 2usize.pow(disk.log2_bytes_per_mapbit as u32);

        dbg!(AllocationParsingParams {
            mapped_space_in_alloc_units,
            zone_size_in_bytes,
            fragment_id_length: disk.idlen as _,
            log_bytes_per_alloc: disk.log2_bytes_per_mapbit as _,
            zone_spare: disk.zone_spare as _,
            free_link
        })
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
            move |input: &mut InputStream<'a>| -> Result<
                AllocationBytes,
                ErrMode<TreeError<LocatingSlice<&'a Bytes>, LoadErrors>>,
            > {
                let mut bits_remaining = params.mapped_space_in_alloc_units;
                println!("Starting pos: {}", input.current_token_start());
                println!("Starting bytes: {:x?}", &input[..5]);
                let mut fragments = vec![];

                while bits_remaining > 0 {
                    let tail = &input[..8];
                    let fragment_block = bits::bits(FragmentBlock::make_parser(
                        params.fragment_id_length,
                        params.log_bytes_per_alloc,
                    ))
                    .parse_next(input)?;

                    dbg!(bits_remaining, fragment_block.total_length);
                    if bits_remaining == 64 {
                        eprintln!("{:x}", tail);
                    }
                    bits_remaining -= fragment_block.total_length;

                    fragments.push(fragment_block);
                }
                println!("Ending pos: {}", input.current_token_start());

                Ok(AllocationBytes { fragments })
            },
        )
    }
}

#[derive(Debug)]
struct FragmentBlock {
    id: String, // "...the fragment id cannot be more than 15 bits long."
    total_length: usize,
    byte_size: usize,
    position: usize,
}
impl FragmentBlock {
    fn make_parser<'a>(
        idlen: usize,
        bytes_per_bit: usize,
    ) -> impl Parser<BitInput<'a>, Self, ErrMode<TreeError<BitInput<'a>, LoadErrors>>> {
        trace("FragmentBlock", move |input: &mut BitInput<'a>| {
            let low_len = idlen.min(8);
            let hi_len = idlen.saturating_sub(8usize);

            let low: u8 = bits::take(low_len).parse_next(input)?;
            let hi: u8 = bits::take(hi_len).parse_next(input)?;
            let id = ((hi as u16) << 8) + low as u16;
            if id == 2 {
                dbg!(low_len, hi_len);
            }
            let mut total_length = dbg!(idlen);
            while bits::pattern::<_, _, _, EmptyError>(0x00, 8usize)
                .parse_next(input)
                .is_ok()
            {
                total_length += 8;
                if id == 2 {
                    dbg!(total_length);
                }
            }
            while !take_ls_bit(input)? {
                total_length += 1;
            }
            if id == 2 {
                dbg!(total_length);
            }

            Ok(FragmentBlock {
                id: format!("{id:x} ({hi:08b} {low:08b})"),
                byte_size: total_length << bytes_per_bit,
                total_length,
                position: input.0.current_token_start(),
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
