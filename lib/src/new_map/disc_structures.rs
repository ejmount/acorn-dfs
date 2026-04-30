// Overall global metadata structures for NewMap formats defined in http://www.riscos.com/support/developers/prm/filecore.html
//
// This does not include structures for ordinary filesystem entries such as
// directory records.

use std::collections::HashMap;
use std::fmt::Debug;
use std::ops::{BitXor, Range};

use serde::Serialize;
use winnow::binary::bits::bits;
use winnow::binary::{le_u8, le_u16, le_u32};
use winnow::combinator::{seq, trace};
use winnow::error::{EmptyError, ErrMode, FromExternalError};
use winnow::stream::{Location, Stream};
use winnow::token::take;
use winnow::{ModalResult, Parser};

use super::filesystem::DirEntry;
use super::util::{
    AllocationParsingParams,
    BitErr,
    BitInput,
    BitPosition,
    DiscPosition,
    FixedLenString,
    FragmentId,
    InputStream,
    ParseError,
    ParseResult,
    take_ls_bit,
};
use super::{Fault, STRICT_MODE};
use crate::new_map::FaultValue;
use crate::new_map::util::{FaultableResult, take_rest_of_sector};

/// The offset of the allocation map from the beginning of the disk
//////
/// Used to calculate the disk-absolute ranges associated with each allocation
const ALLOCATION_MAP_START_IN_BITS: usize = (3 + 61) * 8;

/// The xor of the cross check header values should be this value
///
/// https://www.riscos.com/support/developers/prm/filecore.html#98861
const EXPECTED_CROSS_CHECK: u8 = 0xFF;

/// The "new"-style file allocation map structure, used by format E and F disks.
///
/// Format E and F disks are conceptually divided into a number of zones, with
/// one [`MapBlock`] per zone. Conceptually, the first Map Block is special
/// because it contains the [`DiscRecord`], which charcteristics the geometry of
/// the disk. Parsing the collection of `MapBlocks` is not straightforward
/// because the exact size of a `MapBlock` is defined by the disc geometry
/// recorded in the `DiscRecord`.
///
/// In practice, we ignore the DR being part of the first MB and instead
/// represent it at the higher level as part of the overall Map, which allows
/// handling for the MapBlocks to be uniform.
#[derive(Debug, Clone, Serialize)]
pub struct NewMap {
    /// Various metadata defining the goemetry of the disc
    ///
    /// On disk, this is stored inside the first MapBlock, but is pulled out
    /// here so Map handling is more consistent
    disc_record: DiscRecord,
    /// Trailing blocks. This may be empty if there is only one zone.
    blocks: Vec<MapBlock>,
}

impl NewMap {
    /// Construct a format-E NewMap out of the given byte stream
    pub fn parse<'a>(input: &mut InputStream<'a>, num_zones: usize) -> FaultableResult<'a, Self> {
        assert!(num_zones > 0);
        let FaultValue((leading_block, disc_record), mut faults) =
            MapBlock::parse_with_disc_record(true, input)?;

        let mut blocks = vec![leading_block];

        for _ in 0..(num_zones - 1) {
            let FaultValue(block, sub_faults) = MapBlock::parse(input, false, &disc_record)?;
            faults.extend(sub_faults);
            blocks.push(block);
        }

        let new_map = NewMap {
            disc_record,
            blocks,
        };

        if new_map.cross_check() != EXPECTED_CROSS_CHECK {
            faults.push(Fault::CrossCheckFailure(new_map.cross_check()));
        }

        Ok(FaultValue(new_map, faults))
    }
    pub(crate) fn get_disc_record(&self) -> &DiscRecord {
        &self.disc_record
    }

    pub(crate) fn get_allocation(&self, idx: usize) -> &AllocationMap {
        match idx {
            0 => &self.blocks[0].allocations,
            n => &self.blocks[n - 1].allocations,
        }
    }
    pub(crate) fn get_fragment(&self, id: FragmentId) -> Option<&FragmentBlock> {
        // TODO: Update this for multiple zones
        let mut fragment = self.blocks[0].get_fragment(id);
        for b in &self.blocks {
            fragment = fragment.or(b.get_fragment(id))
        }
        fragment
    }
    pub(crate) fn get_file_region(&self, dir_entry: &DirEntry) -> Option<Range<usize>> {
        let position = dir_entry.address;
        let fragment = self.get_fragment(position.fragment())?;

        let sector_number = position.sector_idx();
        let sector_size = self.disc_record.sector_size_in_bytes();
        let byte_offset: usize = sector_number as usize * sector_size;

        let Range { start, end } = fragment.disk_region();
        let end = end.min(start + dir_entry.len as usize);

        Some(Range {
            start: start + byte_offset,
            end,
        })
    }

    pub fn cross_check(&self) -> u8 {
        self.blocks
            .iter()
            .map(|b| b.header.cross_check)
            .reduce(BitXor::bitxor)
            .expect("Map should not contain zero blocks")
    }
}

