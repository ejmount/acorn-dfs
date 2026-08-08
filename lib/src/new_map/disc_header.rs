use serde::Serialize;
use winnow::Parser;
use winnow::binary::{le_u8, le_u16};
use winnow::combinator::{seq, trace};

use super::super::util::InputStream;
use crate::util::ParseResult;

/// A prefix of a [`MapBlock`], containing various checks and the pointer to
/// beginning of the free list
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Header {
    /// A checksum of the parent MapBlock
    pub(crate) zone_check: u8,
    /// Pointer to the first free fragment in this zone, relative to itself,
    /// i.e. from the first byte of the same zone's map sector.
    ///
    /// The value that is stored on disk is described as:
    /// offset in bits to first free space in zone, or 0 if none, with top
    /// bit always set
    /// https://www.chiark.greenend.org.uk/~theom/riscos/docs/ultimate/a252efmt.txt
    ///
    /// When actually parsed, we automatically lower the top bit
    pub(crate) free_link: u16,
    /// Coherency check between all zones
    pub(crate) cross_check: u8,
}

impl Header {
    pub(crate) const LENGTH: usize = 4;
    pub(crate) fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, Self> {
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
