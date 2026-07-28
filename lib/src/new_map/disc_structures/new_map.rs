use std::ops::{BitXor, Range};

use serde::Serialize;

use super::super::Fault;
use super::super::filesystem::DirEntry;
use super::super::util::InputStream;
use super::allocation_map::AllocationMap;
use super::disc_record::DiscRecord;
use super::fragment_block::FragmentBlock;
use super::map_block::MapBlock;
use crate::new_map::FaultValue;
use crate::new_map::disc_structures::disc_record::DrInclusion;
use crate::new_map::util::{FaultableResult, FragmentId};

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
/// allows handling for the MapBlocks to be uniform.
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
    pub fn parse<'a>(input: &mut InputStream<'a>, num_zones: usize) -> FaultableResult<'a, Self> {
        assert!(num_zones > 0);
        let FaultValue((leading_block, disc_record), mut faults) =
            MapBlock::parse_with_disc_record(input)?;

        let mut blocks = vec![leading_block];

        for _ in 0..(num_zones - 1) {
            let FaultValue(block, sub_faults) =
                MapBlock::parse(input, DrInclusion::WithoutDiscRecord, &disc_record)?;
            faults.extend(sub_faults);
            blocks.push(block);
        }

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

    pub(crate) fn get_allocation(&self, idx: usize) -> &AllocationMap {
        &self.blocks[idx].allocations
    }
    pub(crate) fn get_fragment(&self, id: FragmentId) -> Option<&FragmentBlock> {
        // TODO: Update this for multiple zones
        let mut fragment = self.blocks[0].get_fragment(id);
        for b in &self.blocks {
            fragment = fragment.or(b.get_fragment(id))
        }
        fragment
    }
    pub(crate) fn get_file_region(&self, dir_entry: &DirEntry) -> Option<Range<usize>> {
        let position = dir_entry.address;
        let fragment = self.get_fragment(position.fragment())?;

        let sector_number = position.sector_idx();
        let sector_size = self.disc_record.sector_size_in_bytes();
        let byte_offset: usize = sector_number as usize * sector_size;

        let Range { start, end } = fragment.disk_region();
        let start = start + byte_offset;
        let end = end.min(start + dir_entry.len as usize);

        debug_assert!(
            (end - start) == dir_entry.len as usize,
            "Region ({start}..{end}) does not match file length {}",
            dir_entry.len
        );

        Some(Range { start, end })
    }

    pub fn cross_check(&self) -> u8 {
        self.blocks
            .iter()
            .map(|b| b.header.cross_check)
            .reduce(BitXor::bitxor)
            .expect("Map should not contain zero blocks")
    }
}
