use std::backtrace::Backtrace;
use std::borrow::Cow;
use std::fmt::Display;
use std::sync::Arc;

use thiserror::Error;
use winnow::error::{EmptyError, ErrorConvert, FromExternalError, ParserError};
use winnow::stream::Location;

use super::filesystem::Path;
use super::util::{BitPosition, DiscPosition};
use crate::util::{BitInput, InputStream};

#[derive(Debug, Clone, thiserror::Error)]
pub struct ErrorContext {
    offset: (usize, Option<usize>),
    fault: Fault,
    context: Vec<Context>,
}

impl ParserError<InputStream<'_>> for ErrorContext {
    type Inner = Self;

    fn from_input(input: &InputStream<'_>) -> Self {
        let fault = if input.is_empty() {
            Fault::UnexpectedEof
        } else {
            Fault::Unknown(Arc::new(Backtrace::capture()))
        };
        ErrorContext {
            offset: (input.current_token_start(), None),
            fault,
            context: vec![],
        }
    }
    fn into_inner(self) -> winnow::Result<Self::Inner, Self> {
        Ok(self)
    }
}

impl ParserError<BitInput<'_>> for ErrorContext {
    type Inner = Self;

    fn from_input(input: &BitInput<'_>) -> Self {
        let fault = if input.0.is_empty() {
            Fault::UnexpectedEof
        } else {
            Fault::Unknown(Arc::new(Backtrace::capture()))
        };
        ErrorContext {
            offset: (input.0.current_token_start(), Some(input.1)),
            fault,
            context: vec![],
        }
    }
    fn into_inner(self) -> winnow::Result<Self::Inner, Self> {
        Ok(self)
    }
}

impl ErrorConvert<ErrorContext> for ErrorContext {
    fn convert(self) -> ErrorContext {
        self
    }
}

impl FromExternalError<InputStream<'_>, Fault> for ErrorContext {
    fn from_external_error(input: &InputStream<'_>, fault: Fault) -> Self {
        ErrorContext {
            offset: (input.current_token_start(), None),
            fault,
            context: vec![],
        }
    }
}

impl Display for ErrorContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (bytes, maybe_bit) = self.offset;
        if let Some(bit) = maybe_bit {
            writeln!(f, "Error at offset byte {bytes} bit {bit}: {}", self.fault)?;
        } else {
            writeln!(f, "Error at offset {bytes}: {}", self.fault)?;
        }
        for c in &self.context {
            match c {
                Context::Info(s) => writeln!(f, "...while {s}")?,
                Context::Warning { kind, offset } => {
                    if let Some(offset) = offset {
                        writeln!(f, "...in addition, {kind}, at offset {offset}")?
                    } else {
                        writeln!(f, "...in addition, {kind}")?
                    }
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn error_with_fault(
    fault: Fault,
    input: InputStream,
) -> impl FnOnce(EmptyError) -> ErrorContext {
    move |_| ErrorContext {
        offset: (input.current_token_start(), None),
        fault,
        context: vec![],
    }
}

pub(crate) trait ErrExt: Sized {
    fn note(self, str: &'static str) -> Self;
    fn note_dynamic(self, str: &str) -> Self;
    fn warning(self, kind: Warning, offset: Option<usize>) -> Self;
}

impl<T> ErrExt for Result<T, ErrorContext> {
    fn note(self, str: &'static str) -> Self {
        match self {
            ok @ Ok(_) => ok,
            Err(mut e) => {
                e.context.push(Context::Info(str.into()));
                Err(e)
            }
        }
    }
    fn note_dynamic(self, str: &str) -> Self {
        match self {
            ok @ Ok(_) => ok,
            Err(mut e) => {
                e.context.push(Context::Info(str.to_owned().into()));
                Err(e)
            }
        }
    }
    fn warning(self, kind: Warning, offset: Option<usize>) -> Self {
        match self {
            ok @ Ok(_) => ok,
            Err(mut e) => {
                e.context.push(Context::Warning { kind, offset });
                Err(e)
            }
        }
    }
}

impl<T> ErrExt for Result<T, Error> {
    fn note(mut self, str: &'static str) -> Self {
        if let Err(Error::ContextError(e)) = &mut self {
            e.context.push(Context::Info(str.into()));
        }
        self
    }
    fn note_dynamic(mut self, str: &str) -> Self {
        if let Err(Error::ContextError(e)) = &mut self {
            e.context.push(Context::Info(str.to_owned().into()));
        }
        self
    }
    fn warning(mut self, kind: Warning, offset: Option<usize>) -> Self {
        if let Err(Error::ContextError(e)) = &mut self {
            e.context.push(Context::Warning { kind, offset });
        }
        self
    }
}

#[derive(Debug, Clone)]
enum Context {
    Info(Cow<'static, str>),
    Warning {
        kind: Warning,
        offset: Option<usize>,
    },
}

#[derive(Error, Debug, Clone)]
pub enum Warning {
    #[error(
        "Directory {path} began with sequence number 0x{start_seq_num:02X} but ended with 0x{end_seq_num:02X}"
    )]
    SequenceNumberMismatch {
        path: Path,
        start_seq_num: u8,
        end_seq_num: u8,
    },
    #[error("Detected sector size ({}) was too big or small to be plausible", 2usize.pow(*.0 as _))]
    UnacceptableSectorSize(u8),
    #[error("Calculated a zone check value of 0x{actual:02X}, but expected 0x{expected:02X}")]
    ZoneCheckFailure { expected: u8, actual: u8 },
    #[error("Map has a cross check value of 0x{:02X} instead of 0xFF", .0)]
    CrossCheckFailure(u8),
    #[error(
        "Directory {path} has a check_byte value of 0x{expected:02X} but was calculated as 0x{actual:02X}"
    )]
    CheckByteFailure {
        path: Path,
        expected: u8,
        actual: u8,
    },
    #[error(
        "Could not retrieve root directory (root_link={root_link:?}, sector_size={sector_size})"
    )]
    InvalidRoot {
        root_link: DiscPosition,
        sector_size: usize,
    },
    #[error("Directory at {path} failed to parse")]
    InvalidDir { path: Path },
    #[error("File {path} has invalid attribute byte: {attr_value:08b} (at {location:?})")]
    InvalidAttr {
        location: BitPosition,
        path: Path,
        attr_value: u8,
    },
}

