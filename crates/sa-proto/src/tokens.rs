//! estimating how many tokens a piece of text costs.
//!
//! the figure is an estimate, not a tokeniser: full-width scripts run near one token per
//! character and everything else near four characters to the token. it is used wherever a
//! count has to be shown before a provider has counted anything itself.

/// the characters that tokenise near one token per character.
const FULL_WIDTH: [core::ops::RangeInclusive<u32>; 5] = [
    0x3040..=0x30ff,
    0x3400..=0x9fff,
    0xac00..=0xd7af,
    0xf900..=0xfaff,
    0xff00..=0xffef,
];

/// characters to the token for text that is not full-width.
const CHARS_PER_TOKEN: f32 = 4.0;

/// the estimated tokens in a piece of text.
#[must_use]
pub fn estimate(text: &str) -> f32 {
    let mut wide = 0.0;
    let mut narrow = 0.0;
    for character in text.chars() {
        let code = u32::from(character);
        if FULL_WIDTH.iter().any(|range| range.contains(&code)) {
            wide += 1.0;
        } else {
            narrow += 1.0;
        }
    }
    wide + narrow / CHARS_PER_TOKEN
}

/// the same figure rounded up to whole tokens, for display.
#[must_use]
pub fn estimate_rounded(text: &str) -> u64 {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let tokens = estimate(text).ceil().max(0.0) as u64;
    tokens
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn narrow_text_is_about_four_characters_a_token() {
        assert_eq!(estimate("abcd"), 1.0);
        assert_eq!(estimate(""), 0.0);
    }

    #[test]
    fn full_width_text_is_about_one_token_a_character() {
        assert_eq!(estimate("漢字"), 2.0);
    }

    #[test]
    fn rounding_is_whole_tokens() {
        assert_eq!(estimate_rounded("abcdefgh"), 2);
    }
}
