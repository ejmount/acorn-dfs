use arrayvec::ArrayVec;
use winnow::ModalResult;
use winnow::Parser;
use winnow::binary::le_u8;
use winnow::binary::le_u16;
use winnow::binary::le_u32;
use winnow::combinator::alt;
use winnow::combinator::repeat;
use winnow::combinator::seq;
use winnow::combinator::trace;
use winnow::error::EmptyError;
use winnow::error::ErrMode;
use winnow::error::FromExternalError;
use winnow::error::TreeError;
use winnow::stream::Location;

use crate::new_map::LoadErrors;
use crate::new_map::STRICT_MODE;
use crate::new_map::util::{DiscPosition, FixedLenString, InputStream, ParseResult};

#[derive(Debug, Clone, Copy)]
struct MagicString([u8; 4]);
impl MagicString {
    fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, Self> {
        alt((b"Hugo", b"Nick"))
            .parse_next(input)
            .map(|data| MagicString(*data.first_chunk().unwrap()))
    }
}

const SIZE_OF_DIRECTORY: usize = 77;
#[derive(Debug, Clone)]
pub(crate) struct Directory {
    header: DirHeader,
    entries: ArrayVec<DirEntry, SIZE_OF_DIRECTORY>,
    tail: DirTail,
}
impl Directory {
    pub(crate) fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, Self> {
        let header = seq! {
           DirHeader {
               start_seq_num: le_u8,
               start_name: MagicString::parse
            }
        }
        .parse_next(input)?;

        let mut entries: ArrayVec<_, _> = ArrayVec::new();
        repeat(SIZE_OF_DIRECTORY, trace("DirEntry", DirEntry::parse))
            .fold(|| {}, |_, e| entries.push(e))
            .parse_next(input)
            .unwrap();

        let first_null_idx = entries.iter().position(|e| e.obj_name.is_empty());
        if let Some(first_null) = first_null_idx {
            entries.truncate(first_null);
        }

        let tail = seq! {
            DirTail {
                last_mark: le_u8,
                reserved: le_u16,
                parent: DiscPosition::parse_for_new_map,
                title: FixedLenString::<19>::parse,
                name: FixedLenString::parse,
                end_seq_num: le_u8,
                end_name: MagicString::parse,
                check_byte: le_u8,
            }
        }
        .parse_next(input)?;
        Ok(Directory {
            header,
            entries,
            tail,
        })
    }
}

#[derive(Debug, Clone)]
struct DirHeader {
    start_seq_num: u8,
    start_name: MagicString,
}

#[derive(Debug, Clone)]
struct DirEntry {
    obj_name: FixedLenString,
    load: u32,
    exec: u32,
    len: u32,
    address: DiscPosition,
    attrs: Attributes,
}
impl DirEntry {
    fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, Self> {
        let obj_name = FixedLenString::parse(input)?;
        let load = le_u32(input)?;
        let exec = le_u32(input)?;
        let len = le_u32(input)?;
        let address = DiscPosition::parse_for_new_map(input)?;
        let attrs = Attributes::parse(input).map_err(|e| {
            e.map(|mut f: LoadErrors| {
                let LoadErrors::InvalidAttr { location, filename } = &mut f else {
                    unreachable!()
                };
                *filename = obj_name.to_string();
                TreeError::from_external_error(input, f)
            })
        })?;

        Ok(DirEntry {
            obj_name,
            load,
            exec,
            len,
            address,
            attrs,
        })
    }
}

#[derive(Debug, Clone)]
struct DirTail {
    last_mark: u8,
    reserved: u16,
    parent: DiscPosition,
    title: FixedLenString<19>,
    name: FixedLenString,
    end_seq_num: u8,
    end_name: MagicString,
    check_byte: u8,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct Attributes: u8 {
        const READ = 1 << 0;
        const WRITE = 1 << 1;
        const LOCK = 1 << 2;
        const DIR = 1 << 3;
        const PUBLIC_READ = 1 << 4;
        const PUBLIC_WRITE = 1 << 5;
    }
}
impl Attributes {
    fn parse<'a>(input: &mut InputStream<'a>) -> ModalResult<Self, LoadErrors> {
        if STRICT_MODE {
            match le_u8::<_, ErrMode<EmptyError>>(input) {
                Ok(a) => match Attributes::from_bits(a) {
                    Some(a) => Ok(a),
                    None => Err(ErrMode::Backtrack(LoadErrors::InvalidAttr {
                        location: super::util::BitPosition(input.current_token_start()),
                        filename: String::new(),
                    })),
                },
                Err(e) => Err(e.map(|_| unreachable!())),
            }
        } else {
            le_u8::<_, ErrMode<EmptyError>>(input)
                .map(Attributes::from_bits_truncate)
                .map_err(|e| e.map(|_| unreachable!()))
        }
    }
}
