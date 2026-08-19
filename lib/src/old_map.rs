use winnow::Parser;
use winnow::binary::le_u16;
use winnow::combinator::repeat;
use winnow::stream::Location;
use winnow::token::{any, take};

use crate::filesystem::FixedLenString;
use crate::util::{InputStream, ParseResult};

const MAX_FREE_SPACE_ENTRIES: usize = 82;
const FREE_SPACE_MAP_LENGTH: usize = 512;
const DISC_NAME_LENGTH: usize = 10;
const HALF_NAME_LENGTH: usize = DISC_NAME_LENGTH / 2;

fn parse_3_byte_number<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, u32> {
    let [i1, i2, i3] = *take(3usize).parse_next(input)? else {
        unreachable!()
    };
    Ok(u32::from_le_bytes([i1, i2, i3, 0]))
}

struct SectorNumber(u32);

impl SectorNumber {
    fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, SectorNumber> {
        parse_3_byte_number(input).map(SectorNumber)
    }
}

impl std::fmt::Debug for SectorNumber {
    // Because the automatic one includes newlines
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SectorNumber({:x})", self.0)
    }
}

#[derive(Debug)]
pub struct FreeSpaceMap {
    free_space_offsets: Vec<SectorNumber>,
    name_odd: FixedLenString<HALF_NAME_LENGTH>,
    sector_count: u32,
    sec0_checksum: u8,
    free_space_lengths: Vec<u32>,
    name_even: FixedLenString<HALF_NAME_LENGTH>,
    disk_id: u16,
    boot_options: u8,
    ptr_end_free_list: u8,
    sec1_checksum: u8,
}

impl FreeSpaceMap {
    pub fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, FreeSpaceMap> {
        let start = input.current_token_start();

        let free_space_offsets =
            repeat(MAX_FREE_SPACE_ENTRIES, SectorNumber::parse).parse_next(input)?;

        let _reserved = 0b0.parse_next(input)?;

        let name_odd = FixedLenString::parse_from_disk(input)?;

        let sector_count = parse_3_byte_number.parse_next(input)?;

        let sec0_checksum = any(input)?;

        let free_space_lengths =
            repeat(MAX_FREE_SPACE_ENTRIES, parse_3_byte_number).parse_next(input)?;

        let name_even = FixedLenString::parse_from_disk(input)?;

        let disk_id = le_u16.parse_next(input)?;

        let boot_options = any(input)?;

        let ptr_end_free_list = any(input)?;

        let sec1_checksum = any(input)?;

        let length = input.current_token_start() - start;

        debug_assert_eq!(length, FREE_SPACE_MAP_LENGTH);

        let map = FreeSpaceMap {
            free_space_offsets,
            free_space_lengths,
            sector_count,
            name_odd,
            sec0_checksum,
            sec1_checksum,
            name_even,
            disk_id,
            boot_options,
            ptr_end_free_list,
        };
        Ok(map)
    }

    pub fn disc_name(&self) -> FixedLenString<DISC_NAME_LENGTH> {
        let (odd, even) = &(self.name_odd, self.name_even);

        debug_assert!((odd.len() == even.len()) || (odd.len() == even.len() + 1));

        let mut output: Vec<_> = odd
            .valid_range()
            .iter()
            .copied()
            .zip(even.valid_range().iter().copied())
            .flat_map(|(a, b)| [a, b].into_iter())
            .collect();

        if odd.len() == even.len() + 1 {
            output.push(*odd.valid_range().last().unwrap());
        }

        FixedLenString::from_bytes_dynamic(&output)
    }
}

#[cfg(test)]
mod test {
    use insta::assert_compact_debug_snapshot;

    use super::FreeSpaceMap;
    use crate::filesystem::FixedLenString;

    #[test]
    fn test_names() {
        let empty_map = |name_odd, name_even| FreeSpaceMap {
            boot_options: 0,
            disk_id: 0,
            free_space_lengths: vec![],
            free_space_offsets: vec![],
            ptr_end_free_list: 0,
            sec0_checksum: 0,
            sec1_checksum: 0,
            sector_count: 0,
            name_odd,
            name_even,
        };

        let f = empty_map(
            FixedLenString::from_bytes_dynamic(&[b'A', b'C', b'E', b'G']),
            FixedLenString::from_bytes_dynamic(&[b'B', b'D', b'F']),
        );
        let g = empty_map(
            FixedLenString::from_bytes_dynamic(&[b'A', b'C', b'E', b'G']),
            FixedLenString::from_bytes_dynamic(&[b'B', b'D', b'F', b'H']),
        );

        assert_compact_debug_snapshot!(&f.disc_name(), @r#"FixedLenString("ABCDEFG\0\0\0")"#);
        assert_compact_debug_snapshot!(&g.disc_name(), @r#"FixedLenString("ABCDEFGH\0\0")"#);
    }
}