/// The MapBlock ordinarily contains
/// 1. various validation checksums in thte [`Header`]
/// 2. the beginning of the free list for this zone, also in the [`Header`]
/// 3. a section of the allocation map
///
/// A MapBlock is also padded to be exactly one disk sector long, which means
/// the DiscRecord must be accessible. For the first block, we have this earlier
/// in the parsing step, but otherwise it must be passed in.
#[derive(Debug, Clone, Serialize)]
struct MapBlock {
    header: Header,
    #[serde(skip)]
    allocations: AllocationMap,
    #[serde(skip)]
    /// the remainder of the sector
    _unused: Vec<u8>,
}

impl MapBlock {
    fn parse<'a>(
        input: &mut InputStream<'a>,
        includes_map: bool,
        disc: &DiscRecord,
    ) -> FaultableResult<'a, Self> {
        let header = Header::parse(input)?;
        let params = AllocationParsingParams::new(includes_map, header.free_link, disc);

        let allocations = AllocationMap::parse(input, &params)?;
        let remainder = disc.sector_size_in_bytes()
            - (input.current_token_start() % disc.sector_size_in_bytes());
        let _unused = Vec::from(take(remainder).parse_next(input)?);

        // TODO: Add zone_check validation

        Ok(FaultValue(
            MapBlock {
                header,
                allocations,
                _unused,
            },
            vec![],
        ))
    }
    fn parse_with_disc_record<'a>(
        includes_map: bool,
        input: &'_ mut InputStream<'a>,
    ) -> FaultableResult<'a, (Self, DiscRecord)> {
        let sector_start = input.checkpoint();

        let header = Header::parse(input)?;
        let disc_record = DiscRecord::parse(input)?;
        let params = AllocationParsingParams::new(includes_map, header.free_link, &disc_record);
        let allocations = AllocationMap::parse(input, &params)?;
        let _unused = take_rest_of_sector(input, disc_record.sector_size_in_bytes())?.to_vec();

        let sector_end = input.checkpoint();
        input.reset(&sector_start);
        let sector_contents = take(params.sector_size())
            .parse_next(input)
            .unwrap_or_else(|_: EmptyError| unreachable!(" should've already succedeed"));

        let actual_zone_check = MapBlock::calculate_zone_check(sector_contents);
        dbg!(actual_zone_check, header.zone_check);
        input.reset(&sector_end);

        let faults = if actual_zone_check != header.zone_check {
            vec![Fault::ZoneCheckFailure {
                actual: actual_zone_check,
                expected: header.zone_check,
            }]
        } else {
            vec![]
        };

        Ok(FaultValue(
            (
                MapBlock {
                    header,
                    allocations,
                    _unused,
                },
                disc_record,
            ),
            faults,
        ))
    }
    fn get_fragment(&self, id: FragmentId) -> Option<&FragmentBlock> {
        self.allocations.get_fragment(id)
    }

    /// Zone checksum
    /// https://www.riscos.com/support/developers/prm/filecore.html#72701
    fn calculate_zone_check(zone: &[u8]) -> u8 {
        use std::ops::BitXor;

        let mut sum = [0u32; 4];
        for rover in (4..zone.len()).step_by(4).rev() {
            sum[0] += zone[rover] as u32 + (sum[3] >> 8);
            sum[3] &= 0xFF;
            sum[1] += zone[rover + 1] as u32 + (sum[0] >> 8);
            sum[0] &= 0xFF;
            sum[2] += zone[rover + 2] as u32 + (sum[1] >> 8);
            sum[1] &= 0xFF;
            sum[3] += zone[rover + 3] as u32 + (sum[2] >> 8);
            sum[2] &= 0xFF;
        }

        sum[0] += sum[3] >> 8;
        sum[1] += zone[1] as u32 + (sum[0] >> 8);
        sum[2] += zone[2] as u32 + (sum[1] >> 8);
        sum[3] += zone[3] as u32 + (sum[2] >> 8);
        sum.into_iter().reduce(u32::bitxor).unwrap() as u8
    }
}

