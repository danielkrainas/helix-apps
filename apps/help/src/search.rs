//! Full-text search over a document's pages.

/// Searchable text of one page.
#[derive(Debug, Clone, Default)]
pub struct Entry {
    pub page: Option<String>,
    pub title: String,
    pub desc: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub index: usize,
    pub score: u32,
}

pub fn terms(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in query
        .split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-')
        .map(str::to_lowercase)
    {
        let t = t.trim_matches(|c| c == '\'' || c == '-').to_string();
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

fn count(hay: &str, needle: &str) -> u32 {
    hay.matches(needle).count() as u32
}

/// Ranks entries for `query`. Every term must appear somewhere in the
/// page. Score: phrase in title +20, each term in title +10, in
/// description +4, in body +1 per occurrence (max 5 per term).
pub fn rank(entries: &[Entry], query: &str) -> Vec<Hit> {
    let terms = terms(query);
    if terms.is_empty() {
        return Vec::new();
    }
    let phrase = terms.join(" ");
    let mut hits: Vec<Hit> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, e)| {
            let title = e.title.to_lowercase();
            let desc = e.desc.to_lowercase();
            let body = e.body.to_lowercase();
            let mut score = 0;
            if terms.len() > 1 && title.contains(&phrase) {
                score += 20;
            }
            for t in &terms {
                let in_title = title.contains(t.as_str());
                let in_desc = desc.contains(t.as_str());
                let body_n = count(&body, t).min(5);
                if !in_title && !in_desc && body_n == 0 {
                    return None;
                }
                score += if in_title { 10 } else { 0 } + if in_desc { 4 } else { 0 } + body_n;
            }
            Some(Hit { index, score })
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score.cmp(&a.score).then_with(|| {
            entries[a.index]
                .title
                .to_lowercase()
                .cmp(&entries[b.index].title.to_lowercase())
        })
    });
    hits
}

/// A short excerpt of `body` around the first matching term.
pub fn snippet(body: &str, query: &str, max_chars: usize) -> String {
    let terms = terms(query);
    let lower = body.to_lowercase();
    // Lower-casing can change byte lengths; only trust positions when it doesn't.
    let pos = if lower.len() == body.len() {
        terms
            .iter()
            .filter_map(|t| lower.find(t.as_str()))
            .min()
            .unwrap_or(0)
    } else {
        0
    };
    let chars: Vec<(usize, char)> = body.char_indices().collect();
    let ci = chars.iter().position(|(b, _)| *b >= pos).unwrap_or(0);
    let start = ci.saturating_sub(max_chars / 3);
    let end = (start + max_chars).min(chars.len());
    let mut s: String = chars[start..end].iter().map(|(_, c)| c).collect();
    if start > 0 {
        s = format!("\u{2026}{}", s.trim_start());
    }
    if end < chars.len() {
        s = format!("{}\u{2026}", s.trim_end());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(title: &str, desc: &str, body: &str) -> Entry {
        Entry {
            page: Some(title.into()),
            title: title.into(),
            desc: desc.into(),
            body: body.into(),
        }
    }

    #[test]
    fn term_split() {
        assert_eq!(terms("Wi-Fi  password, PASSWORD!"), ["wi-fi", "password"]);
        assert!(terms("  ,, ").is_empty());
    }

    #[test]
    fn ranking() {
        let entries = vec![
            e(
                "Printing",
                "Send documents to a printer",
                "Use the print dialog. password",
            ),
            e(
                "Wireless password",
                "Change the Wi-Fi password",
                "Enter the password, then the password again",
            ),
            e(
                "Passwords overview",
                "",
                "password password password password password password password",
            ),
            e("Unrelated", "nothing", "here"),
            e(
                "Networking",
                "Connect to wireless networks",
                "No pass word here",
            ),
        ];
        let hits = rank(&entries, "password");
        let order: Vec<usize> = hits.iter().map(|h| h.index).collect();
        assert_eq!(order, [1, 2, 0]);
        assert_eq!(hits[0].score, 10 + 4 + 2);
        assert_eq!(hits[1].score, 10 + 5, "body occurrences are capped");

        // All terms required; phrase in title boosts.
        let hits = rank(&entries, "wireless password");
        assert_eq!(hits.iter().map(|h| h.index).collect::<Vec<_>>(), [1]);
        assert_eq!(hits[0].score, 20 + 10 + 10 + 4 + 2);
        assert!(rank(&entries, "").is_empty());
        assert!(rank(&entries, "zzz").is_empty());
    }

    #[test]
    fn ties_sort_by_title() {
        let entries = vec![e("b", "", "x"), e("A", "", "x")];
        assert_eq!(
            rank(&entries, "x")
                .iter()
                .map(|h| h.index)
                .collect::<Vec<_>>(),
            [1, 0]
        );
    }

    #[test]
    fn snippets() {
        let body = "aaaa bbbb cccc dddd eeee ffff gggg hhhh needle iiii jjjj kkkk";
        let s = snippet(body, "needle", 20);
        assert!(s.contains("needle"), "{s}");
        assert!(s.starts_with('\u{2026}') && s.ends_with('\u{2026}'));
        assert_eq!(snippet("short", "x", 50), "short");
    }
}
