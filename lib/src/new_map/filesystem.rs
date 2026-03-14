use arrayvec::ArrayVec;
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
use winnow::stream::Location;

use crate::new_map::LoadErrors;
use crate::new_map::STRICT_MODE;
use crate::new_map::util::BitPosition;
use crate::new_map::util::{DiscPosition, FixedLenString, InputStream, ParseResult};

#[derive(Clone, Copy)]
struct MagicString([u8; 4]);
impl MagicString {
    fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, Self> {
        alt((b"Hugo", b"Nick"))
            .context(LoadErrors::MagicStringFailure(
                *input.first_chunk().unwrap(),
            ))
            .parse_next(input)
            .map(|data| MagicString(*data.first_chunk().unwrap()))
    }
}
impl std::fmt::Debug for MagicString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MagicString({})", str::from_utf8(&self.0).unwrap())
    }
}

const SIZE_OF_DIRECTORY: usize = 77;
#[derive(Debug, Clone)]
pub(crate) struct Directory {
    pub(crate) header: DirHeader,
    pub(crate) entries: ArrayVec<DirEntry, SIZE_OF_DIRECTORY>,
    pub(crate) tail: DirTail,
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
pub(crate) struct DirHeader {
    start_seq_num: u8,
    start_name: MagicString,
}

#[derive(Debug, Clone)]
pub(crate) struct DirEntry {
    pub(crate) obj_name: FixedLenString,
    pub(crate) load: u32,
    pub(crate) exec: u32,
    pub(crate) len: u32,
    pub(crate) address: DiscPosition,
    pub(crate) attrs: Attributes,
}
impl DirEntry {
    fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, Self> {
        let obj_name = trace("obj_name", FixedLenString::parse).parse_next(input)?;
        let load = trace("load", le_u32).parse_next(input)?;
        let exec = trace("exec", le_u32).parse_next(input)?;
        let len = trace("len", le_u32).parse_next(input)?;
        let address = trace("address", DiscPosition::parse_for_new_map).parse_next(input)?;
        let attrs = Attributes::parse(input, obj_name)?;

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
pub(crate) struct DirTail {
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
    fn parse<'a>(input: &mut InputStream<'a>, obj_name: FixedLenString) -> ParseResult<'a, Self> {
        if STRICT_MODE {
            let pos = input.current_token_start();
            trace("Attributes", le_u8)
                .try_map(|a| {
                    Attributes::from_bits(a).ok_or(LoadErrors::InvalidAttr {
                        location: BitPosition(pos),
                        filename: obj_name.to_string(),
                        attr_value: a,
                    })
                })
                .parse_next(input)
        } else {
            trace("Attributes", le_u8::<_, ErrMode<EmptyError>>)
                .parse_next(input)
                .map(Attributes::from_bits_truncate)
                .map_err(|e| e.map(|_| unreachable!()))
        }
    }
}
