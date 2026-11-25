use std::sync::LazyLock;

use font8x8::{
    BASIC_FONTS, BLOCK_FONTS, BOX_FONTS, GREEK_FONTS, LATIN_FONTS, MISC_FONTS, UnicodeFonts,
};

const CP437_LOW: &str = "\0☺☻♥♦♣♠•◘○◙♂♀♪♫☼►◄↕‼¶§▬↨↑↓→←∟↔▲▼";
const CP437_HIGH: &str = concat!(
    "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒ",
    "áíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐",
    "└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀",
    "αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■ "
);
const CP850_HIGH: &str = concat!(
    "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜø£Ø×ƒ",
    "áíóúñÑªº¿®¬½¼¡«»░▒▓│┤ÁÂÀ©╣║╗╝¢¥┐",
    "└┴┬├─┼ãÃ╚╔╩╦╠═╬¤ðÐÊËÈıÍÎÏ┘┌█▄¦Ì▀",
    "ÓßÔÒõÕµþÞÚÛÙýÝ¯´­±‗¾¶§÷¸°¨·¹³²■ ",
);

/// OEM character sets available to the built-in DOS console.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CodePage {
    #[default]
    Cp437,
    Cp850,
}

impl CodePage {
    pub const fn from_number(number: u16) -> Option<Self> {
        match number {
            437 => Some(Self::Cp437),
            850 => Some(Self::Cp850),
            _ => None,
        }
    }

    pub const fn number(self) -> u16 {
        match self {
            Self::Cp437 => 437,
            Self::Cp850 => 850,
        }
    }

    const fn high(self) -> &'static str {
        match self {
            Self::Cp437 => CP437_HIGH,
            Self::Cp850 => CP850_HIGH,
        }
    }

    /// Interpret a byte in text video memory, including the low DOS glyphs.
    pub fn character(self, byte: u8) -> char {
        match byte {
            0..32 => CP437_LOW.chars().nth(usize::from(byte)).unwrap(),
            32..127 => char::from(byte),
            127 => '\u{2302}',
            _ => self.high().chars().nth(usize::from(byte - 128)).unwrap(),
        }
    }

    /// Encode input for DOS, preserving ASCII control bytes.
    pub fn encode(self, character: char) -> u8 {
        if character.is_ascii() {
            return character as u8;
        }
        self.high()
            .chars()
            .position(|candidate| candidate == character)
            .map_or(b'?', |index| index as u8 + 128)
    }

    pub fn encode_text(self, text: &str) -> Vec<u8> {
        text.chars()
            .map(|character| self.encode(character))
            .collect()
    }

    /// Decode DOS text, preserving ASCII line endings and control characters.
    pub fn decode_text(self, bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|&byte| {
                if byte.is_ascii() {
                    char::from(byte)
                } else {
                    self.character(byte)
                }
            })
            .collect()
    }

    /// The bitmap glyphs for the active code page, with the low bit on the left.
    pub fn glyphs(self) -> &'static [[u8; 8]; 256] {
        static CP437: LazyLock<[[u8; 8]; 256]> = LazyLock::new(|| build_glyphs(CodePage::Cp437));
        static CP850: LazyLock<[[u8; 8]; 256]> = LazyLock::new(|| build_glyphs(CodePage::Cp850));
        match self {
            Self::Cp437 => &CP437,
            Self::Cp850 => &CP850,
        }
    }
}

fn build_glyphs(page: CodePage) -> [[u8; 8]; 256] {
    std::array::from_fn(|index| {
        let character = page.character(index as u8);
        BASIC_FONTS
            .get(character)
            .or_else(|| LATIN_FONTS.get(character))
            .or_else(|| BOX_FONTS.get(character))
            .or_else(|| BLOCK_FONTS.get(character))
            .or_else(|| GREEK_FONTS.get(character))
            .or_else(|| MISC_FONTS.get(character))
            .or(match character {
                '\u{131}' => Some([0, 0, 0x0e, 0x0c, 0x0c, 0x0c, 0x1e, 0]),
                '\u{b4}' => Some([0x20, 0x10, 0, 0, 0, 0, 0, 0]),
                '\u{2017}' => Some([0, 0, 0, 0, 0, 0xff, 0, 0xff]),
                '\u{b3}' => Some([0x1c, 0x10, 0x18, 0x10, 0x1c, 0, 0, 0]),
                '\u{25a0}' => Some([0, 0x7e, 0x7e, 0x7e, 0x7e, 0x7e, 0x7e, 0]),
                _ => None,
            })
            .unwrap_or_else(|| BASIC_FONTS.get('?').unwrap())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cp850_glyphs_cover_extended_characters() {
        let glyphs = CodePage::Cp850.glyphs();
        let missing: String = (128..=255)
            .filter(|&byte| glyphs[usize::from(byte)] == glyphs[usize::from(b'?')])
            .map(|byte| CodePage::Cp850.character(byte))
            .collect();
        assert!(missing.is_empty(), "missing CP850 glyphs {missing:?}");
    }
}
