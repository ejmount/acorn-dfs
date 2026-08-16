use serde::Serialize;
use winnow::stream::{Offset, Stream};

use super::allocation_map::AllocationMap;
use super::disc_header::Header;
use super::disc_record::DiscRecord;
use super::fragment_block::FragmentBlock;
use crate::error::{Fault, FaultValue};
use crate::util::{AllocationParsingParams, FaultableResult, FragmentId, InputStream, make_input};

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
}

impl MapBlock {
    pub(crate) fn parse<'a>(
        input: &mut InputStream<'a>,
        disc_record: &DiscRecord,
    ) -> FaultableResult<'a, Self> {
        // Already know the size of the sector ahead of time, so we can immediately
        // create a stream of only this sector.
        let sector_length = disc_record.sector_size_in_bytes();
        let sector = &input[..sector_length];

        // Keep this separate from the raw data so we can go back to the full sector
        // easily
        let mut sector_stream = make_input(sector);

        // Header is always there
        let header = Header::parse(&mut sector_stream)?;

        // Do the zone checksum since we have this data. The calculation does not
        // advance the stream.
        let faults = MapBlock::test_checksum(header.zone_check, sector);

        let params = AllocationParsingParams::new(header.free_link, disc_record);
        // Parsing the AM expects *only* the sector as input, the end point of the
        // stream is load-bearing.
        let allocations = AllocationMap::parse(&mut sector_stream, &params)?;

        // Advance the input to the beginning of the next sector.
        input.next_slice(sector_length);

        Ok(FaultValue(
            MapBlock {
                header,
                allocations,
            },
            faults.into_iter().collect(),
        ))
    }

    /// Parses the MapBlock and the contained DiscRecord.
    ///
    /// This is expected to be used to parse the initial map block (the only one
    /// in format E) which is then followed with calling [`Self::parse`] once
    /// per remaining zone. This choice must be made by the caller as there
    /// appears to be no robust way to determine it from the stream alone.
    ///
    /// ## Errors
    ///
    /// Can fail when:
    /// - the Disc Record is invalid (see [`DiscRecord::parse`]), currently only
    ///   because of a suspect sector size
    /// - the AllocationMap fails to build, which is most likely to be because
    ///   the [free list is not traversable](AllocationMap::walk_free_chain).
    pub(crate) fn parse_with_disc_record<'a>(
        input: &'_ mut InputStream<'a>,
    ) -> FaultableResult<'a, (Self, DiscRecord)> {
        // Take note of the starting point of the stream so we can use it as a baseline
        let start = input.checkpoint();

        // Header is always there
        let header = Header::parse(input)?;

        // Read the Disc Record
        let disc_record = DiscRecord::parse(input)?;

        // Having parsed the header and any disc record, record where we are within the
        // sector
        let allocation_offset = input.checkpoint().offset_from(&start);

        // Reset the input to the start of the sector so we can create a sub-stream of
        // only the sector.
        input.reset(&start);
        let sector_length = disc_record.sector_size_in_bytes();
        let sector = &input[..sector_length];

        // Do the zone checksum since we have this data. The calculation does not
        // advance the stream.
        let fault = MapBlock::test_checksum(header.zone_check, sector);

        // Advance the substream past the header and any DR.
        let mut sector = make_input(sector);
        sector.next_slice(allocation_offset);

        // Parse the allocation map.
        let params = AllocationParsingParams::new(header.free_link, &disc_record);
        // Parsing the AM expects *only* the sector as input, the end point of the
        // stream is load-bearing.
        let allocations = AllocationMap::parse(&mut sector, &params)?;

        // Advance the input to the beginning of the next sector.
        input.next_slice(sector_length);

        Ok(FaultValue(
            (
                MapBlock {
                    header,
                    allocations,
                },
                disc_record,
            ),
            fault.into_iter().collect(),
        ))
    }

    pub(crate) fn get_fragments(&self, id: FragmentId) -> Vec<FragmentBlock> {
        self.allocations.get_fragments(id)
    }

    /// Run the zone check and create a fault code if needed.
    fn test_checksum(expected_zone_check: u8, sector: &[u8]) -> Option<Fault> {
        // Do the zone checksum since we have this data. The calculation does not
        // advance the stream.
        let actual_zone_check = MapBlock::calculate_zone_check(sector);
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
