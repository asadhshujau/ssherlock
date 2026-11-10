//! Links in terminal output: explicit hyperlinks (`OSC 8`) and URLs found in plain text.
//!
//! Finding a link is the terminal's job. Deciding whether to open it ([`Link::is_safe_to_open`])
//! is a policy kept here so it is tested in one place.

use crate::selection::VisibleRange;

/// A link under the pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Where it goes. For an `OSC 8` hyperlink this can differ from the text shown, so the UI
    /// should display it before opening.
    pub uri: String,
    /// The cells it covers on the visible screen.
    pub range: VisibleRange,
}

/// Schemes that are opened when clicked. Anything else (`file:`, `ssh:`, custom app schemes) is
/// shown but not opened: an `OSC 8` link's target is chosen by whatever printed it, which may
/// be a remote server.
const SAFE_SCHEMES: &[&str] = &["http", "https", "mailto", "ftp"];

impl Link {
    /// Whether clicking should open this link.
    pub fn is_safe_to_open(&self) -> bool {
        is_safe_uri(&self.uri)
    }
}

pub(crate) fn is_safe_uri(uri: &str) -> bool {
    let Some((scheme, rest)) = uri.split_once(':') else {
        return false;
    };
    !rest.is_empty()
        && SAFE_SCHEMES
            .iter()
            .any(|safe| scheme.eq_ignore_ascii_case(safe))
}

/// URLs recognised in plain text: a known scheme followed by anything but whitespace, control
/// characters and characters that commonly surround URLs in prose.
pub(crate) const URL_PATTERN: &str = "(https?://|ftp://|mailto:|file://|ssh://|git://)\
    [^\u{0000}-\u{001F}\u{007F}-\u{009F}<>\"\\s{}\\^`]+";

/// How many characters to drop from the end of a URL found in text: sentence punctuation, and
/// closing brackets that weren't opened inside the URL (so `(see https://a.b/c)` doesn't take
/// the `)`, while `https://en.wikipedia.org/wiki/Rust_(language)` keeps it).
pub(crate) fn trailing_junk(url: &str) -> usize {
    let mut kept = url;
    while let Some(last) = kept.chars().next_back() {
        let opener = match last {
            ')' => Some('('),
            ']' => Some('['),
            _ => None,
        };
        let drop = match opener {
            Some(opener) => {
                kept.chars().filter(|&c| c == last).count()
                    > kept.chars().filter(|&c| c == opener).count()
            }
            None => matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*'),
        };
        if !drop {
            break;
        }
        match kept.strip_suffix(last) {
            Some(shorter) => kept = shorter,
            None => break,
        }
    }
    url.chars().count() - kept.chars().count()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn only_web_and_mail_links_open() {
        let cases = [
            ("https://example.com", true),
            ("HTTP://EXAMPLE.COM", true),
            ("mailto:a@b.c", true),
            ("ftp://host/file", true),
            ("file:///etc/passwd", false),
            ("ssh://host", false),
            ("javascript:alert(1)", false),
            ("vscode://open", false),
            ("https:", false),
            ("no scheme", false),
            ("", false),
        ];
        for (uri, expected) in cases {
            assert_eq!(is_safe_uri(uri), expected, "{uri}");
        }
    }

    #[test]
    fn trailing_punctuation_and_unbalanced_brackets_are_dropped() {
        let cases = [
            ("https://a.b/c", 0),
            ("https://a.b/c.", 1),
            ("https://a.b/c).", 2),
            ("https://a.b/c?x=1&y=2", 0),
            ("https://en.wikipedia.org/wiki/Rust_(language)", 0),
            ("https://en.wikipedia.org/wiki/Rust_(language)),", 2),
            ("https://a.b/[x]", 0),
            ("https://a.b/c]", 1),
            ("https://a.b/c:", 1),
            ("https://a.b/日本.", 1),
        ];
        for (url, expected) in cases {
            assert_eq!(trailing_junk(url), expected, "{url}");
        }
    }
}
