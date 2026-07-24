// Overall global metadata structures for NewMap formats defined in http://www.riscos.com/support/developers/prm/filecore.html
//
// This does not include structures for ordinary filesystem entries such as
// directory records.

/// The offset of the allocation map from the beginning of the disk
///
/// Used to calculate the disk-absolute ranges associated with each allocation
const ALLOCATION_MAP_START_IN_BITS: usize = (3 + 61) * 8;

/// The xor of the cross check header values should be this value
///
/// https://www.riscos.com/support/developers/prm/filecore.html#98861
const EXPECTED_CROSS_CHECK: u8 = 0xFF;

pub(crate) mod allocation_map;
pub(crate) mod disc_header;
pub(crate) mod disc_record;
pub(crate) mod fragment_block;
pub(crate) mod map_block;
pub(crate) mod new_map;