/// A prefix of a [`MapBlock`], containing various checks and the pointer to
/// beginning of the free list
#[derive(Debug, Clone, Serialize)]
struct Header {
    /// A checksum of the parent MapBlock
    zone_check: u8,
    /// Pointer to the first free fragment in this zone, relative to the
    /// beginning of the same zone's allocation map
    ///
    /// The value that is stored on disk is described as:
    /// offset in bits to first free space in zone, or 0 if none, with top bit
    /// always set
    /// https://www.chiark.greenend.org.uk/~theom/riscos/docs/ultimate/a252efmt.txt
    ///
    /// When actually parsed, we automatically lower the top bit
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
                    free_link: le_u16.map(|n| n & 0x7FFF),
                    cross_check: le_u8,
                }
            },
        )
        .parse_next(input)
    }
}

/// Various metadata describing the overall disk geometry and "global"
/// filesystem metadata. Some values are particularly important as they are
/// needed to parse other structures on disk:
/// 1. `log2_sec_size`: the sector size, stored in its base-2 log form
/// 2. `idlen`: the length of a [`FragmentId`], in bits
/// 3. `root_dir`: the position of the root directory record, as a byte offset
/// 4. `size`: the total size of the disk, which then dictates how large the
///    Allocation Map is
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DiscRecord {
    pub(crate) log2_sec_size: u8,
    pub(crate) secs_per_track: u8,
    pub(crate) heads: u8,
    pub(crate) density: u8,
    pub(crate) idlen: u8,
    pub(crate) log2_bytes_per_mapbit: u8,
    pub(crate) skew: u8,
    // TODO: Model these better
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
    pub(crate) fn sector_size_in_bytes(&self) -> usize {
        2u32.pow(self.log2_sec_size as _) as _
    }
    pub(crate) fn zone_size_in_bytes(&self) -> usize {
        (self.size / self.num_zones as u32) as _
    }
    pub(crate) fn ids_per_zone(&self) -> usize {
        ((1 << (self.log2_sec_size as usize + 3)) - self.zone_spare as usize)
            / (self.idlen as usize + 1)
    }
    fn test_sector_size(s: u8) -> Result<u8, Fault> {
        if !STRICT_MODE || [8, 9, 10, 11].contains(&s) {
            Ok(s)
        } else {
            Err(Fault::UnacceptableSectorSize(s))
        }
    }
    fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, Self> {
        trace(
            "DiscRecord",
            seq! {
                DiscRecord {
                    log2_sec_size: le_u8.try_map(Self::test_sector_size),
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
                    disc_name: FixedLenString::parse_from_disk,
                    disc_type: le_u32,
                    _: take(24usize), // overall structure is 60 bytes long, tail end is reserved
                }
            },
        )
        .parse_next(input)
    }
}

