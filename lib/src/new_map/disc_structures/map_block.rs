use serde::Serialize;
use winnow::Parser;
use winnow::error::EmptyError;
use winnow::stream::{Location, Stream};
use winnow::token::take;

use super::allocation_map::AllocationMap;
use super::disc_header::Header;
use super::disc_record::{DiscRecord, DrInclusion};
use super::fragment_block::FragmentBlock;
use crate::new_map::util::{
    AllocationParsingParams,
    FaultableResult,
    FragmentId,
    InputStream,
    take_rest_of_sector,
};
use crate::new_map::{Fault, FaultValue};

/// The MapBlock ordinarily contains
/// 1. various validation checksums in the [`Header`]
/// 2. the beginning of the free list for this zone, also in the [`Header`]
/// 3. a section of the allocation map
///
/// A MapBlock is also padded to be exactly one disk sector long, which
/// means the DiscRecord must be accessible. For the first block, we
/// have this earlier in the parsing step, but otherwise it must be
/// passed in.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct MapBlock {
    pub(crate) header: Header,
    #[serde(skip)]
    pub(crate) allocations: AllocationMap,
    #[serde(skip)]
    /// the remainder of the sector
    pub(crate) _unused: Vec<u8>,
}

impl MapBlock {
    pub(crate) fn parse<'a>(
        input: &mut InputStream<'a>,
        includes_map: DrInclusion,
        disc: &DiscRecord,
    ) -> FaultableResult<'a, Self> {
        let header = Header::parse(input)?;
        let params = AllocationParsingParams::new(includes_map, header.free_link, disc);

        let allocations = AllocationMap::parse(input, &params)?;
        let remainder = disc.sector_size_in_bytes()
            - (input.current_token_start() % disc.sector_size_in_bytes());
        let _unused = Vec::from(take(remainder).parse_next(input)?);

        // TODO: Add zone_check validation

        Ok(MapBlock {
            header,
            allocations,
            _unused,
        }
        .into())
    }
    pub(crate) fn parse_with_disc_record<'a>(
        input: &'_ mut InputStream<'a>,
    ) -> FaultableResult<'a, (Self, DiscRecord)> {
        let sector_start = input.checkpoint();

        let header = Header::parse(input)?;
        let disc_record = DiscRecord::parse(input)?;
        let params = AllocationParsingParams::new(
            DrInclusion::WithDiscRecord,
            header.free_link,
            &disc_record,
        );
        let allocations = AllocationMap::parse(input, &params)?;
        let _unused = take_rest_of_sector(input, disc_record.sector_size_in_bytes())?.to_vec();

        let sector_end = input.checkpoint();
        input.reset(&sector_start);
        let sector_contents = take(params.sector_size())
            .parse_next(input)
            .unwrap_or_else(|_: EmptyError| unreachable!(" should've already succedeed"));

        let fault = MapBlock::test_checksum(header.zone_check, sector_contents);
        input.reset(&sector_end);

        Ok(FaultValue(
            (
                MapBlock {
                    header,
                    allocations,
                    _unused,
                },
                disc_record,
            ),
            fault.into_iter().collect(),
        ))
    }
    pub(crate) fn get_fragments(&self, id: FragmentId) -> Vec<FragmentBlock> {
        self.allocations.get_fragments(id)
    }

    /// Run the zone check and create a fault code if needed
    fn test_checksum(expected_zone_check: u8, sector: &[u8]) -> Option<Fault> {
        // Do the zone checksum since we have this data. The calculation does not
        // advance the stream.
        let actual_zone_check = MapBlock::calculate_zone_check(sector);
        // Set up fault code if checksum failed
        if actual_zone_check != expected_zone_check {
            Some(Fault::ZoneCheckFailure {
                actual: actual_zone_check,
                expected: expected_zone_check,
            })
        } else {
            None
        }
    }

    /// Zone checksum
    /// https://www.riscos.com/support/developers/prm/filecore.html#72701
    pub(crate) fn calculate_zone_check(zone: &[u8]) -> u8 {
        use std::ops::BitXor;

        let mut sum = [0u32; 4];
        for rover in (4..zone.len()).step_by(4).rev() {
            sum[0] += zone[rover] as u32 + (sum[3] >> 8);
            sum[3] &= 0xFF;
            sum[1] += zone[rover + 1] as u32 + (sum[0] >> 8);
            sum[0] &= 0xFF;
            sum[2] += zone[rover + 2] as u32 + (sum[1] >> 8);
            sum[1] &= 0xFF;
            sum[3] += zone[rover + 3] as u32 + (sum[2] >> 8);
            sum[2] &= 0xFF;
        }

        sum[0] += sum[3] >> 8;
        sum[1] += zone[1] as u32 + (sum[0] >> 8);
        sum[2] += zone[2] as u32 + (sum[1] >> 8);
        sum[3] += zone[3] as u32 + (sum[2] >> 8);
        sum.into_iter().reduce(u32::bitxor).unwrap() as u8
    }
}
