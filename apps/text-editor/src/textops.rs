//! Small text helpers: indentation, columns, "go to line" parsing.

/// Indentation preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Indent {
    pub tab_width: u32,
    pub use_spaces: bool,
}

impl Indent {
    /// One indent level inserted at visual column `col`: a tab, or spaces up
    /// to the next tab stop.
    pub fn unit_at(&self, col: u32) -> String {
        if self.use_spaces {
            let w = self.tab_width.max(1);
            " ".repeat((w - col % w) as usize)
        } else {
            "\t".into()
        }
    }

    /// A full indent level (independent of position).
    pub fn unit(&self) -> String {
        self.unit_at(0)
    }
}

/// Leading whitespace of `line`.
pub fn leading_whitespace(line: &str) -> &str {
    let end = line.find(|c: char| c != ' ' && c != '\t').unwrap_or(line.len());
    &line[..end]
}

/// Indentation for a new line inserted after `before_cursor` (the text of the
/// current line up to the cursor): copy its leading whitespace, plus one level
/// after an opening bracket.
pub fn newline_indent(before_cursor: &str, indent: &Indent) -> String {
    let mut ws = leading_whitespace(before_cursor).to_string();
    if matches!(before_cursor.trim_end().chars().last(), Some('{' | '[' | '(')) {
        ws.push_str(&indent.unit());
    }
    ws
}

/// Visual column (0-based) at the end of `prefix`, expanding tabs.
pub fn visual_column(prefix: &str, tab_width: u32) -> u32 {
    let w = tab_width.max(1);
    prefix.chars().fold(0, |col, c| if c == '\t' { col + w - col % w } else { col + 1 })
}

/// Width of an indentation string in columns.
fn indent_width(ws: &str, tab_width: u32) -> u32 {
    visual_column(ws, tab_width)
}

/// Adds one indent level to the start of `line` (empty lines unchanged).
pub fn indent_line(line: &str, indent: &Indent) -> String {
    if line.trim().is_empty() {
        return line.to_string();
    }
    format!("{}{}", indent.unit(), line)
}

/// Removes up to one indent level from the start of `line`.
pub fn unindent_line(line: &str, indent: &Indent) -> String {
    let ws = leading_whitespace(line);
    let w = indent_width(ws, indent.tab_width);
    if w == 0 {
        return line.to_string();
    }
    let unit = indent.tab_width.max(1);
    let target = (w - 1) / unit * unit;
    // Drop leading whitespace characters until the width is <= target.
    let mut kept = String::new();
    let mut col = 0;
    for c in ws.chars() {
        let next = if c == '\t' { col + unit - col % unit } else { col + 1 };
        if next > target {
            break;
        }
        kept.push(c);
        col = next;
    }
    if col < target {
        kept.push_str(&" ".repeat((target - col) as usize));
    }
    format!("{}{}", kept, &line[ws.len()..])
}

/// Parses `LINE` or `LINE:COLUMN` (1-based). Returns 1-based values.
pub fn parse_goto(s: &str) -> Option<(u32, Option<u32>)> {
    let s = s.trim();
    let (l, c) = match s.split_once([':', ',']) {
        Some((l, c)) => (l.trim(), Some(c.trim())),
        None => (s, None),
    };
    let line = l.parse::<u32>().ok().filter(|&n| n > 0)?;
    let col = match c {
        Some(c) if !c.is_empty() => Some(c.parse::<u32>().ok().filter(|&n| n > 0)?),
        _ => None,
    };
    Some((line, col))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPACES4: Indent = Indent { tab_width: 4, use_spaces: true };
    const TABS: Indent = Indent { tab_width: 8, use_spaces: false };

    #[test]
    fn units() {
        assert_eq!(SPACES4.unit(), "    ");
        assert_eq!(SPACES4.unit_at(2), "  ");
        assert_eq!(SPACES4.unit_at(4), "    ");
        assert_eq!(TABS.unit_at(3), "\t");
    }

    #[test]
    fn newline_indent_copies_whitespace() {
        assert_eq!(newline_indent("    foo", &SPACES4), "    ");
        assert_eq!(newline_indent("\t\tbar", &TABS), "\t\t");
        assert_eq!(newline_indent("plain", &SPACES4), "");
        assert_eq!(newline_indent("   ", &SPACES4), "   ");
    }

    #[test]
    fn newline_indent_after_bracket() {
        assert_eq!(newline_indent("  if x {", &SPACES4), "      ");
        assert_eq!(newline_indent("call(  ", &TABS), "\t");
        assert_eq!(newline_indent("\tlist = [", &TABS), "\t\t");
        assert_eq!(newline_indent("done }", &SPACES4), "");
    }

    #[test]
    fn columns() {
        assert_eq!(visual_column("", 8), 0);
        assert_eq!(visual_column("abc", 8), 3);
        assert_eq!(visual_column("\t", 8), 8);
        assert_eq!(visual_column("ab\tc", 4), 5);
        assert_eq!(visual_column("é\t", 4), 4);
    }

    #[test]
    fn indent_unindent() {
        assert_eq!(indent_line("x", &SPACES4), "    x");
        assert_eq!(indent_line("", &SPACES4), "");
        assert_eq!(indent_line("y", &TABS), "\ty");
        assert_eq!(unindent_line("      x", &SPACES4), "    x");
        assert_eq!(unindent_line("    x", &SPACES4), "x");
        assert_eq!(unindent_line("  x", &SPACES4), "x");
        assert_eq!(unindent_line("\t\tx", &TABS), "\tx");
        assert_eq!(unindent_line("x", &TABS), "x");
    }

    #[test]
    fn goto() {
        assert_eq!(parse_goto("12"), Some((12, None)));
        assert_eq!(parse_goto(" 3:7 "), Some((3, Some(7))));
        assert_eq!(parse_goto("3,"), Some((3, None)));
        assert_eq!(parse_goto("0"), None);
        assert_eq!(parse_goto("abc"), None);
        assert_eq!(parse_goto("4:x"), None);
    }
}
