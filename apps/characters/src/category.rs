//! Sidebar categories and the characters they contain.

use crate::charinfo::{is_displayable, unicode_name};
use emojis::Group;
use unicode_general_category::{get_general_category, GeneralCategory as Gc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Recent,
    Smileys,
    Animals,
    Food,
    Activities,
    Travel,
    Objects,
    Symbols,
    Flags,
    Punctuation,
    Arrows,
    Bullets,
    Pictures,
    Currency,
    Math,
    Numbers,
    Letters,
}

pub const EMOJI: [Category; 8] = [
    Category::Smileys,
    Category::Animals,
    Category::Food,
    Category::Activities,
    Category::Travel,
    Category::Objects,
    Category::Symbols,
    Category::Flags,
];

pub const LETTERS_AND_SYMBOLS: [Category; 8] = [
    Category::Punctuation,
    Category::Arrows,
    Category::Bullets,
    Category::Pictures,
    Category::Currency,
    Category::Math,
    Category::Numbers,
    Category::Letters,
];

/// Characters that are commonly used as bullets but whose names do not say so.
const EXTRA_BULLETS: &str = "·◦○●◘◙▪▫■□◆◇◉◎★☆✓✔✗✘➢➣➤⦾⦿※";

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::Recent => "Recently Used",
            Category::Smileys => "Smileys & People",
            Category::Animals => "Animals & Nature",
            Category::Food => "Food & Drink",
            Category::Activities => "Activities",
            Category::Travel => "Travel & Places",
            Category::Objects => "Objects",
            Category::Symbols => "Symbols",
            Category::Flags => "Flags",
            Category::Punctuation => "Punctuation",
            Category::Arrows => "Arrows",
            Category::Bullets => "Bullets",
            Category::Pictures => "Pictures",
            Category::Currency => "Currency",
            Category::Math => "Math",
            Category::Numbers => "Numbers",
            Category::Letters => "Letters",
        }
    }

    pub fn icon_name(self) -> Option<&'static str> {
        Some(match self {
            Category::Recent => "document-open-recent-symbolic",
            Category::Smileys => "emoji-people-symbolic",
            Category::Animals => "emoji-nature-symbolic",
            Category::Food => "emoji-food-symbolic",
            Category::Activities => "emoji-activities-symbolic",
            Category::Travel => "emoji-travel-symbolic",
            Category::Objects => "emoji-objects-symbolic",
            Category::Symbols => "emoji-symbols-symbolic",
            Category::Flags => "emoji-flags-symbolic",
            _ => return None,
        })
    }

    fn emoji_groups(self) -> &'static [Group] {
        match self {
            Category::Smileys => &[Group::SmileysAndEmotion, Group::PeopleAndBody],
            Category::Animals => &[Group::AnimalsAndNature],
            Category::Food => &[Group::FoodAndDrink],
            Category::Activities => &[Group::Activities],
            Category::Travel => &[Group::TravelAndPlaces],
            Category::Objects => &[Group::Objects],
            Category::Symbols => &[Group::Symbols],
            Category::Flags => &[Group::Flags],
            _ => &[],
        }
    }

    /// Whether a single displayable code point belongs to this (non-emoji)
    /// category.
    fn contains_char(self, c: char) -> bool {
        let gc = get_general_category(c);
        let is_symbol = matches!(
            gc,
            Gc::MathSymbol | Gc::CurrencySymbol | Gc::ModifierSymbol | Gc::OtherSymbol
        );
        let name_has = |w: &str| unicode_name(c).is_some_and(|n| n.contains(w));
        match self {
            Category::Punctuation => matches!(
                gc,
                Gc::ConnectorPunctuation
                    | Gc::DashPunctuation
                    | Gc::OpenPunctuation
                    | Gc::ClosePunctuation
                    | Gc::InitialPunctuation
                    | Gc::FinalPunctuation
                    | Gc::OtherPunctuation
            ),
            Category::Arrows => is_symbol && name_has("ARROW"),
            Category::Bullets => EXTRA_BULLETS.contains(c) || name_has("BULLET"),
            Category::Pictures => {
                let mut buf = [0; 4];
                let s = &*c.encode_utf8(&mut buf);
                // Exclude emoji-presentation code points (shown under Emoji);
                // text-default ones like ♠ (emoji only with U+FE0F) stay.
                gc == Gc::OtherSymbol
                    && !name_has("ARROW")
                    && !emojis::get(s).is_some_and(|e| e.as_str() == s)
            }
            Category::Currency => gc == Gc::CurrencySymbol,
            Category::Math => gc == Gc::MathSymbol,
            Category::Numbers => {
                matches!(gc, Gc::DecimalNumber | Gc::LetterNumber | Gc::OtherNumber)
            }
            Category::Letters => matches!(
                gc,
                Gc::UppercaseLetter
                    | Gc::LowercaseLetter
                    | Gc::TitlecaseLetter
                    | Gc::ModifierLetter
                    | Gc::OtherLetter
            ),
            _ => false,
        }
    }

    /// The characters of this category, in display order. `Recent` is not
    /// computed here (it comes from settings) and returns an empty list.
    pub fn characters(self) -> Vec<String> {
        let groups = self.emoji_groups();
        if !groups.is_empty() {
            return groups
                .iter()
                .flat_map(|g| g.emojis())
                .map(|e| e.as_str().to_string())
                .collect();
        }
        if self == Category::Recent {
            return Vec::new();
        }
        all_code_points()
            .filter(|&c| self.contains_char(c))
            .map(String::from)
            .collect()
    }
}

