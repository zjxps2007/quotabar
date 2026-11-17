//! Swift's Codable Data uses Foundation's base64 decoder. Keep the decoded backup
//! so rollback compares the original bytes, even for noncanonical legacy encodings.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

pub(super) fn serialize<S: Serializer>(bytes: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
    bytes
        .as_ref()
        .map(|data| STANDARD.encode(data))
        .serialize(s)
}
pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
    Option::<String>::deserialize(d)?
        .map(|s| {
            STANDARD
                .decode(&s)
                .ok()
                .or_else(|| foundation_decode(&s))
                .ok_or_else(|| de::Error::custom("Invalid base64 Data"))
        })
        .transpose()
}
#[cfg(target_os = "macos")]
fn foundation_decode(text: &str) -> Option<Vec<u8>> {
    use objc2::AnyThread;
    use objc2_foundation::{NSData, NSDataBase64DecodingOptions, NSString};
    NSData::initWithBase64EncodedString_options(
        NSData::alloc(),
        &NSString::from_str(text),
        NSDataBase64DecodingOptions::empty(),
    )
    .map(|d| d.to_vec())
}

#[cfg(not(target_os = "macos"))]
fn foundation_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let len = bytes.iter().rposition(|&b| b != b'=').map_or(0, |i| i + 1);
    if len == 0 {
        return (bytes.len() >= 4).then(|| vec![0]);
    }
    let padded = len.next_multiple_of(4);
    let last = bytes.get(padded - 4..padded)?;
    let mut result = STANDARD.decode(&bytes[..padded - 4]).ok()?;
    let slot = |b| if b == b'=' { b'A' } else { b };
    let mut decoded = [0; 3];
    STANDARD
        .decode_slice(
            [last[0], last[1], slot(last[2]), slot(last[3])],
            &mut decoded,
        )
        .ok()?;
    result.push(decoded[0]);
    if last[2] != b'=' {
        result.push(decoded[1]);
    }
    if last[3] != b'=' {
        result.push(decoded[2]);
    }
    Some(result)
}
