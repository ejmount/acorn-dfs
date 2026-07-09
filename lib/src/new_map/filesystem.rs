// Structures representing repeating features within the filesystem, including
// directory records, file attribute flags, etc.

use arrayvec::ArrayVec;
use winnow::Parser;
use winnow::binary::{le_u8, le_u16, le_u32};
use winnow::combinator::{repeat, seq, trace};
use winnow::stream::Location;
use winnow::token::take;

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

fn parse_magic_string<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, ()> {
    take(4usize)
        .try_map(|b: &[u8]| {
            if b == b"Hugo" || b == b"Nick" {
                Ok(())
            } else {
                Err(Fault::MagicStringFailure(*b.first_chunk().unwrap()))
            }
        })
        .parse_next(input)
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
            let (header, start_data) = DirHeader::parse.with_taken().parse_next(input)?;

            let results: Vec<_> = repeat(
                SIZE_OF_DIRECTORY,
                trace("DirEntry", DirEntry::parse.with_taken()),
            )
            .parse_next(input)?;

            let (tail, tail_data) = DirTail::parse.with_taken().parse_next(input)?;

            let mut entries = ArrayVec::new();
            let mut faults = vec![];
            let mut entry_data = vec![];

            for (FaultValue(e, f), span) in results {
                if e.obj_name.is_empty() {
                    break;
                }
                entries.push(e);
                faults.extend(f);
                entry_data.push(span);
            }

            let check_byte = Self::compute_checksum(start_data, &entry_data, tail_data);

            if header.start_seq_num != tail.end_seq_num {
                faults.push(Fault::SequenceNumberMismatch {
                    path: Path::default(),
                    start_seq_num: header.start_seq_num,
                    end_seq_num: tail.end_seq_num,
                });
            }

            if check_byte != tail.check_byte {
                faults.push(Fault::CheckByteFailure {
                    path: Path::from_segments(&[tail.name]),
                    expected: tail.check_byte,
                    actual: check_byte,
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

    fn compute_checksum(start_data: &[u8], entries: &[&[u8]], tail_data: &[u8]) -> u8 {
        fn accumulate_word(a: u32, &word: &[u8; 4]) -> u32 {
            a.rotate_right(13) ^ u32::from_le_bytes(word)
        }
        fn accumulate_byte(a: u32, &byte: &u8) -> u32 {
            a.rotate_right(13) ^ (byte as u32)
        }

        let mut data = Vec::from_iter(start_data.iter().copied());
        for e in entries {
            data.extend(*e);
        }

        let (starting_words, trail) = data.as_chunks();

        let accumulation = starting_words.iter().fold(0, accumulate_word);
        let accumulation = trail.iter().fold(accumulation, accumulate_byte);

        // "The last whole words in the directory are accumulated, except the very last
        // WORD which is excluded as it contains the check byte."
        //
        // However, the PRM seems to miss that the leading zero byte is *not* included
        // in the checksum calculation specified here:
        // https://gitlab.riscosopen.org/RiscOS/Sources/FileSys/FileCore/-/blob/master/s/FileCore25#L1287
        let tail = &tail_data[1..tail_data.len() - 4];

        let (leading_bytes, tail_words) = tail.as_rchunks();
        let accumulation = leading_bytes.iter().fold(accumulation, accumulate_byte);

        let accumulation = tail_words.iter().fold(accumulation, accumulate_word);

        let [a, b, c, d] = accumulation.to_le_bytes();
        a ^ b ^ c ^ d
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DirHeader {
    start_seq_num: u8,
}
impl DirHeader {
    fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, DirHeader> {
        trace(
            "DirHeader",
            seq! {
               DirHeader {
                   start_seq_num: le_u8,
                   _: parse_magic_string,
                }
            },
        )
        .parse_next(input)
    }
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
                *path = Path::from_segments(&[obj_name]);
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
    parent: DiscPosition,
    title: FixedLenString<MAX_TITLE_LENGTH>,
    name: FixedLenString<MAX_SEGMENT_LENGTH>,
    end_seq_num: u8,
    check_byte: u8,
}
impl DirTail {
    fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, DirTail> {
        trace(
            "DirTail",
            seq! {
                DirTail {
                    _: b'\0', // NewDirLastMark - must be 0
                    _: b"\0\0", // Reserved - must be 0
                    parent: DiscPosition::parse_for_new_map,
                    title: FixedLenString::<MAX_TITLE_LENGTH>::parse_from_disk,
                    name: FixedLenString::parse_from_disk,
                    end_seq_num: le_u8,
                    _: parse_magic_string,
                    check_byte: le_u8,
                }
            },
        )
        .parse_next(input)
    }
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
