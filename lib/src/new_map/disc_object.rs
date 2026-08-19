use std::num::Wrapping;
use std::range::Range;

use super::disc_record::DiscRecord;
use super::fragment_block::FragmentBlock;

type ZonedFragment = (usize, FragmentBlock);

const SYSTEM_FRAGMENT: usize = 2;

#[derive(Debug, Clone)]
pub(crate) struct DiscObject {
    regions: Vec<Range<usize>>,
    sector_size: usize,
}

impl DiscObject {
    pub(crate) fn new(mut fragments: Vec<ZonedFragment>, dr: &DiscRecord) -> DiscObject {
        // Assumes the fragment exists
        debug_assert!(!fragments.is_empty());
        // Assumes that all blocks are of the same fragment ID
        debug_assert!({
            let first = fragments.first().unwrap().1.id;
            fragments.iter().map(|(_, f)| f.id).all(|id| id == first)
        });

        sort_blocks(&mut fragments, dr.ids_per_zone(), dr.num_zones as _);

        let mut regions = vec![];

        for (zone_idx, block) in fragments {
            let zone_offset = dr.get_zone_offset(zone_idx);
            let bit_start = block.position.bits();

            let bit_end = bit_start + block.map_length;

            let byte_range = Range {
                start: dr.bytes_per_map_bit() * bit_start + zone_offset,
                end: dr.bytes_per_map_bit() * bit_end + zone_offset,
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

fn sort_blocks(fragments: &mut [(usize, FragmentBlock)], ids_per_zone: usize, num_zones: usize) {
    // "Object 2, being the object which carries the map with it, is special. It is
    // always at the beginning of the middle zone, as opposed to being at the
    // beginning of zone 0."
    // The boot block of format F disks is included in object 2, but this check
    // sorts that section to the end rather than the beginning. This means the
    // beginning of object 2 is the set of map blocks, which begin at the halfway
    // zone. https://www.riscos.com/support/developers/prm/filecore.html#32170
    let fragment_id = fragments.first().expect("Empty DiscObject is invalid").1.id;

    let starting_zone = if fragment_id as usize == SYSTEM_FRAGMENT {
        num_zones.div_ceil(2)
    } else {
        fragment_id as usize / ids_per_zone
    };

    fragments.sort_by_key(|&(zone, f)| {
        let adjusted_zone = Wrapping(zone) - Wrapping(starting_zone);
        (adjusted_zone, f.position)
    });
}

#[cfg(test)]
mod test {
    use crate::filesystem::FixedLenString;
    use crate::new_map::disc_object::{DiscObject, sort_blocks};
    use crate::new_map::disc_record::DiscRecord;
    use crate::new_map::fragment_block::FragmentBlock;
    use crate::util::{BitPosition, DiscPosition};

    #[test]
    fn ordering_test() {
        let mut fragments: Vec<_> = (0..6)
            .map(|i| {
                (
                    i / 2,
                    FragmentBlock {
                        id: 1000,
                        free_space: false,
                        position: BitPosition::from_bits(i * 80),
                        map_length: 0,
                    },
                )
            })
            .collect();

        sort_blocks(&mut fragments, 400, 4);

        let ordering: Vec<_> = fragments
            .into_iter()
            .map(|(z, f)| (z, f.position.bits()))
            .collect();
        insta::assert_compact_debug_snapshot!(ordering, @"[(2, 320), (2, 400), (0, 0), (0, 80), (1, 160), (1, 240)]");
    }

    #[test]
    fn shortening_ranges() {
        let dr = DiscRecord {
            log2_sec_size: 10,
            secs_per_track: 0,
            heads: 0,
            density: 0,
            idlen: 0,
            log2_bytes_per_mapbit: 4,
            skew: 0,
            boot_options: 0,
            low_sector: 0,
            num_zones: 0,
            zone_spare: 0,
            root_dir: DiscPosition(0),
            size: 0,
            disc_id: 0,
            disc_name: FixedLenString::from_bytes_dynamic(&[]),
            disc_type: 0,
        };

        let fragments = (0..3)
            .map(|i| {
                (
                    0,
                    FragmentBlock {
                        id: 0,
                        free_space: false,
                        map_length: 128,
                        position: BitPosition::from_bits(128 * i),
                    },
                )
            })
            .collect();

        insta::assert_compact_debug_snapshot!(dr.sector_size_in_bytes(), @"1024");

        let obj = DiscObject::new(fragments, &dr);

        let originals = obj.get_disc_regions(0, obj.total_length()).unwrap();
        insta::assert_compact_debug_snapshot!(originals, @"[0..2048, 2048..4096, 4096..6144]");

        let trimmed_regions = obj.get_disc_regions(3, 1024).unwrap();
        insta::assert_compact_debug_snapshot!(trimmed_regions, @"[3072..4096]");

        let failure = obj.get_disc_regions(3, 1024 + 2048 + 1);
        insta::assert_compact_debug_snapshot!(failure, @"None");
    }
}
