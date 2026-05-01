// Structures representing repeating features within the filesystem, including
// directory records, file attribute flags, etc.

use arrayvec::ArrayVec;
use winnow::Parser;
use winnow::binary::{le_u8, le_u16, le_u32};
use winnow::combinator::{alt, repeat, trace};
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
pub(crate) struct MagicString([u8; 4]);
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
    pub(crate) start_seq_num: u8,
    pub(crate) start_name: MagicString,
    pub(crate) entries: ArrayVec<DirEntry, SIZE_OF_DIRECTORY>,
    pub(crate) last_mark: u8,
    pub(crate) reserved: u16,
    pub(crate) parent: DiscPosition,
    pub(crate) title: FixedLenString<MAX_TITLE_LENGTH>,
    pub(crate) name: FixedLenString<MAX_SEGMENT_LENGTH>,
    pub(crate) end_seq_num: u8,
    pub(crate) end_name: MagicString,
    pub(crate) check_byte: u8,
}
impl Directory {
    pub(crate) fn parse<'a>(input: &mut InputStream<'a>) -> FaultableResult<'a, Self> {
        trace("Directory", |input: &mut InputStream<'a>| {
            let ((start_seq_num, start_name), start_text) =
                trace("DirHeader", (le_u8, MagicString::parse))
                    .with_taken()
                    .parse_next(input)?;

            let results: Vec<(_, &[u8])> = repeat(
                SIZE_OF_DIRECTORY,
                trace("DirEntry", DirEntry::parse.with_taken()),
            )
            .parse_next(input)?;

            let mut entries = ArrayVec::new();
            let mut faults = vec![];
            let mut entry_texts = vec![];

            for (FaultValue(e, f), span) in results {
                if e.obj_name.is_empty() {
                    break;
                }
                entries.push(e);
                faults.extend(f);
                entry_texts.push(span);
            }

            let (fields, tail_text) = (
                le_u8,
                le_u16,
                DiscPosition::parse_for_new_map,
                FixedLenString::<MAX_TITLE_LENGTH>::parse_from_disk,
                FixedLenString::parse_from_disk,
                le_u8,
                MagicString::parse,
                le_u8,
            )
                .with_taken()
                .parse_next(input)?;

            let (last_mark, reserved, parent, title, name, end_seq_num, end_name, actual_check) =
                fields;

            let check_byte = Self::compute_checksum(start_text, &entry_texts, tail_text);

            eprintln!("calculated check_byte={check_byte:8b}, actual_check={actual_check:8b}");

            if start_seq_num != end_seq_num {
                faults.push(Fault::SequenceNumberMismatch {
                    path: Path::from_segments(vec![name]),
                    start_seq_num,
                    end_seq_num,
                });
            }

            Ok(FaultValue(
                Directory {
                    start_seq_num,
                    start_name,
                    entries,
                    last_mark,
                    reserved,
                    parent,
                    title,
                    name,
                    end_seq_num,
                    end_name,
                    check_byte,
                },
                faults,
            ))
        })
        .parse_next(input)
    }

    /// Calculates the check_byte value for a given set of directory sections.
    ///
    /// Note that these sections are expected to be discontiguous, because dead
    /// file entries are not used.
    ///
    /// https://www.riscos.com/support/developers/prm/filecore.html#32170
    fn compute_checksum(start_text: &[u8], entries: &[&[u8]], orig_tail: &[u8]) -> u8 {
        fn accumulate_word(a: u32, &word: &[u8; 4]) -> u32 {
            a.rotate_right(13) ^ u32::from_le_bytes(word)
        }
        fn accumulate_byte(a: u32, &byte: &u8) -> u32 {
            a.rotate_right(13) ^ (byte as u32)
        }

        dbg!(entries.len());
        let mut data = Vec::from_iter(start_text.iter().copied());
        for e in entries {
            data.extend(*e);
        }

        let (starting_words, trail) = data.as_chunks();

        let accumulation = starting_words.iter().fold(0, accumulate_word);
        let accumulation = trail.iter().fold(accumulation, accumulate_byte);

        // "The last whole words in the directory are accumulated, except the very last
        // WORD which is excluded as it contains the check byte."
        let tail = &orig_tail[..orig_tail.len() - 4];

        let (leading_bytes, tail_words) = tail.as_rchunks();
        let accumulation = leading_bytes.iter().fold(accumulation, accumulate_byte);

        let accumulation = tail_words.iter().fold(accumulation, accumulate_word);

        let [a, b, c, d] = accumulation.to_le_bytes();
        a ^ b ^ c ^ d
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
                //dbg!(obj_name);
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
