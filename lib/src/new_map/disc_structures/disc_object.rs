use std::range::Range;

use super::disc_record::DiscRecord;
use super::fragment_block::FragmentBlock;
use crate::new_map::disc_structures::ALLOCATION_MAP_START_IN_BITS;

#[derive(Debug, Clone)]
pub(crate) struct DiscObject {
    regions: Vec<Range<usize>>,
    sector_size: usize,
}

impl DiscObject {
    pub(crate) fn new(fragments: Vec<FragmentBlock>, dr: &DiscRecord) -> DiscObject {
        // Assumes the fragment exists
        debug_assert!(!fragments.is_empty());
        // Assumes that all blocks are of the same fragment ID
        debug_assert!({
            let first = fragments.first().unwrap().id;
            fragments.iter().map(|f| f.id).all(|id| id == first)
        });

        let mut regions = vec![];

        for block in &fragments {
            let bit_start = block.position.bits() - ALLOCATION_MAP_START_IN_BITS;

            let bit_end = bit_start + block.map_length;

            let byte_range = Range {
                start: dr.bytes_per_map_bit() * bit_start,
                end: dr.bytes_per_map_bit() * bit_end,
            };
            regions.push(byte_range);
        }

        DiscObject {
            regions,
            sector_size: dr.sector_size_in_bytes(),
        }
    }

    #[cfg(test)]
    pub(crate) fn total_length(&self) -> usize {
        self.regions.iter().copied().map(Self::range_size).sum()
    }

    pub(crate) fn range_size(Range { start, end }: Range<usize>) -> usize {
        end - start
    }

    /// Given an offset, and a range, advances the start of the range by the
    /// given offset and updates the offset accordingly. A Some is always
    /// returned even if the range is empty because the intention is skip items,
    /// whereas scan will interpret a None as termination.
    fn advance_range(offset_remaining: &mut usize, r: Range<usize>) -> Option<Range<usize>> {
        let forward_step = Self::range_size(r).min(*offset_remaining);

        *offset_remaining -= forward_step;
        Some(Range {
            start: r.start + forward_step,
            end: r.end,
        })
    }

    /// Given an length budget, and a range, reduces the budget by the range's
    /// size, and shrinks the range if insufficient budget left.
    ///
    /// A Some is always returned even if the range is empty because the
    /// intention is skip items, whereas scan will interpret a None as
    /// termination.
    fn trim_range(length_remaining: &mut usize, r: Range<usize>) -> Option<Range<usize>> {
        let remaining_length = Self::range_size(r).min(*length_remaining);
        *length_remaining -= remaining_length;

        Some(Range {
            start: r.start,
            end: r.start + remaining_length,
        })
    }

    /// Produce a new set of ranges, so that the start of the new first
    /// range is the given number of sectors further ahead compared to the start
    /// of the original first. This process may remove entire ranges
    /// from the original sequence and the new starting point may be in the
    /// middle of an existing range.
    ///
    /// A disk might have a sector size of 1024 and a fragment on that disk
    /// might have be allocated in byte ranges <[0, 2048], [2048, 4096], [4096,
    /// 6144]>. If the caller asks for sector 3, we find an offset
    /// of 3072. As a result, we remove the first range entirely and advance
    /// past part of the second. This results in a new range-set of
    /// <[3072, 4096], [4096, 6144]>.
    ///
    /// Returns `None` if the resulting new ranges are too short for the
    /// requested length. This corresponds to an out-of-bounds read.
    pub(crate) fn get_disc_regions(
        &self,
        sector_number: usize,
        len: usize,
    ) -> Option<Vec<Range<usize>>> {
        let total_offset = sector_number * self.sector_size;

        // Have to return empty lists because scan will stop if we return None rather
        // than skip items
        let new_regions: Vec<_> = self
            .regions
            .iter()
            .copied()
            .scan(total_offset, Self::advance_range)
            .scan(len, Self::trim_range)
            .filter(|r| !r.is_empty())
            .collect();

        let total_length: usize = new_regions.iter().copied().map(Self::range_size).sum();

        if total_length >= len {
            Some(new_regions)
        } else {
            None
        }
    }
}
