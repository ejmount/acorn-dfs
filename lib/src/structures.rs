use winnow::Parser;
use winnow::combinator::repeat;
use winnow::error::EmptyError;
use winnow::token::{any, take};

const MAX_FREE_SPACE_ENTRIES: usize = 82;
const FREE_SPACE_MAP_LENGTH: usize = 2048;
const DISK_ID_LENGTH: usize = 2;

fn parse_3_byte_number(input: &mut &[u8]) -> Result<u32, EmptyError> {
    let (&bytes, remainder) = input.split_first_chunk().ok_or(EmptyError)?;
    *input = remainder;
    let [i1, i2, i3] = bytes.map(u32::from);
    let sum = i1 + (i2 << 8) + (i3 << 16);
    Ok(sum)
}

struct SectorNumber(u32);

impl SectorNumber {
    fn parse(input: &mut &[u8]) -> Result<SectorNumber, EmptyError> {
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
    free_space_lengths: Vec<u32>,
    number_of_sectors: u32,
    name_chars: (Vec<u8>, Vec<u8>),
    reported_sec0_checksum: u8,
    reported_sec1_checksum: u8,
    partition1_offset: SectorNumber,
    partition2_offset: SectorNumber,
    disk_id: [u8; DISK_ID_LENGTH],
    boot_options: u8,
    ptr_end_free_list: u8,
}

impl FreeSpaceMap {
    pub fn from_bytes(data: &[u8; FREE_SPACE_MAP_LENGTH]) -> Result<FreeSpaceMap, EmptyError> {
        let cursor = &mut &data[..];

        let free_space_offsets: Vec<_> =
            repeat(MAX_FREE_SPACE_ENTRIES, SectorNumber::parse).parse_next(cursor)?;
        let partition1_offset = SectorNumber::parse.parse_next(cursor)?;

        let odd_name_chars = take(5usize).parse_next(cursor)?.to_vec();

        let number_of_sectors = parse_3_byte_number.parse_next(cursor)?;

        let reported_sec0_checksum = any(cursor)?;

        let free_space_lengths: Vec<_> =
            repeat(MAX_FREE_SPACE_ENTRIES, parse_3_byte_number).parse_next(cursor)?;

        let partition2_offset = SectorNumber::parse.parse_next(cursor)?;

        let even_name_chars = take(5usize).parse_next(cursor)?.to_vec();

        let disk_id: [u8; DISK_ID_LENGTH] =
            *take(2usize).parse_next(cursor)?.first_chunk().unwrap();

        let boot_options = any(cursor)?;

        let ptr_end_free_list = any(cursor)?;

        let reported_sec1_checksum = any(cursor)?;

        let map = FreeSpaceMap {
            free_space_offsets,
            free_space_lengths,
            number_of_sectors,
            name_chars: (odd_name_chars, even_name_chars),
            reported_sec0_checksum,
            reported_sec1_checksum,
            partition1_offset,
            partition2_offset,
            disk_id,
            boot_options,
            ptr_end_free_list,
        };
        Ok(map)
    }
}
