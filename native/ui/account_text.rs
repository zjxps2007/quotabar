use eframe::egui::TextBuffer;
use std::{any::TypeId, ops::Range};

pub struct AccountField<'a>(pub &'a mut String);

impl TextBuffer for AccountField<'_> {
    fn is_mutable(&self) -> bool {
        true
    }
    fn as_str(&self) -> &str {
        self.0
    }
    fn insert_text(&mut self, text: &str, char_index: usize) -> usize {
        // WPF limits user edits to 100 UTF-16 code units. Count the proposed
        // insertion after selection deletion and leave the existing suffix intact.
        #[cfg(windows)]
        let text = {
            let mut room = 100_usize.saturating_sub(self.0.encode_utf16().count());
            let end = text.char_indices().find(|(_, c)| {
                let Some(rest) = room.checked_sub(c.len_utf16()) else {
                    return true;
                };
                room = rest;
                false
            });
            &text[..end.map_or(text.len(), |(index, _)| index)]
        };
        self.0.insert_text(text, char_index)
    }
    fn delete_char_range(&mut self, chars: Range<usize>) {
        self.0.delete_char_range(chars);
    }
    fn replace_with(&mut self, text: &str) {
        // Undo and redo restore an existing value, including a long imported name.
        text.clone_into(self.0);
    }
    fn type_id(&self) -> TypeId {
        TypeId::of::<AccountField<'static>>()
    }
}

#[cfg(windows)]
pub fn contains_query(value: &str, query: &str) -> bool {
    use quotabar::windows_format::ordinal_uppercase;
    ordinal_uppercase(value).contains(&ordinal_uppercase(query))
}

#[cfg(target_os = "macos")]
pub fn contains_query(value: &str, query: &str) -> bool {
    use objc2_foundation::NSString;
    NSString::from_str(value).localizedCaseInsensitiveContainsString(&NSString::from_str(query))
}

#[cfg(windows)]
pub fn email_matches_alias(email: &str, alias: &str) -> bool {
    use quotabar::windows_format::ordinal_uppercase;
    ordinal_uppercase(email) == ordinal_uppercase(alias)
}

#[cfg(target_os = "macos")]
pub fn email_matches_alias(email: &str, alias: &str) -> bool {
    use objc2_foundation::{NSCharacterSet, NSComparisonResult, NSString};
    let alias = NSString::from_str(alias)
        .stringByTrimmingCharactersInSet(&NSCharacterSet::whitespaceAndNewlineCharacterSet());
    NSString::from_str(email).caseInsensitiveCompare(&alias) == NSComparisonResult::Same
}

#[cfg(all(windows, test))]
mod tests {
    use super::*;

    #[test]
    fn account_input_keeps_suffix_and_never_splits_a_surrogate_pair() {
        let mut value = format!("{}z", "🐈".repeat(49));
        let mut field = AccountField(&mut value);
        assert_eq!(field.insert_text("한🐈", 49), 1);
        assert_eq!(field.insert_text("x", 0), 0);
        assert!(field.as_str().ends_with("한z"));
        field.delete_char_range(0..1);
        assert_eq!(field.insert_text("🐈a", 0), 1);
        let imported = "🐈".repeat(55);
        field.replace_with(&imported);
        assert_eq!(field.as_str(), imported);
        assert_eq!(field.insert_text("x", 0), 0);
        assert!(contains_query("β ς 𐐀", "σ 𐐨"));
        assert!(!email_matches_alias("K@abc", "k@abc"));
        assert!(!email_matches_alias("ſ", "s"));
        assert!(!email_matches_alias(" user ", "user"));
    }
}
