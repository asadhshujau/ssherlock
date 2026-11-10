//! Finding text in a terminal's screen and scrollback.
//!
//! The user types plain text. It is matched literally, case-insensitively unless it contains
//! an upper-case letter ("smart case", as in vim and less).

/// Which way to look for the next match.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SearchDirection {
    /// Up, towards older output.
    Older,
    /// Down, towards newer output.
    Newer,
}

/// How many matches there are and which one is focused, for "3 of 17".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SearchStatus {
    /// Matches in the screen and scrollback, at most [`MAX_COUNTED_MATCHES`].
    pub total: usize,
    /// Position of the focused match, counting from the oldest (1-based), if it was counted.
    pub current: Option<usize>,
    /// More matches exist than were counted.
    pub is_capped: bool,
}

/// Counting stops here, so a one-letter query in a long scrollback stays cheap.
pub const MAX_COUNTED_MATCHES: usize = 9_999;

/// A regex that matches `query` literally.
pub(crate) fn literal_pattern(query: &str) -> String {
    let mut pattern = String::with_capacity(query.len() * 2);
    for ch in query.chars() {
        if is_meta(ch) {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern
}

/// Characters with a meaning in regex syntax, which escaping turns back into themselves.
fn is_meta(ch: char) -> bool {
    matches!(
        ch,
        '\\' | '.'
            | '+'
            | '*'
            | '?'
            | '('
            | ')'
            | '|'
            | '['
            | ']'
            | '{'
            | '}'
            | '^'
            | '$'
            | '#'
            | '&'
            | '-'
            | '~'
    )
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn literal_pattern_escapes_regex_syntax() {
        let cases = [
            ("plain", "error", "error"),
            ("dot", "a.b", r"a\.b"),
            ("path", "/usr/local", "/usr/local"),
            ("brackets", "[x](y){z}", r"\[x\]\(y\)\{z\}"),
            ("anchors and alternation", "^a|b$", r"\^a\|b\$"),
            ("quantifiers", "a+b*c?", r"a\+b\*c\?"),
            ("backslash", r"C:\dir", r"C:\\dir"),
            ("class operators", "a-b&&c~~d#", r"a\-b\&\&c\~\~d\#"),
            ("unicode", "日本 é", "日本 é"),
        ];
        for (name, query, expected) in cases {
            assert_eq!(literal_pattern(query), expected, "{name}");
        }
    }
}
