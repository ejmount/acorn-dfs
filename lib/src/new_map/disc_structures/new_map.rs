use std::ops::BitXor;
use std::range::Range;

use serde::Serialize;

use super::super::Fault;
use super::super::filesystem::DirEntry;
use super::super::util::InputStream;
use super::disc_object::DiscObject;
use super::disc_record::DiscRecord;
use super::map_block::MapBlock;
use crate::new_map::util::{DiscPosition, FaultableResult};
use crate::new_map::{FaultValue, IoError};

/// The xor of the cross check header values should be this value
///
/// https://www.riscos.com/support/developers/prm/filecore.html#98861
const EXPECTED_CROSS_CHECK: u8 = 0xFF;

/// The "new"-style file allocation map structure, used by format E and F
/// disks.
///
/// Format E and F disks are conceptually divided into a number of zones,
/// with one [`MapBlock`] per zone. Conceptually, the first Map Block is
/// special because it contains the [`DiscRecord`], which charcteristics
/// the geometry of the disk. Parsing the collection of `MapBlocks` is
/// not straightforward because the exact size of a `MapBlock` is
/// defined by the disc geometry recorded in the `DiscRecord`.
///
/// In practice, we ignore the DR being part of the first MB and instead
/// represent it at the higher level as part of the overall Map, which
/// allows handling for the MapBlocks to be (relatively) uniform.
#[derive(Debug, Clone, Serialize)]
pub struct NewMap {
    /// Various metadata defining the goemetry of the disc
    ///
    /// On disk, this is stored inside the first MapBlock, but is pulled out
    /// here so Map handling is more consistent
    pub(crate) disc_record: DiscRecord,
    /// Zone blocks.
    pub(crate) blocks: Vec<MapBlock>,
}

impl NewMap {
    /// Construct a format-E NewMap out of the given byte stream
    pub fn parse_format_e<'a>(input: &mut InputStream<'a>) -> FaultableResult<'a, Self> {
        let FaultValue((leading_block, disc_record), mut faults) =
            MapBlock::parse_with_disc_record(input)?;

        let blocks = vec![leading_block];

        let new_map = NewMap {
            disc_record,
            blocks,
        };

        if new_map.cross_check() != EXPECTED_CROSS_CHECK {
            faults.push(Fault::CrossCheckFailure(new_map.cross_check()));
        }

        Ok(FaultValue(new_map, faults))
    }
    pub(crate) fn get_disc_record(&self) -> &DiscRecord {
        &self.disc_record
    }

    pub(crate) fn get_disc_object(&self, position: DiscPosition) -> Result<DiscObject, IoError> {
        let mut total_fragments = vec![];
        for block in &self.blocks {
            let fragments = block.get_fragments(position.fragment());
            total_fragments.extend(fragments);
        }
        if !total_fragments.is_empty() {
            Ok(DiscObject::new(total_fragments, &self.disc_record))
        } else {
            Err(IoError::MissingFragment(position))
        }
    }
    pub(crate) fn get_file_regions(
        &self,
        dir_entry: &DirEntry,
    ) -> Result<Vec<Range<usize>>, IoError> {
        let position = dir_entry.address;
        let disc_object = self.get_disc_object(position)?;

        let sector_number = position.sector_idx();
        let regions = disc_object
            .get_disc_regions(sector_number as _, dir_entry.len as _)
            .ok_or(IoError::OutOfBounds {
                object: position,
                length: dir_entry.len as _,
            })?;

        Ok(regions)
    }

    pub fn cross_check(&self) -> u8 {
        self.blocks
            .iter()
            .map(|b| b.header.cross_check)
            .reduce(BitXor::bitxor)
            .expect("Map should not contain zero blocks")
    }
}
