use std::collections::HashMap;
use std::fmt::Debug;

use winnow::Parser;
use winnow::binary::bits::bits;
use winnow::combinator::trace;
use winnow::error::{ErrMode, FromExternalError};

use super::super::Fault;
use super::super::util::{BitInput, FragmentId, InputStream, ParseError, ParseResult};
use super::fragment_block::FragmentBlock;
use crate::new_map::util::{AllocationParsingParams, BitPosition};

/// The Allocation Map represents how the space across the disk is assigned
/// to "fragments," and is encoded as a *bit* stream with
/// least-significant bits read first.
///
/// Each allocation within the disk is represented (in disk order) by a
/// "fragment block," which consists of,
/// 1. a [`FragmentId`], the length of which is defined by [`DiscRecord::idlen`]
///    and which is built from the stream "least first"
/// 2. some number of `0` bits
/// 3. a terminating `1`
///
/// The *total* bit length (both idlen and terminating `1` included) of the
/// fragment block is the number of "allocation units" that the allocation
/// takes up on disk. The *log* of the size (in bytes) of a single
/// allocation unit is defined by [`DiscRecord::log2_bytes_per_mapbit`].
///
/// The allocation map contains no *gaps* - every bit within it assigns disk
/// space to some fragment - so some fragments are actually representing
/// free space. This is constructed as a linked list:
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
    pub(crate) fragments: HashMap<BitPosition, FragmentBlock>,
}

impl AllocationMap {
    pub(crate) fn parse<'a>(
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

                // ...and only after that, flag the ones that are part of the free list.
                // (A free link of 0 would mean no free blocks.)
                if params.free_link() != 0 {
                    Self::walk_free_chain(&mut fragments, params.free_link())
                        .map_err(|e| ErrMode::from_external_error(input, e))?;
                }

                Ok(AllocationMap { fragments })
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
    pub(crate) fn walk_free_chain(
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

    /// Get the FragmentBlock(s) of the given ID.
    pub fn get_fragments(&self, id: FragmentId) -> Vec<FragmentBlock> {
        self.fragments
            .values()
            .filter(|f| f.id == id)
            .copied()
            .collect()
    }
}

impl Debug for AllocationMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut f = f.debug_struct("AllocationMap");

        let mut blocks: Vec<_> = self.fragments.iter().collect();
        blocks.sort_by_key(|bp| bp.0);
        f.field("blocks", &blocks);

        f.finish()
    }
}
