// Structures representing repeating features within the filesystem, including
// directory records, file attribute flags, etc.

use arrayvec::ArrayVec;
use winnow::Parser;
use winnow::binary::{le_u8, le_u16, le_u32};
use winnow::combinator::{alt, repeat, seq, trace};
use winnow::stream::Location;

use super::sys_structures::Path;
use super::util::{
    BitPosition,
    DiscPosition,
    FaultableResult,
    FixedLenString,
    InputStream,
    ParseResult,
};
use super::{Fault, FaultValue, STRICT_MODE};

pub(crate) const MAX_SEGMENT_LENGTH: usize = 10;
pub(crate) const MAX_TITLE_LENGTH: usize = 19;

#[derive(Clone, Copy)]
struct MagicString([u8; 4]);
impl MagicString {
    fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, Self> {
        alt((b"Hugo", b"Nick"))
            .context(Fault::MagicStringFailure(*input.first_chunk().unwrap()))
            .parse_next(input)
            .map(|data| MagicString(*data.first_chunk().unwrap()))
    }
}
impl std::fmt::Debug for MagicString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A MagicString being successfully constructed means its valid UTF8
        write!(f, "MagicString({})", str::from_utf8(&self.0).unwrap())
    }
}

const SIZE_OF_DIRECTORY: usize = 77;
#[derive(Debug, Clone)]
pub struct Directory {
    pub(crate) header: DirHeader,
    pub(crate) entries: ArrayVec<DirEntry, SIZE_OF_DIRECTORY>,
    pub(crate) tail: DirTail,
}
impl Directory {
    pub(crate) fn parse<'a>(input: &mut InputStream<'a>) -> FaultableResult<'a, Self> {
        trace("Directory", |input: &mut InputStream<'a>| {
            let header = seq! {
               DirHeader {
                   start_seq_num: le_u8,
                   start_name: MagicString::parse
                }
            }
            .parse_next(input)?;

            let mut results: Vec<_> =
                repeat(SIZE_OF_DIRECTORY, trace("DirEntry", DirEntry::parse)).parse_next(input)?;

            if let Some(first_null) = results
                .iter()
                .position(|FaultValue(entry, _)| entry.obj_name.is_empty())
            {
                results.truncate(first_null);
            }

            let (entries, faults): (ArrayVec<_, _>, Vec<_>) =
                results.into_iter().map(|FaultValue(e, f)| (e, f)).unzip();
            let mut faults: Vec<_> = faults.into_iter().flatten().collect();

            let tail = seq! {
                DirTail {
                    last_mark: le_u8,
                    reserved: le_u16,
                    parent: DiscPosition::parse_for_new_map,
                    title: FixedLenString::<MAX_TITLE_LENGTH>::parse_from_disk,
                    name: FixedLenString::parse_from_disk,
                    end_seq_num: le_u8,
                    end_name: MagicString::parse,
                    check_byte: le_u8,
                }
            }
            .parse_next(input)?;

            if header.start_seq_num != tail.end_seq_num {
                faults.push(Fault::SequenceNumberMismatch {
                    path: Path::default(),
                    start_seq_num: header.start_seq_num,
                    end_seq_num: tail.end_seq_num,
                });
            }

            Ok(FaultValue(
                Directory {
                    header,
                    entries,
                    tail,
                },
                faults,
            ))
        })
        .parse_next(input)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DirHeader {
    start_seq_num: u8,
    start_name: MagicString,
}

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub obj_name: FixedLenString<MAX_SEGMENT_LENGTH>,
    pub load: u32,
    pub exec: u32,
    pub len: u32,
    pub address: DiscPosition,
    pub attrs: Attributes,
}
impl DirEntry {
    fn parse<'a>(input: &mut InputStream<'a>) -> FaultableResult<'a, Self> {
        let obj_name = trace("obj_name", FixedLenString::parse_from_disk).parse_next(input)?;
        let load = trace("load", le_u32).parse_next(input)?;
        let exec = trace("exec", le_u32).parse_next(input)?;
        let len = trace("len", le_u32).parse_next(input)?;
        let address = trace("address", DiscPosition::parse_for_new_map).parse_next(input)?;
        let FaultValue(attrs, mut fault) = Attributes::parse(input)?;
        fault.iter_mut().for_each(|f| {
            if let Fault::InvalidAttr { path, .. } = f {
                dbg!(obj_name);
                *path = Path::from_segments(vec![obj_name]);
            }
        });

        Ok(FaultValue(
            DirEntry {
                obj_name,
                load,
                exec,
                len,
                address,
                attrs,
            },
            fault,
        ))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DirTail {
    last_mark: u8,
    reserved: u16,
    parent: DiscPosition,
    title: FixedLenString<MAX_TITLE_LENGTH>,
    name: FixedLenString<MAX_SEGMENT_LENGTH>,
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
    fn parse<'a>(input: &mut InputStream<'a>) -> FaultableResult<'a, Self> {
        if STRICT_MODE {
            let pos = input.current_token_start();
            trace("Attributes", le_u8)
                .map(|attr_value| match Attributes::from_bits(attr_value) {
                    Some(a) => a.into(),
                    None => FaultValue(
                        Attributes::from_bits_retain(attr_value),
                        vec![Fault::InvalidAttr {
                            location: BitPosition::from_bytes(pos),
                            path: Path::default(),
                            attr_value,
                        }],
                    ),
                })
                .parse_next(input)
        } else {
            trace("Attributes", le_u8)
                .parse_next(input)
                .map(Attributes::from_bits_truncate)
                .map(Into::into)
        }
    }
}
