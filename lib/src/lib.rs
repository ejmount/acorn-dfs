#![allow(unused_variables)]
#![allow(dead_code)]
pub mod new_map;
pub mod old_map;

use std::num::NonZero;

use thiserror::Error;
use winnow::Bytes;
use winnow::LocatingSlice;
use winnow::ModalResult;
use winnow::error::ErrMode;
use winnow::error::TreeError;
use winnow::stream::Stream;

use crate::new_map::BitPosition;

type InputStream<'a> = LocatingSlice<&'a Bytes>;
type BitInput<'a> = (InputStream<'a>, usize);
type BitErr<'a> = ErrMode<TreeError<BitInput<'a>, LoadErrors>>;
type ParseError<'a> = TreeError<InputStream<'a>, LoadErrors>;
type ParseResult<'a, Type> = ModalResult<Type, ParseError<'a>>;

#[derive(Error, Debug)]
pub enum LoadErrors {
    #[error("Free link value {0} did not point at valid fragment")]
    InvalidFreeLink(u16),
    #[error(
        "Free fragment block at offset {origin:?} bits points to offset {dest_bit_offset:?} bits which does not contain a fragment"
    )]
    BrokenFreeChain {
        origin: BitPosition,
        dest_bit_offset: BitPosition,
    },
}

fn take_ls_bit<'a>(
    input: &mut BitInput<'a>,
) -> ModalResult<bool, TreeError<BitInput<'a>, LoadErrors>> {
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

#[cfg(test)]
mod test {
    use crate::take_ls_bit;
    use winnow::{Bytes, LocatingSlice};

    #[test]
    fn test_ls_bit() {
        let mut lsb = (LocatingSlice::new(Bytes::new(&[1])), 0);
        let mut msb = (LocatingSlice::new(Bytes::new(&[0x80, 0x01])), 0);

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
        let mut msb = (LocatingSlice::new(Bytes::new(&[0xAA, 0xAA])), 0);
        let c = &mut msb;
        let mut outs = vec![];
        for _ in 0..16 {
            outs.push(take_ls_bit(c).unwrap());
        }
        let (t, f): (Vec<_>, _) = outs.into_iter().partition(|m| *m);
        assert_eq!(t.len(), f.len());
    }
}
