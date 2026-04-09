pub mod disc_structures;
pub mod filesystem;
pub mod sys_structures;
pub mod util;

use thiserror::Error;

pub use self::sys_structures::Path;
use self::util::{BitPosition, DiscPosition};

const STRICT_MODE: bool = true;

#[derive(Error, Debug, Clone)]
pub enum Fault {
    #[error("Free link value 0x{0:04x} did not point at valid fragment")]
    InvalidFreeLink(u16),
    #[error(
        "Free fragment block at offset {origin:?} bits points to offset {dest_bit_offset:?} bits which does not contain a fragment"
    )]
    BrokenFreeChain {
        origin: BitPosition,
        dest_bit_offset: BitPosition,
    },
    #[error("File {path} has invalid attribute byte: {attr_value:b}")]
    InvalidAttr {
        location: BitPosition,
        path: Path,
        attr_value: u8,
    },
    #[error("Could not retrieve root directory")]
    InvalidRoot {
        root_link: DiscPosition,
        sector_size: usize,
    },
    #[error("Expected Nick or Hugo, found {}", str::escape_debug(&String::from_utf8_lossy(&*.0)))]
    MagicStringFailure([u8; 4]),

    #[error(
        "Directory {path} began with sequence number {start_seq_num} but ended with {end_seq_num}"
    )]
    SequenceNumberMismatch {
        path: Path,
        start_seq_num: u8,
        end_seq_num: u8,
    },
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