/// All displayable code points. Planes 4–13 are unassigned and 15–16 are
/// private use, so only planes 0–3 and 14 are scanned.
pub fn all_code_points() -> impl Iterator<Item = char> {
    (0u32..0x40000)
        .chain(0xE0000..0xE1000)
        .filter_map(char::from_u32)
        .filter(|&c| is_displayable(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(cat: Category, s: &str) -> bool {
        cat.characters().iter().any(|c| c == s)
    }

    #[test]
    fn emoji_groups() {
        assert!(has(Category::Smileys, "😀"));
        assert!(
            has(Category::Smileys, "👍"),
            "people & body merged into smileys"
        );
        assert!(
            !has(Category::Smileys, "👍🏽"),
            "skin tone variants are not listed"
        );
        assert!(has(Category::Animals, "🐶"));
        assert!(has(Category::Food, "🍎"));
        assert!(has(Category::Travel, "🚀"));
        assert!(has(Category::Flags, "🇫🇷"));
        assert_eq!(Category::Smileys.characters()[0], "😀");
    }

    #[test]
    fn symbol_categories() {
        assert!(has(Category::Punctuation, "!"));
        assert!(has(Category::Punctuation, "«"));
        assert!(!has(Category::Punctuation, "a"));
        assert!(has(Category::Arrows, "→"));
        assert!(has(Category::Arrows, "⇒"));
        assert!(!has(Category::Arrows, "+"));
        assert!(has(Category::Bullets, "•"));
        assert!(has(Category::Bullets, "◦"));
        assert!(has(Category::Currency, "€"));
        assert!(has(Category::Currency, "$"));
        assert!(has(Category::Math, "∑"));
        assert!(has(Category::Math, "→"), "categories may overlap");
        assert!(has(Category::Numbers, "½"));
        assert!(has(Category::Letters, "ß"));
        assert!(has(Category::Letters, "一"));
        assert!(has(Category::Pictures, "♠"));
        assert!(has(Category::Pictures, "─"));
        assert!(!has(Category::Pictures, "😀"));
    }

    #[test]
    fn no_invisible() {
        for cat in LETTERS_AND_SYMBOLS {
            for s in cat.characters() {
                let c = s.chars().next().unwrap();
                assert!(is_displayable(c), "{cat:?} contains {:04X}", c as u32);
            }
        }
        assert!(Category::Recent.characters().is_empty());
    }
}
