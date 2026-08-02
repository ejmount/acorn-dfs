use serde::Serialize;
use winnow::Parser;
use winnow::binary::{le_u8, le_u16, le_u32};
use winnow::combinator::{seq, trace};
use winnow::token::take;

use super::super::util::{DiscPosition, FixedLenString, InputStream, ParseResult};
use super::super::{Fault, STRICT_MODE};

/// Various metadata describing the overall disk geometry and "global"
/// filesystem metadata. Some values are particularly important as they are
/// needed to parse other structures on disk:
/// 1. `log2_sec_size`: the sector size, stored in its base-2 log form
/// 2. `idlen`: the length of a [`FragmentId`], in bits
/// 3. `root_dir`: the position of the root directory record, as a byte offset
/// 4. `size`: the total size of the disk, which then dictates how large the
///    Allocation Map is
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DiscRecord {
    pub(crate) log2_sec_size: u8,
    pub(crate) secs_per_track: u8,
    pub(crate) heads: u8,
    pub(crate) density: u8,
    pub(crate) idlen: u8,
    pub(crate) log2_bytes_per_mapbit: u8,
    pub(crate) skew: u8,
    // TODO: Model these better
    pub(crate) boot_options: u8,
    pub(crate) low_sector: u8,
    pub(crate) num_zones: u8,
    pub(crate) zone_spare: u16,
    pub(crate) root_dir: DiscPosition,
    pub(crate) size: u32,
    pub(crate) disc_id: u16,
    pub(crate) disc_name: FixedLenString,
    pub(crate) disc_type: u32,
}

impl DiscRecord {
    pub(crate) const LENGTH: usize = 60;

    pub(crate) fn sector_size_in_bytes(&self) -> usize {
        2usize.pow(self.log2_sec_size as _)
    }
    pub(crate) fn zone_size_in_bytes(&self) -> usize {
        (self.size / self.num_zones as u32) as _
    }
    pub(crate) fn ids_per_zone(&self) -> usize {
        ((1 << (self.log2_sec_size as usize + 3)) - self.zone_spare as usize)
            / (self.idlen as usize + 1)
    }
    pub(crate) fn test_sector_size(s: u8) -> Result<u8, Fault> {
        if !STRICT_MODE || [8, 9, 10, 11].contains(&s) {
            Ok(s)
        } else {
            Err(Fault::UnacceptableSectorSize(s))
        }
    }
    pub(crate) fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<'a, Self> {
        trace(
            "DiscRecord",
            seq! {
                DiscRecord {
                    log2_sec_size: le_u8.try_map(Self::test_sector_size),
                    secs_per_track: le_u8,
                    heads: le_u8,
                    density: le_u8,
                    idlen: le_u8,
                    log2_bytes_per_mapbit: le_u8,
                    skew: le_u8,
                    boot_options: le_u8,
                    low_sector: le_u8,
                    num_zones: le_u8,
                    zone_spare: le_u16,
                    root_dir: le_u32.map(DiscPosition),
                    size: le_u32,
                    disc_id: le_u16,
                    disc_name: FixedLenString::parse_from_disk,
                    disc_type: le_u32,
                    _: take(24usize), // overall structure is 60 bytes long, tail end is reserved
                }
            },
        )
        .parse_next(input)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrInclusion {
    WithDiscRecord,
    WithoutDiscRecord,
}
