use core_foundation::{
    base::{CFRange, TCFType},
    string::{CFString, CFStringRef},
};
use core_text::font::{self, CTFont, CTFontRef};
use eframe::egui::FontData;

pub fn load(family: &str, semibold: bool) -> Option<FontData> {
    let system = font::new_ui_font_for_language(
        if semibold {
            font::kCTFontEmphasizedSystemFontType
        } else {
            font::kCTFontSystemFontType
        },
        13.,
        None,
    );
    match family {
        "Apple SD Gothic Neo" => {
            let name = if semibold {
                "AppleSDGothicNeo-SemiBold"
            } else {
                "AppleSDGothicNeo-Regular"
            };
            font::new_from_name(name, 13.)
                .ok()
                .and_then(|f| data_for(&f, '한'))
                .or_else(|| data_for(&fallback_for_hangul(&system)?, '한'))
        }
        ".AppleSystemUIFont" => data_for(&system, 'A').or_else(|| {
            let name = if semibold {
                "HelveticaNeue-Bold"
            } else {
                "HelveticaNeue"
            };
            data_for(&font::new_from_name(name, 13.).ok()?, 'A')
        }),
        _ => data_for(&font::new_from_name(family, 13.).ok()?, '0'),
    }
}

fn fallback_for_hangul(system: &CTFont) -> Option<CTFont> {
    unsafe extern "C" {
        fn CTFontCreateForString(
            current: CTFontRef,
            text: CFStringRef,
            range: CFRange,
        ) -> CTFontRef;
    }
    let text = CFString::new("한");
    // The single Hangul syllable spans one UTF-16 code unit. Core Text returns
    // a retained font from the same fallback cascade AppKit uses for this text.
    let owned = unsafe {
        CTFontCreateForString(
            system.as_concrete_TypeRef(),
            text.as_concrete_TypeRef(),
            CFRange::init(0, 1),
        )
    };
    (!owned.is_null()).then(|| unsafe { CTFont::wrap_under_create_rule(owned) })
}

fn data_for(font: &CTFont, sample: char) -> Option<FontData> {
    let usable = |face: &ttf_parser::Face<'_>| {
        let glyph = face.glyph_index(sample)?;
        // LastResort maps unrelated syllables to the same missing-glyph box.
        if sample == '한' && face.glyph_index('글')? == glyph {
            return None;
        }
        Some(())
    };
    let from_file = || {
        let bytes = std::fs::read(font.url()?.to_path()?).ok()?;
        let index = if let Some(count) = ttf_parser::fonts_in_collection(&bytes) {
            let postscript = font.postscript_name();
            (0..count).find(|&index| {
                ttf_parser::Face::parse(&bytes, index).is_ok_and(|face| {
                    face.glyph_index(sample).is_some()
                        && face.names().into_iter().any(|name| {
                            name.name_id == ttf_parser::name_id::POST_SCRIPT_NAME
                                && (name.name == postscript.as_bytes()
                                    || name.to_string().as_deref() == Some(postscript.as_str()))
                        })
                })
            })?
        } else {
            0
        };
        usable(&ttf_parser::Face::parse(&bytes, index).ok()?)?;
        let mut data = FontData::from_owned(bytes);
        data.index = index;
        Some(data)
    };
    from_file().or_else(|| {
        // Asset-backed system faces may expose no file or collection index.
        // Core Text still supplies the selected face's OpenType tables in memory.
        let bytes = standalone_font(font)?;
        usable(&ttf_parser::Face::parse(&bytes, 0).ok()?)?;
        Some(FontData::from_owned(bytes))
    })
}

fn standalone_font(font: &CTFont) -> Option<Vec<u8>> {
    let mut tables: Vec<_> = font
        .get_available_font_tables()?
        .iter()
        .filter_map(|tag| Some((*tag, font.get_font_table(*tag)?)))
        .collect();
    tables.sort_unstable_by_key(|(tag, _)| *tag);
    let count = u16::try_from(tables.len()).ok()?;
    if count == 0 || count > 4095 {
        return None;
    }
    let cff = tables
        .iter()
        .any(|(tag, _)| matches!(&tag.to_be_bytes(), b"CFF " | b"CFF2"));
    let mut bytes = Vec::new();
    bytes.extend_from_slice(if cff { b"OTTO" } else { b"\0\x01\0\0" });
    let power = count.ilog2() as u16;
    let range = 16 << power;
    for value in [count, range, power, count * 16 - range] {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    bytes.resize(12 + usize::from(count) * 16, 0);
    let checksum = |words: &[u8]| {
        words.as_chunks::<4>().0.iter().fold(0_u32, |sum, word| {
            sum.wrapping_add(u32::from_be_bytes(*word))
        })
    };
    let mut adjustment = None;
    for (i, (tag, table)) in tables.into_iter().enumerate() {
        let offset = bytes.len();
        bytes.extend_from_slice(table.bytes());
        if tag.to_be_bytes() == *b"head" {
            if table.len() < 12 {
                return None;
            }
            bytes[offset + 8..offset + 12].fill(0);
            adjustment = Some(offset + 8);
        }
        bytes.resize(bytes.len().next_multiple_of(4), 0);
        let entry = [
            tag,
            checksum(&bytes[offset..]),
            u32::try_from(offset).ok()?,
            u32::try_from(table.len()).ok()?,
        ];
        for (j, value) in entry.into_iter().enumerate() {
            let start = 12 + i * 16 + j * 4;
            bytes[start..start + 4].copy_from_slice(&value.to_be_bytes());
        }
    }
    let start = adjustment?;
    let total = 0xB1B0_AFBA_u32.wrapping_sub(checksum(&bytes));
    bytes[start..start + 4].copy_from_slice(&total.to_be_bytes());
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_font_cascade_draws_dashboard_labels() {
        for semibold in [false, true] {
            let data = load("Apple SD Gothic Neo", semibold).expect("system Hangul face");
            let face = ttf_parser::Face::parse(&data.font, data.index).unwrap();
            let mut glyphs = std::collections::HashSet::new();
            for text in "계정연결새로고침".chars() {
                glyphs.insert(face.glyph_index(text).expect("missing Hangul glyph"));
            }
            assert_eq!(glyphs.len(), 8);
        }
    }
}
