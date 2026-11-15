#[cfg(any(test, target_os = "macos", feature = "mac-settings-compat"))]
pub(crate) mod keyed;

use std::borrow::Cow;
use zeroize::Zeroizing;

/// Foundation detects UTF-16/32 both with a BOM and from the leading JSON bytes.
/// Keep transcoded login files in a buffer that is cleared after parsing.
pub(crate) enum Utf8<'a> {
    Borrowed(&'a [u8]),
    Transcoded(Zeroizing<String>),
}

impl AsRef<[u8]> for Utf8<'_> {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Borrowed(bytes) => bytes,
            Self::Transcoded(text) => text.as_bytes(),
        }
    }
}

pub(crate) fn utf8(bytes: &[u8]) -> Option<Utf8<'_>> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let (width, little, skip) = match bytes {
        [0xff, 0xfe, 0, 0, ..] => (4, true, 4),
        [0, 0, 0xfe, 0xff, ..] => (4, false, 4),
        [0xff, 0xfe, ..] => (2, true, 2),
        [0xfe, 0xff, ..] => (2, false, 2),
        [0, 0, 0, _, ..] => (4, false, 0),
        [_, 0, 0, 0, ..] => (4, true, 0),
        [0, _, 0, _, ..] => (2, false, 0),
        [_, 0, _, 0, ..] => (2, true, 0),
        _ => return Some(Utf8::Borrowed(bytes)),
    };
    transcode(&bytes[skip..], width, little)
}

pub(crate) fn transcode(bytes: &[u8], width: usize, little: bool) -> Option<Utf8<'_>> {
    if !bytes.len().is_multiple_of(width) {
        return None;
    }
    let mut decoded = Zeroizing::new(String::new());
    if width == 2 {
        let units = bytes.as_chunks::<2>().0.iter().map(|s| {
            if little {
                u16::from_le_bytes(*s)
            } else {
                u16::from_be_bytes(*s)
            }
        });
        for ch in char::decode_utf16(units) {
            decoded.push(ch.ok()?);
        }
    } else {
        for &unit in bytes.as_chunks::<4>().0 {
            let value = if little {
                u32::from_le_bytes(unit)
            } else {
                u32::from_be_bytes(unit)
            };
            decoded.push(char::from_u32(value)?);
        }
    }
    Some(Utf8::Transcoded(decoded))
}

// Foundation accepts a trailing comma after an element or keyed value.
// Keep quoted strings and malformed empty/multiple commas intact.
pub(crate) fn trailing_commas(bytes: &[u8]) -> Cow<'_, [u8]> {
    let mut result = Cow::Borrowed(bytes);
    let (mut quoted, mut escaped, mut previous) = (false, false, 0);
    for (i, &b) in bytes.iter().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                quoted = false;
                previous = b;
            }
        } else if b == b'"' {
            quoted = true;
        } else if !matches!(b, b' ' | b'\n' | b'\r' | b'\t') {
            if b == b','
                && !matches!(previous, 0 | b'[' | b'{' | b':' | b',')
                && matches!(
                    bytes[i + 1..]
                        .iter()
                        .find(|&&c| !matches!(c, b' ' | b'\n' | b'\r' | b'\t')),
                    Some(b']' | b'}')
                )
            {
                result.to_mut()[i] = b' ';
            }
            previous = b;
        }
    }
    result
}
