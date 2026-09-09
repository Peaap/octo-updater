// Game-language (WoW.exe locale) patch. Ported byte-for-byte from the legacy
// Python updater's `locale_patches`. The language is switched by patching
// three spots in a pristine 1.12.1-family WoW.exe:
//   1. an assert-instruction opcode/operand pair at _LOCALE_TAG_OFFSET,
//   2. the locale slot index + a short jump at _LOCALE_INDEX_OFFSET,
//   3. the chosen slot's 8-byte locale-name string at _LOCALE_NAME_OFFSET(idx).
// Offsets and bytes were verified by the original author against a pristine
// base-WoW.exe; they are reproduced here unmodified.

const LOCALE_TAG_OFFSET: usize = 0x1b2115;
const LOCALE_INDEX_OFFSET: usize = 0x253c;
const LOCALE_NAME_TABLE_BASE: usize = 0x45591c;
const LOCALE_NAME_SLOT_SIZE: usize = 8;

/// The pristine (unpatched) WoW.exe carries 0xa1 at `LOCALE_TAG_OFFSET`; a
/// patched one carries 0xb8. Used to detect whether a cached/on-disk exe is
/// safe to treat as a clean base for re-patching.
pub const PRISTINE_ASSERT_BYTE: u8 = 0xa1;
pub const PATCHED_ASSERT_BYTE: u8 = 0xb8;

const LOCALE_NAMES: [&str; 8] = [
    "enUS", "koKR", "frFR", "deDE", "zhCN", "zhTW", "esES", "xxYY",
];

pub const DEFAULT_LOCALE: &str = "enUS";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Locale {
    pub code: &'static str,
    pub slot_index: usize,
    pub display_label: &'static str,
}

// selectable code -> (slot index, display label), in menu order. ruRU and
// ptBR reuse the unused zhTW / xxYY slots and rename them at patch time.
pub const LOCALES: [Locale; 6] = [
    Locale {
        code: "enUS",
        slot_index: 0,
        display_label: "English",
    },
    Locale {
        code: "deDE",
        slot_index: 3,
        display_label: "Deutsch",
    },
    Locale {
        code: "ruRU",
        slot_index: 5,
        display_label: "Русский",
    },
    Locale {
        code: "zhCN",
        slot_index: 4,
        display_label: "中文 (简体)",
    },
    Locale {
        code: "esES",
        slot_index: 6,
        display_label: "Español",
    },
    Locale {
        code: "ptBR",
        slot_index: 7,
        display_label: "Português (BR)",
    },
];

pub fn find_locale(code: &str) -> Locale {
    LOCALES
        .iter()
        .copied()
        .find(|locale| locale.code == code)
        .unwrap_or_else(|| {
            LOCALES
                .iter()
                .copied()
                .find(|locale| locale.code == DEFAULT_LOCALE)
                .unwrap()
        })
}

/// The three WoW.exe byte edits that force the given game language, as
/// (offset, bytes) pairs applied on top of a pristine base.
pub fn locale_patches(code: &str) -> Vec<(usize, Vec<u8>)> {
    let locale = find_locale(code);
    let carrier = LOCALE_NAMES[locale.slot_index];
    let mut carrier_reversed: Vec<u8> = carrier.bytes().collect();
    carrier_reversed.reverse();

    let mut assert_patch = vec![PATCHED_ASSERT_BYTE];
    assert_patch.extend_from_slice(&carrier_reversed);

    let index_patch = vec![0xbe, locale.slot_index as u8, 0x00, 0x00, 0x00, 0xeb, 0x1f];

    let name_offset = LOCALE_NAME_TABLE_BASE - locale.slot_index * LOCALE_NAME_SLOT_SIZE;
    let name_patch = locale.code.as_bytes().to_vec();

    vec![
        (LOCALE_TAG_OFFSET, assert_patch),
        (LOCALE_INDEX_OFFSET, index_patch),
        (name_offset, name_patch),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_locale_patches_enus_slot_zero() {
        let patches = locale_patches("enUS");
        assert_eq!(patches.len(), 3);
        assert_eq!(patches[0].0, LOCALE_TAG_OFFSET);
        assert_eq!(patches[0].1[0], PATCHED_ASSERT_BYTE);
        assert_eq!(patches[1].1[1], 0); // slot index 0
        assert_eq!(patches[2].1, b"enUS".to_vec());
    }

    #[test]
    fn rudu_reuses_the_zhtw_slot() {
        let patches = locale_patches("ruRU");
        // slot 5 (zhTW's index) is written with "ruRU" bytes
        assert_eq!(patches[1].1[1], 5);
        assert_eq!(patches[2].1, b"ruRU".to_vec());
    }

    #[test]
    fn unknown_locale_falls_back_to_default() {
        let default_patches = locale_patches(DEFAULT_LOCALE);
        let unknown_patches = locale_patches("xxXX");
        assert_eq!(default_patches[1].1, unknown_patches[1].1);
    }
}
