//! The locale's decimal and digit-group separators, for the decimal key,
//! the display and input. The engine itself always reads `.` as the
//! decimal point (and `,` as a thousands or argument separator), so
//! expressions are converted on the way in ([`Separators::to_engine`]).
//!
//! In a locale with a decimal comma (German, French, …) both `,` and `.`
//! typed or pasted on their own are read as the decimal point, numbers
//! written the locale's way (`1.234,5`, `1 234,5`) and the English way
//! (`1,234.5`) are both understood, and function arguments are separated
//! with `;` (or `, ` with a space). A lone `.` followed by exactly three
//! digits, in a locale that groups with `.` (`1.500` in German), is a
//! group separator, as it would be in a result shown there.

/// Decimal and group separators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Separators {
    pub decimal: char,
    pub group: char,
}

impl Default for Separators {
    fn default() -> Self {
        Separators::C
    }
}

/// Characters that may separate digit groups in pasted numbers.
fn spacing(c: char) -> bool {
    matches!(c, '\u{202f}' | '\u{a0}' | '\u{2009}' | '\'' | '’')
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Role {
    Decimal,
    Group,
    Keep,
}

impl Separators {
    /// The engine's own: `.` and `,`.
    pub const C: Separators = Separators {
        decimal: '.',
        group: ',',
    };

    /// The current locale's (after `i18n::init`).
    pub fn current() -> Separators {
        let (decimal, group) = helix_app_common::i18n::number_separators();
        let group = group
            .filter(|g| *g != decimal)
            .unwrap_or(if decimal == ',' { '\u{202f}' } else { ',' });
        Separators { decimal, group }
    }

    /// Whether the locale writes a decimal comma (or another mark).
    pub fn comma(&self) -> bool {
        self.decimal != '.'
    }

    fn is_sep(&self, c: char) -> bool {
        c == '.' || c == ',' || c == self.decimal || c == self.group || spacing(c)
    }

    /// `input` as the engine reads it, and for each of its characters the
    /// position of the character in `input` it came from (for error
    /// positions).
    pub fn to_engine(self, input: &str) -> (String, Vec<usize>) {
        let chars: Vec<char> = input.chars().collect();
        let mut out = String::with_capacity(input.len());
        let mut map = Vec::with_capacity(chars.len());
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let digit_next = chars.get(i + 1).is_some_and(|d| d.is_ascii_digit());
            let starts = c.is_ascii_digit() || (self.is_sep(c) && digit_next);
            // Digits that continue a name (log10, x1).
            let in_word = starts && i > 0 && (chars[i - 1].is_alphabetic() || chars[i - 1] == '_');
            if !starts || in_word {
                // Inside a name (log10, x1): copy the run as it is.
                let mut j = i + 1;
                if in_word {
                    while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                        j += 1;
                    }
                }
                for (k, ch) in chars.iter().enumerate().take(j).skip(i) {
                    out.push(*ch);
                    map.push(k);
                }
                i = j;
                continue;
            }
            // A number: digits, and separators that are followed by a digit.
            let mut j = i;
            while j < chars.len()
                && (chars[j].is_ascii_digit()
                    || (self.is_sep(chars[j])
                        && chars.get(j + 1).is_some_and(|d| d.is_ascii_digit())))
            {
                j += 1;
            }
            let token = &chars[i..j];
            let roles = self.roles(token);
            for (k, ch) in token.iter().enumerate() {
                match roles[k] {
                    Role::Decimal => out.push('.'),
                    Role::Group => continue,
                    Role::Keep => out.push(*ch),
                }
                map.push(i + k);
            }
            i = j;
        }
        (out, map)
    }

    /// What each character of a number token is.
    fn roles(&self, token: &[char]) -> Vec<Role> {
        let seps: Vec<usize> = (0..token.len())
            .filter(|&k| !token[k].is_ascii_digit())
            .collect();
        let mut roles = vec![Role::Keep; token.len()];
        if seps.is_empty() {
            return roles;
        }
        // Exactly three digits after each separator, and then no digit.
        let grouped = |at: &[usize]| {
            at.iter().all(|&k| {
                let run = token[k + 1..]
                    .iter()
                    .take_while(|c| c.is_ascii_digit())
                    .count();
                run == 3
            })
        };
        let spaced: Vec<usize> = seps
            .iter()
            .copied()
            .filter(|&k| {
                let c = token[k];
                spacing(c) || (c == self.group && c != '.' && c != ',')
            })
            .collect();
        if grouped(&spaced) {
            for &k in &spaced {
                roles[k] = Role::Group;
            }
        }
        let marks: Vec<usize> = seps
            .iter()
            .copied()
            .filter(|&k| token[k] == '.' || token[k] == ',' || token[k] == self.decimal)
            .collect();
        if marks.is_empty() {
            return roles;
        }
        if !self.comma() {
            // The engine reads `.` and `,` itself.
            return roles;
        }
        let has_dot = marks.iter().any(|&k| token[k] == '.');
        let has_comma = marks.iter().any(|&k| token[k] != '.');
        let last = *marks.last().unwrap();
        if has_dot && has_comma {
            // 1.234,5 or 1,234.5: the last mark is the decimal point.
            for &k in &marks {
                roles[k] = if k == last {
                    Role::Decimal
                } else {
                    Role::Group
                };
            }
        } else if marks.len() > 1 {
            // 1.234.567 or 1,234,567: groups (if they are groups).
            if grouped(&marks) {
                for &k in &marks {
                    roles[k] = Role::Group;
                }
            }
        } else if token[last] == self.group && grouped(&[last]) {
            // 1.500 in German: grouped, as a result would be shown.
            roles[last] = Role::Group;
        } else {
            // 0,5 or 0.5: the decimal point.
            roles[last] = Role::Decimal;
        }
        roles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DE: Separators = Separators {
        decimal: ',',
        group: '.',
    };
    const FR: Separators = Separators {
        decimal: ',',
        group: '\u{202f}',
    };

    fn eng(s: &Separators, input: &str) -> String {
        s.to_engine(input).0
    }

    #[test]
    fn dot_locale_is_unchanged() {
        let en = Separators::C;
        for t in [
            "1,234.5+2",
            "fv(100, 0.05, 10)",
            "max(1,2)",
            "log10 100",
            ".5",
        ] {
            assert_eq!(eng(&en, t), t);
        }
        // Pasted thin-space groups are still read.
        assert_eq!(eng(&en, "1\u{202f}234.5"), "1234.5");
    }

    #[test]
    fn comma_locale() {
        assert_eq!(eng(&DE, "1,5+2,25"), "1.5+2.25");
        assert_eq!(eng(&DE, "1.5"), "1.5", "a typed or pasted point");
        assert_eq!(eng(&DE, "1.234,5"), "1234.5");
        assert_eq!(eng(&DE, "1,234.5"), "1234.5", "English, pasted");
        assert_eq!(eng(&DE, "1.234.567"), "1234567");
        assert_eq!(eng(&DE, "1.500"), "1500", "the locale's own grouping");
        assert_eq!(eng(&DE, ",5"), ".5");
        assert_eq!(eng(&FR, "1\u{202f}234,5"), "1234.5");
        assert_eq!(eng(&FR, "1.500"), "1.500", "French does not group with .");
        // Arguments: `;`, or a comma and a space.
        assert_eq!(eng(&DE, "pmt(1000;5%;12)"), "pmt(1000;5%;12)");
        assert_eq!(eng(&DE, "fv(100, 0,05, 10)"), "fv(100, 0.05, 10)");
        // Names with digits are left alone.
        assert_eq!(eng(&DE, "log10 1,5"), "log10 1.5");
    }

    #[test]
    fn positions_map_back() {
        let (out, map) = DE.to_engine("1.234,5+)");
        assert_eq!(out, "1234.5+)");
        assert_eq!(map, vec![0, 2, 3, 4, 5, 6, 7, 8]);
    }
}