#[derive(Error, Debug, Clone)]
pub enum Fault {
    #[error("Unknown Error: (backtrace)\n{}", .0)]
    Unknown(Arc<Backtrace>),
    #[error("File ended early")]
    UnexpectedEof,
    #[error("Free link value 0x{0:04X} did not point at valid fragment")]
    InvalidFreeLink(u16),
    #[error(
        "Free fragment block at offset {origin:?} bits points to offset {dest_bit_offset:?} bits which does not contain a fragment"
    )]
    BrokenFreeChain {
        origin: BitPosition,
        dest_bit_offset: BitPosition,
    },
    #[error(
        "Could not retrieve root directory (root_link={root_link:?}, sector_size={sector_size})"
    )]
    InvalidRoot {
        root_link: DiscPosition,
        sector_size: usize,
    },
    #[error("Directory structure at {path} failed to parse")]
    InvalidDir { path: Path },
    #[error("Expected 'Nick' or 'Hugo', found '{}'", str::escape_debug(&String::from_utf8_lossy(&*.0)))]
    MagicStringFailure([u8; 4]),
    #[error("Detected sector size ({}) was too big or small to be plausible", 2usize.pow(*.0 as _))]
    UnacceptableSectorSize(u8),
}

#[derive(Error, Debug)]
pub enum IoError {
    #[error("Could not find disc object ID {:?}", .0)]
    MissingFragment(DiscPosition),
    #[error("No entry with path {}", .0)]
    MissingTarget(Path),
    #[error("Path {} did exist but did not have requested type", .0)]
    InvalidTarget(Path),
    #[error("Insufficient data to read {length} byes from fragment {}, sector offset {}",  object.fragment(), object.sector_idx())]
    OutOfBounds { object: DiscPosition, length: usize },
    #[error(transparent)]
    StdError(std::io::Error),
}

#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    LookupError(#[from] IoError),
    #[error(transparent)]
    ContextError(#[from] ErrorContext),
}

#[derive(Debug, Clone)]
pub struct FaultValue<T>(pub T, pub Vec<Warning>);

impl<T> From<T> for FaultValue<T> {
    fn from(value: T) -> Self {
        FaultValue(value, vec![])
    }
}
