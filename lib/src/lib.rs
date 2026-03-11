#![allow(unused_variables)]
#![allow(dead_code)]
pub mod new_map;
pub mod old_map;

use std::num::NonZero;
use std::ops::Range;

use thiserror::Error;
use winnow::Bytes;
use winnow::LocatingSlice;
use winnow::ModalResult;
use winnow::Parser;
use winnow::error::ErrMode;
use winnow::error::TreeError;
use winnow::stream::Location;
use winnow::stream::Stream;

type InputStream<'a> = LocatingSlice<&'a Bytes>;
type BitInput<'a> = (InputStream<'a>, usize);
type BitErr<'a> = ErrMode<TreeError<BitInput<'a>, LoadErrors>>;
type ParseError<'a> = TreeError<InputStream<'a>, crate::LoadErrors>;
type ParseResult<'a, Type> = ModalResult<Type, ParseError<'a>>;

#[derive(Error, Debug)]
pub enum LoadErrors {}

trait LenParser<I, O, E>: Parser<I, O, E> + Sized
where
    I: Stream + Location,
{
    fn with_len(self) -> impl Parser<I, (O, usize), E>;
}

impl<I, O, E, P> LenParser<I, O, E> for P
where
    P: Parser<I, O, E>,
    I: Stream + Location,
{
    fn with_len(self) -> impl Parser<I, (O, usize), E> {
        let p = self.with_span();
        p.map(|(o, Range { start, end })| (o, end - start))
    }
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
    if *offset >= 8 {
        let _ = stream.next_token();
        *offset -= 8;
    }
    Ok(o)
}

#[cfg(test)]
mod test {
    use winnow::{Bytes, LocatingSlice};

    use crate::take_ls_bit;

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
}