/// The Allocation Map represents how the space across the disk is assigned to
/// "fragments," and is encoded as a *bit* stream with least-significant bits
/// read first.
///
/// Each allocation within the disk is represented (in disk order) by a
/// "fragment block," which consists of,
/// 1. a [`FragmentId`], the length of which is defined by [`DiscRecord::idlen`]
///    and which is built from the stream "least first"
/// 2. some number of `0` bits
/// 3. a terminating `1`
///
/// The *total* bit length (both idlen and terminating `1` included) of the
/// fragment block is the number of "allocation units" that the allocation takes
/// up on disk. The *log* of the size (in bytes) of a single allocation unit is
/// defined by [`DiscRecord::log2_bytes_per_mapbit`].
///
/// The allocation map contains no *gaps* - every bit within it assigns disk
/// space to some fragment - so some fragments are actually representing free
/// space. This is constructed as a linked list:
/// - the [`Header::free_link`] value is the offset, in bits, counting from zone
///   byte `0x1` (i.e. the free link value itself) of the first fragment that is
///   representing free space
/// - each fragment in the list contains an ID that is an offset, in bits, from
///   the beginning of that fragment to the beginning of the next one
/// - the final fragment in the free list has ID 0
///
/// Multiple fragment blocks may have the same fragment ID, which means they
/// represent discontiguous chunks of the same "disc object," e.g. a
/// file
#[derive(Clone)]
pub struct AllocationMap {
    fragments: HashMap<BitPosition, FragmentBlock>,
    object_regions: HashMap<FragmentId, Vec<Range<usize>>>,
}
impl AllocationMap {
    fn parse<'a>(
        input: &mut InputStream<'a>,
        params: &AllocationParsingParams,
    ) -> ParseResult<'a, Self> {
        trace(
            "AllocationBytes",
            move |input: &mut InputStream<'a>| -> Result<AllocationMap, ErrMode<ParseError<'a>>> {
                let mut bits_remaining = params.mapped_space_in_alloc_units();

                // The process here is to read and digest the entire collection of fragment
                // blocks...
                let mut fragments = bits(|input: &mut BitInput<'a>| -> Result<_, ErrMode<_>> {
                    let mut fragments = HashMap::new();
                    while bits_remaining > 0 {
                        let fragment_block = FragmentBlock::parse(input, params)?;

                        bits_remaining =
                            bits_remaining.saturating_sub(fragment_block.map_length + 1);

                        fragments.insert(fragment_block.position, fragment_block);
                    }
                    Ok(fragments)
                })
                .parse_next(input)?;

                // ...and only after that, flag the ones that are part of the free list
                if params.free_link() != 0 {
                    Self::walk_free_chain(&mut fragments, params.free_link())
                        .map_err(|e| ErrMode::from_external_error(input, e))?;
                }

                let fragment_regions = Self::build_fragment_map(&fragments, params);

                Ok(AllocationMap {
                    fragments,
                    object_regions: fragment_regions,
                })
            },
        )
        .parse_next(input)
    }

    /// Walks the list of free-space fragments beginning from the given
    /// `free_link` value, modifying the appropriate fragments.
    ///
    /// This can fail if:
    /// - [`Fault::InvalidFreeLink`]: the inital `free_link` does not point to a
    ///   valid fragment
    /// - [`Fault::BrokenFreeChain`]: one of the intermediate free fragments
    ///   does not have a valid successor
    fn walk_free_chain(
        fragments: &mut HashMap<BitPosition, FragmentBlock>,
        free_link: u16,
    ) -> Result<(), Fault> {
        let free_link_from_zero = 8 + free_link; // Free link value on disc is counting in bits from overall zone offset byte 0x01
        let free_link_position = BitPosition::from_bits(free_link_from_zero as usize);
        let head_fragment = fragments
            .get_mut(&free_link_position)
            .ok_or(Fault::InvalidFreeLink(free_link))?;
        head_fragment.free_space = true;

        let FragmentBlock {
            id: mut cursor_id,
            position: mut cursor_position,
            ..
        } = *head_fragment;

        while cursor_id != 0 {
            let dest_bit_offset = BitPosition::from_bits(cursor_id as _) + cursor_position;

            let new_fragment =
                fragments
                    .get_mut(&dest_bit_offset)
                    .ok_or(Fault::BrokenFreeChain {
                        dest_bit_offset,
                        origin: cursor_position,
                    })?;
            new_fragment.free_space = true;

            FragmentBlock {
                id: cursor_id,
                position: cursor_position,
                ..
            } = *new_fragment;
        }
        Ok(())
    }

    /// Build the map of which disc regions belong to which disc objects, in the
    /// proper order
    fn build_fragment_map(
        blocks: &HashMap<BitPosition, FragmentBlock>,
        params: &AllocationParsingParams,
    ) -> HashMap<FragmentId, Vec<Range<usize>>> {
        let mut fragment_regions: HashMap<_, Vec<_>> = HashMap::new();
        for block in blocks.values() {
            fragment_regions
                .entry(block.id)
                .or_default()
                .push(block.disk_region());
        }
        for (&fid, v) in &mut fragment_regions {
            // The regions need to be put in the order that results from searching for
            // regions belonging to disc object F starting from the zone
            // numbered `(F / ids per zone)` and wrapping around the end of the
            // disc
            //
            // https://www.riscos.com/support/developers/prm/filecore.html#32170
            v.sort_by_key(|id| {
                let start = params.search_starting_point(fid);
                id.start.wrapping_sub(start) % params.total_disk_size
            });
        }
        fragment_regions
    }

    /// Get the FragmentBlock of the given ID.
    ///
    /// Currently O(n) w.r.t. how many fragments there are
    pub fn get_fragment(&self, id: FragmentId) -> Option<&FragmentBlock> {
        // TODO: Object 2, being the object which carries the map with it, is special.
        // It is always at the beginning of the middle zone, as opposed to being
        // at the beginning of zone 0.
        self.fragments
            .iter()
            .find_map(|(_, f)| (f.id == id).then_some(f))
    }
}

