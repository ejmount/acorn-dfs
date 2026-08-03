//! Overall global metadata structures for NewMap formats defined in http://www.riscos.com/support/developers/prm/filecore.html
//!
//! This does not include structures for ordinary filesystem entries such as
//! directory records.
//!
//! The entry point to this module is [`new_map::FormatE`]

/// The offset of the allocation map from the beginning of the disk
///
/// Used to calculate the disk-absolute ranges associated with each allocation
const ALLOCATION_MAP_START_IN_BITS: usize = (3 + 61) * 8;

pub(crate) mod allocation_map;
pub(crate) mod disc_header;
pub(crate) mod disc_object;
pub(crate) mod disc_record;
pub(crate) mod fragment_block;
pub(crate) mod map_block;
pub(crate) mod new_map;
