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
    #[error("Expected 'Nick' or 'Hugo', found {}", str::escape_debug(&String::from_utf8_lossy(&*.0)))]
    MagicStringFailure([u8; 4]),

    #[error(
        "Directory {path} began with sequence number 0x{start_seq_num:X} but ended with 0x{end_seq_num:X}"
    )]
    SequenceNumberMismatch {
        path: Path,
        start_seq_num: u8,
        end_seq_num: u8,
    },
    #[error("Detected sector size ({}) was too big or small to be plausible", 2usize.pow(*.0 as _))]
    UnacceptableSectorSize(u8),
    #[error("Calculated a zone check value of {actual:2X}, but expected {expected:2X}")]
    ZoneCheckFailure { expected: u8, actual: u8 },
#[error("Map had a cross check failure of {:2X}", .0)]
    CrossCheckFailure(u8),
}

#[derive(Error, Debug, Clone)]
pub enum IoError {
    #[error("Could not find indirect disc position {:?}", .0)]
    MissingFragment(DiscPosition),
    #[error("No entity with path {}", .0)]
    MissingTarget(Path),
    #[error("Path {} did exist but did not have requested type", .0)]
    InvalidTarget(Path),
}

#[derive(Debug, Clone)]
pub struct FaultValue<T>(pub T, pub Vec<Fault>);

impl<T> From<T> for FaultValue<T> {
    fn from(value: T) -> Self {
        FaultValue(value, vec![])
    }
}
