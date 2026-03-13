use std::{fmt::Debug, ops::Add};

use std::num::NonZero;

use winnow::ModalResult;
use winnow::error::ErrMode;
use winnow::error::TreeError;
use winnow::stream::Stream;
use winnow::{BStr, LocatingSlice, Parser, combinator::trace, token::take};

use crate::new_map::LoadErrors;
use crate::new_map::disc_structures::DiscRecord;

pub(crate) type InputStream<'a> = LocatingSlice<&'a BStr>;
pub(crate) type ParseError<'a> = TreeError<InputStream<'a>, LoadErrors>;
pub(crate) type ParseResult<'a, Type> = ModalResult<Type, ParseError<'a>>;
pub(crate) type BitInput<'a> = (InputStream<'a>, usize);
pub(crate) type BitErr<'a> = TreeError<BitInput<'a>, LoadErrors>;

pub(crate) type FragmentId = u16;

pub(crate) fn make_input<'a>(input: &'a [u8]) -> InputStream<'a> {
    LocatingSlice::new(BStr::new(input))
}

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
}

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

#[derive(Clone, Copy)]
pub struct FixedLenString<const LEN: usize = 10>([u8; LEN]);

impl<const N: usize> Debug for FixedLenString<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FixedLenString({:?})", String::from_utf8_lossy(&self.0))
    }
}

impl<const LEN: usize> FixedLenString<LEN> {
    pub fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, Self> {
        trace(
            format!("FixedString {LEN}"),
            |input: &mut InputStream<'a>| {
                let o = *take(LEN).parse_next(input)?.first_chunk().unwrap();
                Ok(FixedLenString(o))
            },
        )
        .parse_next(input)
    }
impl std::fmt::Display for FixedLenString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let lossy = String::from_utf8_lossy(&self.0);
        write!(f, "{}", str::escape_default(&lossy))
    }
}

#[cfg(test)]
mod test {
    use crate::new_map::util::DiscPosition;
    use crate::new_map::util::make_input;
    use crate::new_map::util::take_ls_bit;
    use std::fmt::Write;
    use crate::new_map::util::make_input;
    use winnow::{Bytes, LocatingSlice};

    #[test]
    fn test_ls_bit() {
        let mut lsb = (make_input(&[1]), 0);
        let mut msb = (make_input(&[0x80, 0x01]), 0);

        let msb = &mut msb;

        assert!(take_ls_bit(&mut lsb).unwrap());
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
}
