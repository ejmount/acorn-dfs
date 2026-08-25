use winnow::Parser;
use winnow::combinator::trace;
use winnow::stream::Location;

use crate::util::{
    AllocationParsingParams,
    BitErr,
    BitInput,
    BitPosition,
    FragmentId,
    take_ls_bit,
};

/// An entry in the [`AllocationMap`] representing some sort of allocation
/// on the disk.
#[derive(Debug, Clone, Copy)]
pub struct FragmentBlock {
    /// The id number of this Fragment. Multiple Fragments can exist with the
    /// same id number, either across a single zone or multiple (in format F)
    /// and a given [`super::disc_object::DiscObject`] corresponds to the
    /// concatenation of all of them.
    ///
    /// The `id` is the only data the disc explicitly records, the other
    /// fields are stored to simplify further logic and/or debugging.
    pub(crate) id: FragmentId,
    /// Whether this block represents a region of free space
    pub(crate) free_space: bool,
    /// The length, in bits, this block takes up inside the `AllocationMap`,
    /// including the terminating `1`
    pub(crate) map_length: usize,
    /// the position within the allocation map this block starts at. This
    /// corresponds to the starting point of the zone allocation this fragment
    /// represents.
    pub(crate) position: BitPosition,
}

impl FragmentBlock {
    /// Parses a single FragmentBlock. Fails only if the stream is exhausted
    /// early, which shouldn't happen with a well-formed image.
    pub(crate) fn parse<'a>(
        input: &mut BitInput<'a>,
        params: &AllocationParsingParams,
    ) -> Result<Self, BitErr<'a>> {
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

            Ok(FragmentBlock {
                id,
                free_space: false,
                map_length,
                position,
            })
        })
        .parse_next(input)
    }
}
