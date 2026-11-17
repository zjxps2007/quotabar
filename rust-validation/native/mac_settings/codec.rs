use super::{Error, Provider, Settings};
use crate::foundation_json::keyed::{Object, decoder_utf8, trailing_commas};
use serde::{Deserialize, Deserializer, de};
use serde_json::value::RawValue;

pub(super) fn provider<'de, D: Deserializer<'de>>(d: D) -> Result<Provider, D::Error> {
    let name = String::deserialize(d)?;
    match name.as_str() {
        "codex" => Ok(Provider::Codex),
        "claude" => Ok(Provider::Claude),
        "gemini" => Ok(Provider::Gemini),
        _ => Err(de::Error::custom("Unknown account provider")),
    }
}

// JSONDecoder also accepts integral decimal and exponent forms for a Swift Int.
// Floating-point precision is insufficient for the upper end of the Int64 range.
fn integer_value(raw: &RawValue) -> Option<i64> {
    let text = raw.get();
    if let Ok(int) = text.parse() {
        return Some(int);
    }
    let number: f64 = text.parse().ok()?;
    let limit = (1u64 << 63) as f64;
    if !(-limit..limit).contains(&number) || number.fract() != 0.0 {
        return None;
    }
    if number.abs() < (1u64 << 53) as f64 {
        return Some(number as i64);
    }
    let (mantissa, exponent) = match text.split_once(['E', 'e']) {
        Some((m, e)) => (m, e.parse::<i32>().ok()?),
        None => (text, 0),
    };
    let (whole, fraction) = mantissa
        .trim_start_matches('-')
        .split_once('.')
        .unwrap_or((mantissa.trim_start_matches('-'), ""));
    let digits = format!("{whole}{fraction}");
    let digits = digits.trim_start_matches('0');
    // Foundation compacts Decimal, then scales an unsigned 64-bit significand
    // before converting its magnitude to a signed Int and applying the sign.
    let compact = digits.trim_end_matches('0');
    let trailing = i32::try_from(digits.len() - compact.len()).ok()?;
    let scale = exponent
        .checked_sub(i32::try_from(fraction.len()).ok()?)?
        .checked_add(trailing)?;
    let significand: u64 = compact.parse().ok()?;
    let power = 10u64.checked_pow(scale.unsigned_abs())?;
    let magnitude = if scale < 0 {
        significand / power
    } else {
        significand.checked_mul(power)?
    };
    let value = i64::try_from(magnitude).ok()?;
    Some(if text.starts_with('-') { -value } else { value })
}

pub(super) fn integer<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    integer_value(<&RawValue>::deserialize(d)?).ok_or_else(|| de::Error::custom("Invalid Int"))
}
pub(super) fn optional_integer<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Option::<&RawValue>::deserialize(d)?
        .map(|v| integer_value(v).ok_or_else(|| de::Error::custom("Invalid Int")))
        .transpose()
}

pub(super) fn decode(data: &[u8]) -> Result<Settings, Error> {
    let utf8 = decoder_utf8(data).ok_or(Error::Json)?;
    let bytes = trailing_commas(utf8.as_ref());
    let Object(mut fields) = serde_json::from_slice(&bytes)?;
    let accounts = fields.remove("accounts").ok_or(Error::Json)?;
    let accounts: Vec<Object<'_>> = serde_json::from_str(accounts.get())?;
    let accounts = serde_json::value::to_raw_value(&accounts)?;
    fields.insert("accounts".to_owned(), &accounts);
    Ok(serde_json::from_slice(&serde_json::to_vec(&fields)?)?)
}
