const STRICT_MODE: bool = true;

pub mod disc_structures;
pub mod filesystem;
pub mod sys_structures;
pub mod util;

use thiserror::Error;

use crate::new_map::util::BitPosition;

#[derive(Error, Debug)]
pub enum LoadErrors {
    #[error("Free link value {0:04x} did not point at valid fragment")]
    InvalidFreeLink(u16),
    #[error(
        "Free fragment block at offset {origin:?} bits points to offset {dest_bit_offset:?} bits which does not contain a fragment"
    )]
    BrokenFreeChain {
        origin: BitPosition,
        dest_bit_offset: BitPosition,
    },
    #[error("File {filename:?} has invalid attribute byte")]
    InvalidAttr {
        location: BitPosition,
        filename: String,
    },
}
