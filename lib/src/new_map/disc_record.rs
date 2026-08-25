use serde::Serialize;
use winnow::Parser;
use winnow::binary::{le_u8, le_u16, le_u32};
use winnow::combinator::{seq, trace};
use winnow::token::take;

use super::disc_header::Header;
use crate::error::{ErrExt, Fault};
use crate::filesystem::FixedLenString;
use crate::util::{DiscPosition, InputStream, ParseResult};

const REASONABLE_SECTOR_SIZES: &[usize] = &[256, 512, 1024, 2048, 4096];

/// Various metadata describing the overall disk geometry and "global"
/// filesystem metadata. Some values are particularly important as they are
/// needed to parse other structures on disk:
/// 1. `log2_sec_size`: the sector size, stored in its base-2 log form
/// 2. `idlen`: the length of a [`FragmentId`], in bits
/// 3. `root_dir`: the position of the root directory record, as a bit offset
/// 4. `log2_bytes_per_mapbit` - the number of bytes represented by each bit in
///    the Allocation Map
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DiscRecord {
    pub(crate) log2_sec_size: u8,
    pub(crate) secs_per_track: u8,
    pub(crate) heads: u8,
    pub(crate) density: u8,
    /// The length of the identifier in this disc's
    /// [`FragmentBlock`](`super::fragment_block::FragmentBlock`)
    pub(crate) idlen: u8,
    pub(crate) log2_bytes_per_mapbit: u8,
    pub(crate) skew: u8,
    /// Boot flags.
    /// TODO: Model these better
    pub(crate) boot_options: u8,
    pub(crate) low_sector: u8,
    /// Number of zones in this disc. Always 1 for format E.
    pub(crate) num_zones: u8,
    /// Number of bits excluded from the Allocation Map.
    ///
    /// Counts backwards from the end of the next zone's header. Subtracting the
    /// length of the header (32 bits) and then dividing by 8 gives the number
    /// of bytes of the Map sector that are excluded.
    pub(crate) zone_spare: u16,
    /// The indirect position of the root directory.
    ///
    /// The root directory is often part of the system object (id 2) so the
    /// sector offset part of this value is frequently not zero, unlike other
    /// positions.
    pub(crate) root_dir: DiscPosition,
    /// Total size of the whole disc, in bytes.
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
    pub(crate) fn ids_per_zone(&self) -> usize {
        ((1 << (self.log2_sec_size as usize + 3)) - self.zone_spare as usize)
            / (self.idlen as usize + 1)
    }
    pub(crate) fn zone_trailing_bytes(&self) -> usize {
        (self.zone_spare as usize - 8 * Header::LENGTH) / 8
    }
    pub(crate) fn allocation_map_size(&self, dr: DrInclusion) -> usize {
        self.sector_size_in_bytes()
            - Header::LENGTH
            - if dr == DrInclusion::WithDiscRecord {
                DiscRecord::LENGTH
            } else {
                0
            }
            - self.zone_trailing_bytes()
    }

    pub(crate) fn bytes_per_map_bit(&self) -> usize {
        2usize.pow(self.log2_bytes_per_mapbit as _) as _
    }
    pub(crate) fn zone_size_in_bytes(&self, dr: DrInclusion) -> usize {
        self.bytes_per_map_bit() * 8 * self.allocation_map_size(dr)
    }

    pub(crate) fn test_sector_size(s: u8) -> Result<u8, Fault> {
        if REASONABLE_SECTOR_SIZES.contains(&2usize.pow(s as _)) {
            Ok(s)
        } else {
            Err(Fault::UnacceptableSectorSize(s))
        }
    }
    pub(crate) fn get_zone_offset(&self, idx: usize) -> usize {
        match idx {
            0 => 0,
            1 => self.zone_size_in_bytes(DrInclusion::WithDiscRecord),
            n => {
                (n - 1) * self.zone_size_in_bytes(DrInclusion::WithoutDiscRecord)
                    + self.zone_size_in_bytes(DrInclusion::WithDiscRecord)
            }
        }
    }

    /// Parses the fields of the new-map disc record.
    ///
    /// Can fail with:
    /// - [`Fault::UnacceptableSectorSize`]
    pub(crate) fn parse<'a>(input: &mut InputStream<'a>) -> ParseResult<Self> {
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
        .note("parsing disc record")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrInclusion {
    WithDiscRecord,
    WithoutDiscRecord,
}
