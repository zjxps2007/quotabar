pub(crate) use super::trailing_commas;
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{MapAccess, Visitor},
};
use serde_json::value::RawValue;
use std::{collections::BTreeMap, fmt};

pub(crate) fn decoder_utf8(data: &[u8]) -> Option<super::Utf8<'_>> {
    // JSONDecoder in the supported Foundation releases checks FE FF 00 00
    // for UTF-32LE before checking 16-bit marks. JSONSerialization, used for
    // login files, has a separate byte-order detector.
    match data {
        [0xfe, 0xff, 0, 0, rest @ ..] => super::transcode(rest, 4, true),
        [0xff, 0xfe, 0, 0, ..] => super::transcode(&data[2..], 2, true),
        [0xef, 0xbb, 0xbf, rest @ ..] => Some(super::Utf8::Borrowed(rest)),
        bytes => super::utf8(bytes),
    }
}

// Foundation's keyed containers retain the first value for a duplicate key.
// Keep the number lexemes intact so Date and Int use their own conversion rules.
#[derive(Serialize)]
pub(crate) struct Object<'a>(pub(crate) BTreeMap<String, &'a RawValue>);
impl<'de> Deserialize<'de> for Object<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Keyed;
        impl<'de> Visitor<'de> for Keyed {
            type Value = Object<'de>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a keyed container")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut fields = BTreeMap::new();
                while let Some((key, value)) = map.next_entry()? {
                    fields.entry(key).or_insert(value);
                }
                Ok(Object(fields))
            }
        }
        d.deserialize_map(Keyed)
    }
}