impl Debug for AllocationMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut f = f.debug_struct("AllocationMap");

        let mut blocks: Vec<_> = self.fragments.iter().collect();
        blocks.sort_by_key(|bp| bp.0);
        f.field("blocks", &blocks);

        let mut fragment_regions: Vec<_> = self.object_regions.iter().collect();
        fragment_regions.sort_by_key(|(k, _)| **k);
        f.field("fragment_regions", &fragment_regions);
        f.finish()
    }
}

/// An entry in the [`AllocationMap`] representing some sort of allocation on
/// the disk.
#[derive(Debug, Clone)]
pub struct FragmentBlock {
    /// The `id` is the only data the disc explicitly records, the other fields
    /// are stored to simplify further logic and/or debugging
    id: FragmentId,
    /// Whether this block represents a region of free space
    free_space: bool,
    /// The length, in bits this block takes up inside the `AllocationMap`
    map_length: usize,
    /// the position within the *overall disk* this block starts at
    position: BitPosition,
    /// the range of bytes on disk this fragment is defining
    disk_region: Range<usize>,
}
impl FragmentBlock {
    pub fn disk_region(&self) -> Range<usize> {
        self.disk_region.clone()
    }
    fn parse<'a>(
        input: &mut BitInput<'a>,
        params: &AllocationParsingParams,
    ) -> ModalResult<Self, BitErr<'a>> {
        trace("FragmentBlock", move |input: &mut BitInput<'a>| {
            let idlen = params.fragment_id_length();
            let position = BitPosition::from_bits(8 * input.0.current_token_start() + input.1);
            let mut id = FragmentId::default();

            for n in 0..idlen {
                id |= if take_ls_bit(input)? { 1 } else { 0 } << n;
            }

            let mut map_length = idlen;
            while !take_ls_bit(input)? {
                map_length += 1;
            }
            map_length += 1; // Count the terminating 1 bit

            let position_from_start = position.bits() - ALLOCATION_MAP_START_IN_BITS;
            let disk_start = position_from_start * params.bytes_per_alloc_unit();
            let disk_end = disk_start + map_length * params.bytes_per_alloc_unit();

            let byte_size = disk_end - disk_start;
            debug_assert!(
                byte_size.is_multiple_of(params.sector_size()),
                "Fragment at {:?} is not a whole sector - {byte_size} % {} != 0",
                position,
                params.sector_size()
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
