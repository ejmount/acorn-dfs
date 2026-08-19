use std::borrow::Borrow;
use std::ops::Index;

use winnow::Parser;
use winnow::combinator::{opt, preceded, separated, terminated};

use super::FixedLenString;
use crate::util::make_input;

/// Represents a Path on the ADFS filesystem.
///
/// ADFS paths are not necessarily valid UTF-8, so cannot be represented by
/// aggregates of [`String`].
///
/// The default empty value corresponds to the root directory `$`.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Path(Vec<FixedLenString>);

impl Path {
    pub const ROOT_PATH: Path = Path(vec![]);
    pub const ROOT_SYMBOL: u8 = b'$';
    pub const DIR_SEPARATOR: u8 = b'.';
    /// Create a path from a byte-string representing the entire path.
    ///
    /// Paths do not exist in this form anywhere within ADFS disk structures, so
    /// this should not be used to read disk content. Instead, it is used for,
    /// e.g. human-provided input referring to file locations within the ADFS
    /// disk.
    ///
    /// Will return `None` if the provided path is invalid. This can be because
    /// the path is ill-formed, or because a single segment is too long to
    /// actually be encoded on the disk.
    fn from_bytes(input: &[u8]) -> Option<Path> {
        let input = make_input(input);

        let segments_parser = preceded(
            Self::DIR_SEPARATOR,
            separated(
                1..,
                FixedLenString::parse_from_byte_str,
                Self::DIR_SEPARATOR,
            ),
        );
        let segments = preceded(
            Self::ROOT_SYMBOL,
            terminated(opt(segments_parser), opt(Self::DIR_SEPARATOR)),
        )
        .parse(input)
        .ok()?;

        Some(Path(segments.unwrap_or_default()))
    }
    pub(crate) fn is_prefix(&self, other: &Self) -> bool {
        let mut zipper: Vec<_> = self.0.iter().zip(other.0.iter()).collect();
        let last = zipper.pop();
        for (a, b) in zipper {
            if a != b {
                return false;
            }
        }
        if let Some((a, b)) = last {
            a.is_prefix(b)
        } else {
            true
        }
    }

    /// Creates a new Path that appends the given segment to the end of `self`
    pub(crate) fn append(&self, segment: FixedLenString) -> Path {
        let mut segments = self.0.clone();
        segments.push(segment);
        Path(segments)
    }

    /// Creates a Path directly out of a set of segments
    pub(crate) fn from_segments(segments: &[FixedLenString]) -> Path {
        Path(segments.to_vec())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Attempts to construct a Path out of a given string that was, e.g.
    /// provided by a user.
    ///
    /// This is intended for user-facing calling, it
    /// currently does nothing to mitigate UTF-8 not matching the ADFS
    /// character set
    pub fn try_from_str(path: &str) -> Result<Path, String> {
        Path::from_bytes(path.as_bytes()).ok_or(format!("Could not convert '{path}' to ADFS path"))
    }

    /// The path as a sequence of variable-length byte-strings.
    ///
    /// The segments are trimmed to only include valid characters.
    pub fn to_byte_segments(&self) -> Vec<Vec<u8>> {
        let mut output = vec![];
        for segment in &self.0 {
            output.push(segment.valid_range().to_vec());
        }
        output
    }
}

impl Borrow<[FixedLenString]> for Path {
    fn borrow(&self) -> &[FixedLenString] {
        &self.0
    }
}

impl std::fmt::Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", Self::ROOT_SYMBOL as char)?;
        for dir in &self.0 {
            write!(f, "{}{dir}", Self::DIR_SEPARATOR as char)?;
        }
        Ok(())
    }
}

impl<I> Index<I> for Path
where
    I: std::slice::SliceIndex<[FixedLenString]>,
{
    type Output = <I as std::slice::SliceIndex<[FixedLenString]>>::Output;
    fn index(&self, index: I) -> &Self::Output {
        &self.0[index]
    }
}

impl<'a> Extend<&'a Path> for Path {
    fn extend<T: IntoIterator<Item = &'a Path>>(&mut self, iter: T) {
        for p in iter {
            self.0.extend(p.0.iter().copied());
        }
    }
}

#[cfg(test)]
mod test {
    use super::Path;
    use crate::filesystem::FixedLenString;

    #[test]
    fn parse_paths() {
        assert_eq!(Path::from_bytes(b"$"), Some(Path::ROOT_PATH));
        assert_eq!(Path::from_bytes(b"$."), Some(Path::ROOT_PATH));
        assert_eq!(
            Path::from_bytes(b"$.Utilities.!TeleRoute.Templates"),
            Some(Path(vec![
                FixedLenString::from_bytes_dynamic(b"Utilities"),
                FixedLenString::from_bytes_dynamic(b"!TeleRoute"),
                FixedLenString::from_bytes_dynamic(b"Templates"),
            ]))
        );
        assert_eq!(Path::from_bytes(b"$.AAAAAAAAAAAAAAAAAA"), None);
        assert_eq!(
            Path::from_bytes(b"$.AAAA.BBB."),
            Some(Path(vec![
                FixedLenString::from_bytes_dynamic(b"AAAA"),
                FixedLenString::from_bytes_dynamic(b"BBB")
            ]))
        );
        assert_eq!(
            Path::from_bytes(b"$.Utilities.!TeleRoute.Templates")
                .unwrap()
                .to_string(),
            "$.Utilities.!TeleRoute.Templates"
        );
        assert_eq!(Path::from_bytes(b"$.Foo\0o.Bar"), None);
    }

    #[test]
    fn path_as_bytes() {
        let p = Path::from_bytes(b"$.Utilities.!TeleRoute.Templates").unwrap();
        assert_eq!(
            p.to_byte_segments(),
            vec![b"Utilities" as &[u8], b"!TeleRoute", b"Templates"]
        );
    }

    #[test]
    fn prefixes() {
        let cases = [
            ("$", "$.A", true),
            ("$.A", "$.A", true),
            ("$.A", "$.AB", true),
            ("$.A", "$.A.B", true),
            ("$.B", "$.A", false),
            ("$.B.A", "$.A.B", false),
        ];

        for (prefix, haystack, expected) in cases {
            let needle = Path::from_bytes(prefix.as_bytes()).unwrap();
            let path = Path::from_bytes(haystack.as_bytes()).unwrap();
            let test = needle.is_prefix(&path);
            let test = test == expected;
            assert!(test, "'{prefix}' is not a prefix of '{haystack}'");
        }
    }
}
