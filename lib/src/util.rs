//! Types and functions that do not directly correspond to system entities, on
//! disk or otherwise, but are still used frequently
use std::fmt::Debug;
use std::hash::Hash;
use std::num::NonZero;
use std::ops::Add;

use serde::Serialize;
use winnow::binary::bits::Bits;
use winnow::binary::le_u24;
use winnow::error::{ErrMode, TreeError};
use winnow::stream::Stream;
use winnow::{BStr, LocatingSlice, ModalResult, Parser};

use super::error::{Fault, FaultValue};
use super::new_map::disc_record::DiscRecord;

/// The input used for parsing most structures on disk with winnow combinators.
///
/// `LocatingSlice` is important in this context as tracking the location within
/// the stream of various structures is important for, e.g. offsets in
/// [`AllocationMap`][`crate::new_map::disc_structures::AllocationMap`]
pub(crate) type InputStream<'a> = LocatingSlice<&'a BStr>;
pub(crate) type ParseError<'a> = TreeError<InputStream<'a>, Fault>;

/// Parsing most structures produces either a valid instance of the structure or
/// a [`ParseError`] which represents unexpected input, but [`ModalResult`] also
/// covers the possibility that we run off the end of the stream
pub(crate) type ParseResult<'a, Type, E = ParseError<'a>> = ModalResult<Type, E>;

/// Parsing certain structures can succeed but raise non-fatal issues, encoded
/// in the [`FaultValue`] pair
pub(crate) type FaultableResult<'a, Type, E = ParseError<'a>> =
    ParseResult<'a, FaultValue<Type>, E>;

/// Input for parsing the
/// [`AllocationMap`][`super::disc_structures::AllocationMap`] which is the only
/// bit-level stream
pub(crate) type BitInput<'a> = Bits<InputStream<'a>>;
pub(crate) type BitErr<'a> = TreeError<BitInput<'a>, Fault>;

/// See [`crate::new_map::disc_structures::FragmentBlock`]. This value has
/// dynamic length but "...the fragment id cannot be more than 15 bits long." http://www.riscos.com/support/developers/prm/filecore.html#32170
pub(crate) type FragmentId = u16;

/// Creates an InputStream for use in most other functions out of a byte-slice
pub(crate) fn make_input<'a>(input: &'a [u8]) -> InputStream<'a> {
    LocatingSlice::new(BStr::new(input))
}

/// Parses and returns a byte stream in least-significant first order
///
/// This is the opposite order as [winnow::binary::bits::bool] uses.
pub(crate) fn take_ls_bit<'a>(input: &mut BitInput<'a>) -> ModalResult<bool, BitErr<'a>> {
    let Bits(stream, offset) = input;
    let byte = stream
        .peek_token()
        .ok_or(ErrMode::Incomplete(winnow::error::Needed::Size(
            NonZero::new(1).unwrap(),
        )))?;

    let shaved_byte = byte >> *offset;
    let o = (shaved_byte % 2) > 0;
    *offset += 1;
    if *offset == 8 {
        let _ = stream.next_token();
        *offset -= 8;
    }
    Ok(o)
}

/// Parsing the
/// [`AllocationMap`][`crate::new_map::disc_structures::AllocationMap`] requires
/// several values that are spread across different structures, so this class
/// consolidates them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AllocationParsingParams {
    pub(crate) fragment_id_length: usize,
    pub(crate) sector_size: usize,
    pub(crate) free_link: u16,
    pub(crate) trailing_bytes: usize,
}

impl AllocationParsingParams {
    pub fn new(free_link: u16, disk: &DiscRecord) -> AllocationParsingParams {
        AllocationParsingParams {
            fragment_id_length: disk.idlen as _,
            sector_size: disk.sector_size_in_bytes(),
            free_link,
            trailing_bytes: disk.zone_trailing_bytes(),
        }
    }

    pub fn sector_size(&self) -> usize {
        self.sector_size
    }
    pub fn free_link(&self) -> FragmentId {
        self.free_link
    }
    pub fn fragment_id_length(&self) -> usize {
        self.fragment_id_length
    }
}

/// A bit-level offset, used for tracking the position of
/// [`crate::new_map::disc_structures::FragmentBlock`]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BitPosition(usize);
impl BitPosition {
    pub(crate) fn from_bits(bits: usize) -> BitPosition {
        BitPosition(bits)
    }
    pub(crate) fn from_bytes(bytes: usize) -> BitPosition {
        BitPosition(8 * bytes)
    }
    pub(crate) fn bits(&self) -> usize {
        self.0
    }
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

/// Records such as file entries in "new map" disc formats refer to positions as
/// an "indirect disc address," containing both a fragment number (disc object
/// ID) and a sector offset within that object. These are used because multiple
/// filesystem objects (e.g. directory entries, file contents) can share a disc
/// object as encoded by the allocation map
///
/// Specifically, the bits are laid out as:
/// ddd00000 0fffffff ffffffff ssssssss
///
/// Where:
/// - the `f` bits are the fragment ID (that is, disc object ID) referred to
/// - the `s` bits is an offset, in whole number of sectors
///
///
/// https://www.riscos.com/support/developers/prm/filecore.html#73575
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct DiscPosition(pub(crate) u32);
impl DiscPosition {
    pub(crate) const LENGTH: usize = 3;

    pub(crate) fn parse_for_new_map<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, Self> {
        le_u24.parse_next(input).map(DiscPosition)
    }
    pub(crate) fn fragment(&self) -> FragmentId {
        ((self.0 & 0x7F_FF_00) >> 8) as _
    }
    pub(crate) fn sector_idx(&self) -> u8 {
        // Sector IDs inside indirect addresses count from 1, except that a value of 0
        // still refers to the beginning of the object area.
        match self.0 & 0xFF {
            0 => 0,
            n => n as u8 - 1,
        }
    }
}

impl Debug for DiscPosition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscPosition")
            .field("val", &self.0)
            .field("fragment", &self.fragment())
            .field("sector no", &self.sector_idx())
            .finish()
    }
}

#[cfg(test)]
mod test {
    use std::cmp::Ordering;
    use std::fmt::Write;

    use winnow::binary::bits::Bits;

    use super::{DiscPosition, make_input, take_ls_bit};

    #[test]
    fn test_ls_bit() {
        let mut lsb = Bits(make_input(&[1]), 0);
        let mut msb = Bits(make_input(&[0x80, 0x01]), 0);

        let lsb = &mut lsb;
        let msb = &mut msb;

        assert!(take_ls_bit(lsb).unwrap());
        assert!(!take_ls_bit(msb).unwrap());

        for _ in 0..6 {
            take_ls_bit(msb).unwrap();
        }

        assert!(take_ls_bit(msb).unwrap());
        assert!(take_ls_bit(msb).unwrap());
    }

    #[test]
    fn repeat_test() {
        let mut msb = Bits(make_input(&[0xAA, 0xAA]), 0);
        let c = &mut msb;
        let mut outs = vec![];
        for _ in 0..16 {
            outs.push(take_ls_bit(c).unwrap());
        }
        let (t, f): (Vec<_>, _) = outs.into_iter().partition(|m| *m);
        assert_eq!(t.len(), f.len());
    }

    #[test]
    // Testing the bit manipulations to accurately represent the different implied
    // parts of the structure
    fn disc_position() {
        let dp = DiscPosition(515);
        let mut s = String::new();
        let _ = write!(s, "{dp:?}").unwrap();
        assert_eq!(s, "DiscPosition { val: 515, fragment: 2, sector no: 2 }");
    }
}
