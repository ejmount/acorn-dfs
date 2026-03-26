/// Types and functions that do not directly correspond to system entities, on
/// disk or otherwise, but are still used frequently
use std::cmp::Ordering;
use std::fmt::{Debug, Display};
use std::hash::{Hash, Hasher};
use std::num::NonZero;
use std::ops::Add;

use winnow::binary::le_u24;
use winnow::combinator::trace;
use winnow::error::{ErrMode, TreeError};
use winnow::stream::Stream;
use winnow::token::take;
use winnow::{BStr, LocatingSlice, ModalResult, Parser};

use super::disc_structures::DiscRecord;
use super::{Fault, FaultValue};

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
pub(crate) type ParseResult<'a, Type> = ModalResult<Type, ParseError<'a>>;

/// Parsing certain structures can succeed but raise non-fatal issues, encoded
/// in the [`FaultValue`] pair
pub(crate) type FaultableResult<'a, Type> = ParseResult<'a, FaultValue<Type>>;

/// Input for parsing the
/// [`AllocationMap`][`super::disc_structures::AllocationMap`] which is the only
/// bit-level stream
pub(crate) type BitInput<'a> = (InputStream<'a>, usize);
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
pub fn take_ls_bit<'a>(input: &mut BitInput<'a>) -> ModalResult<bool, BitErr<'a>> {
    let (stream, offset) = input;
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
    pub(crate) mapped_space_in_alloc_units: usize,
    pub(crate) fragment_id_length: usize,
    pub(crate) log_bytes_per_alloc: usize,
    pub(crate) sector_size: usize,
    pub(crate) free_link: u16,
}

impl AllocationParsingParams {
    pub fn new(
        zone_includes_map: bool,
        free_link: u16,
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

        AllocationParsingParams {
            mapped_space_in_alloc_units,
            fragment_id_length: disk.idlen as _,
            log_bytes_per_alloc: disk.log2_bytes_per_mapbit as _,
            sector_size: disk.sector_size(),
            free_link,
        }
    }
    pub fn sector_size(&self) -> usize {
        self.sector_size
    }
    pub fn bytes_per_alloc_unit(&self) -> usize {
        2usize.pow(self.log_bytes_per_alloc as _)
    }
    pub fn mapped_space_in_alloc_units(&self) -> usize {
        self.mapped_space_in_alloc_units
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
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiscPosition(pub(crate) u32);
impl DiscPosition {
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

/// A string of bytes of fixed length, used for various types of data such as
/// file/directory names.
///
/// While this data has a fixed length when stored within disk structures,
/// semantically it is of variable length, with *any* control character
/// representing a terminator.
#[derive(Clone, Copy, Eq)]
pub struct FixedLenString<const LEN: usize = 10>([u8; LEN]);
impl<const LEN: usize> FixedLenString<LEN> {
    // https://www.riscos.com/support/users/firststeps/chap08.htm#L0058
    const FORBIDDEN_CHARS: &[u8] = b"$%&*#@\"|^.:";

    /// For easily constructing an FLS for testing. Similar to
    /// `parse_from_byte_str` but ignores control character considerations.
    #[cfg(test)]
    pub fn from_bytes_dynamic(input: &[u8]) -> FixedLenString<LEN> {
        let len = input.len().min(LEN);
        let mut output = [0; LEN];
        output[..len].copy_from_slice(&input[..len]);
        FixedLenString(output)
    }

    /// Read this value from text, such as a user-provided
    /// [`super::sys_structures::Path`].
    ///
    /// This *does* stop early should a terminator be encountered, unlike
    /// `parse_from_disk` and will reject empty components. This is
    /// load-bearing for rejecting invalid [`super::sys_structures::Path`]
    /// values correctly.
    pub fn parse_from_byte_str<'a>(
        input: &mut InputStream<'a>,
    ) -> ModalResult<FixedLenString<LEN>, TreeError<InputStream<'a>, Fault>> {
        let next_end_char = input
            .offset_for(|c| c.is_ascii_control() || Self::FORBIDDEN_CHARS.contains(&c))
            .unwrap_or(input.len());

        let length = [LEN, next_end_char, input.len()].into_iter().min().unwrap();

        let mut output = [0; LEN];
        let data = trace(
            format!("FixedLenString (dynamic, up to {length}"),
            take(length).verify(|b: &[u8]| !b.is_empty()),
        )
        .parse_next(input)?;
        output[..length].copy_from_slice(data);

        Ok(FixedLenString(output))
    }
    /// Read a value from a disk image, which reads (and advances the stream by)
    /// exactly `LEN` bytes,
    pub fn parse_from_disk<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, Self> {
        // Unlike other methods, this will accept an empty string (containing an initial
        // control character)
        trace(
            format!("FixedString {LEN}"),
            |input: &mut InputStream<'a>| {
                let o = *take(LEN).parse_next(input)?.first_chunk().unwrap();
                Ok(FixedLenString(o))
            },
        )
        .map(Into::into)
        .parse_next(input)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn len(&self) -> usize {
        self.0
            .iter()
            .position(|&u| (u as char).is_control())
            .unwrap_or(self.0.len())
    }

    /// The segment of this string that represents valid usable data
    pub fn valid_range(&self) -> &[u8] {
        let idx = self.len();
        &self.0[..idx]
    }
}
impl<const N: usize> Debug for FixedLenString<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FixedLenString({:?})", String::from_utf8_lossy(&self.0))
    }
}
impl Display for FixedLenString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let lossy = String::from_utf8_lossy(self.valid_range());
        write!(f, "{}", str::escape_default(&lossy))
    }
}
impl<const LEN: usize> Hash for FixedLenString<LEN> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write(self.valid_range());
    }
}

impl<const LEN: usize> PartialEq for FixedLenString<LEN> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl<const LEN: usize> PartialOrd for FixedLenString<LEN> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<const LEN: usize> Ord for FixedLenString<LEN> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let self_valid = self.valid_range();
        let other_valid = other.valid_range();

        self_valid.cmp(other_valid)
    }
}

#[cfg(test)]
mod test {
    use std::cmp::Ordering;
    use std::fmt::Write;

    use super::{DiscPosition, FixedLenString, make_input, take_ls_bit};

    #[test]
    fn test_ls_bit() {
        let mut lsb = (make_input(&[1]), 0);
        let mut msb = (make_input(&[0x80, 0x01]), 0);

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
        let mut msb = (make_input(&[0xAA, 0xAA]), 0);
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

    #[test]
    fn fixed_string_properties() {
        let empty = FixedLenString([0; 10]);
        assert_eq!(empty.valid_range(), &[]);

        let a = FixedLenString([b'a', b'b', 0]);
        let b = FixedLenString([b'a', b'b', b'c']);
        assert_eq!(a.cmp(&b), Ordering::Less);
        assert_eq!(a.len(), 2);
        assert_eq!(b.len(), 3);

        let c = FixedLenString([0]);
        let d = FixedLenString([1]);
        assert_eq!(c, d);
    }
}
