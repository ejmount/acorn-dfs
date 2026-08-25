use std::cmp::Ordering;
use std::fmt::{Debug, Display};
use std::hash::{Hash, Hasher};

use serde::Serialize;
use winnow::Parser;
use winnow::combinator::trace;
use winnow::stream::Stream;
use winnow::token::take;

use crate::util::{InputStream, ParseResult};

/// A string of bytes of fixed length, used for various types of data such as
/// file/directory names.
///
/// While this data has a fixed length when stored within disk structures,
/// semantically it is of variable length, with *any* control character
/// representing a terminator.
#[derive(Clone, Copy, Eq)]
pub struct FixedLenString<const LEN: usize = 10>([u8; LEN]);
impl<const LEN: usize> FixedLenString<LEN> {
    // https://www.riscos.com/support/users/firststeps/chap08.htm#L0058
    const FORBIDDEN_CHARS: &[u8] = b"$%&*#@\"|^.:";

    /// For easily constructing an FLS for testing. Similar to
    /// `parse_from_byte_str` but ignores control character considerations.
    pub fn from_bytes_dynamic(input: &[u8]) -> FixedLenString<LEN> {
        let len = input.len().min(LEN);
        let mut output = [0; LEN];
        output[..len].copy_from_slice(&input[..len]);
        FixedLenString(output)
    }

    /// Read this value from text, such as a user-provided
    /// [`super::sys_structures::Path`].
    ///
    /// This *does* stop early should a terminator be encountered, unlike
    /// `parse_from_disk` and will reject empty components. This is
    /// load-bearing for rejecting invalid [`super::sys_structures::Path`]
    /// values correctly.
    pub fn parse_from_byte_str<'a>(
        input: &mut InputStream<'a>,
    ) -> ParseResult<FixedLenString<LEN>> {
        let next_end_char = input
            .offset_for(|c| c.is_ascii_control() || Self::FORBIDDEN_CHARS.contains(&c))
            .unwrap_or(input.len());

        let length = [LEN, next_end_char, input.len()].into_iter().min().unwrap();

        let mut output = [0; LEN];
        let data = trace(
            format!("FixedLenString (dynamic, up to {length}"),
            take(length).verify(|b: &[u8]| !b.is_empty()),
        )
        .parse_next(input)?;
        output[..length].copy_from_slice(data);

        Ok(FixedLenString(output))
    }
    /// Read a value from a disk image, which reads (and advances the stream by)
    /// exactly `LEN` bytes,
    pub fn parse_from_disk<'a>(input: &mut InputStream<'a>) -> ParseResult<Self> {
        // Unlike other methods, this will accept an empty string (containing an initial
        // control character)
        trace(
            format!("FixedString {LEN}"),
            |input: &mut InputStream<'a>| {
                let o = *take(LEN).parse_next(input)?.first_chunk().unwrap();
                Ok(FixedLenString(o))
            },
        )
        .map(Into::into)
        .parse_next(input)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn len(&self) -> usize {
        self.0
            .iter()
            .position(|&u| (u as char).is_control())
            .unwrap_or(self.0.len())
    }

    pub fn is_prefix(&self, other: &Self) -> bool {
        if self.len() > other.len() {
            return false;
        };
        self.valid_range()
            .iter()
            .zip(other.valid_range())
            .all(|(&i, &j)| i == j)
    }

    /// The segment of this string that represents valid usable data
    pub fn valid_range(&self) -> &[u8] {
        let idx = self.len();
        &self.0[..idx]
    }
}
impl<const N: usize> Debug for FixedLenString<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FixedLenString({:?})", String::from_utf8_lossy(&self.0))
    }
}
impl<const N: usize> Display for FixedLenString<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let lossy = String::from_utf8_lossy(self.valid_range());
        write!(f, "{}", str::escape_default(&lossy))
    }
}
impl<const LEN: usize> Hash for FixedLenString<LEN> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write(self.valid_range());
    }
}

impl<const LEN: usize> PartialEq for FixedLenString<LEN> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl<const LEN: usize> PartialOrd for FixedLenString<LEN> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<const LEN: usize> Ord for FixedLenString<LEN> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let self_valid = self.valid_range();
        let other_valid = other.valid_range();

        self_valid.cmp(other_valid)
    }
}
impl<const N: usize> Serialize for FixedLenString<N> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_seq(&self.0)
    }
}

#[cfg(test)]
mod test {
    use std::cmp::Ordering;

    use super::FixedLenString;

    #[test]
    fn fixed_string_properties() {
        let empty = FixedLenString([0; 10]);
        assert!(empty.valid_range().is_empty());

        let a = FixedLenString([b'a', b'b', 0]);
        let b = FixedLenString([b'a', b'b', b'c']);
        assert_eq!(a.cmp(&b), Ordering::Less);
        assert_eq!(a.len(), 2);
        assert_eq!(b.len(), 3);

        let c = FixedLenString([0]);
        let d = FixedLenString([1]);
        assert_eq!(c, d);
    }
}
