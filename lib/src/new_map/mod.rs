const STRICT_MODE: bool = true;

pub mod disc_structures;
pub mod filesystem;
pub mod sys_structures;
pub mod util;

use thiserror::Error;

use crate::new_map::{
    sys_structures::Path,
    util::{BitPosition, DiscPosition},
};

#[derive(Error, Debug, Clone)]
pub enum Fault {
    #[error("Free link value {0:04x} did not point at valid fragment")]
    InvalidFreeLink(u16),
    #[error(
        "Free fragment block at offset {origin:?} bits points to offset {dest_bit_offset:?} bits which does not contain a fragment"
    )]
    BrokenFreeChain {
        origin: BitPosition,
        dest_bit_offset: BitPosition,
    },
    #[error("File {path:?} has invalid attribute byte")]
    InvalidAttr {
        location: BitPosition,
        path: Path,
        attr_value: u8,
    },
    #[error("Could not retreieve root directory")]
    InvalidRoot {
        root_link: DiscPosition,
        sector_size: usize,
    },
    #[error("Expected Nick or Hugo, found {}", str::escape_debug(&String::from_utf8_lossy(&*.0)))]
    MagicStringFailure([u8; 4]),
}

#[derive(Debug, Clone)]
pub(crate) struct FaultValue<T>(pub(crate) T, pub(crate) Vec<Fault>);
impl<T> FaultValue<T> {
    pub(crate) fn unpack(FaultValue(t, f): Self) -> (T, Vec<Fault>) {
        (t, f)
    }
}
impl<T> From<T> for FaultValue<T> {
    fn from(value: T) -> Self {
        FaultValue(value, vec![])
    }
}
