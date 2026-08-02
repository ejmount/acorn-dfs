use std::ops::Range;

use winnow::combinator::trace;
use winnow::stream::Location;
use winnow::{ModalResult, Parser};

use super::super::util::{BitErr, BitInput, BitPosition, FragmentId};
use super::ALLOCATION_MAP_START_IN_BITS;
use crate::new_map::util::{AllocationParsingParams, take_ls_bit};

/// An entry in the [`AllocationMap`] representing some sort of allocation
/// on the disk.
#[derive(Debug, Clone)]
pub struct FragmentBlock {
    /// The `id` is the only data the disc explicitly records, the other
    /// fields are stored to simplify further logic and/or debugging
    pub(crate) id: FragmentId,
    /// Whether this block represents a region of free space
    pub(crate) free_space: bool,
    /// The length, in bits this block takes up inside the `AllocationMap`
    pub(crate) map_length: usize,
    /// the position within the map block sector this block starts at.
    pub(crate) position: BitPosition,
    /// the range of bytes on disk this fragment is defining
    pub(crate) disk_region: Range<usize>,
}

impl FragmentBlock {
    pub fn disk_region(&self) -> Range<usize> {
        self.disk_region.clone()
    }
    pub(crate) fn parse<'a>(
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
