//! Points in time, and how the UI describes them.
//!
//! **Model** layer. Models store [`Timestamp`]s (UTC; RFC 3339 text in SQLite). Functions here
//! take "now" as an argument instead of reading the clock, so they stay pure and testable.

pub use jiff::Timestamp;

/// A short description of when `then` was, seen from `now`: "just now", "5 min ago",
/// "3 h ago", "yesterday", "4 days ago", or the UTC date (`2026-03-01`) beyond a month.
/// A `then` in the future (a clock that moved back) reads "just now".
pub fn ago(then: Timestamp, now: Timestamp) -> String {
    let seconds = now.as_second().saturating_sub(then.as_second());
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    match () {
        () if minutes < 1 => "just now".to_owned(),
        () if hours < 1 => format!("{minutes} min ago"),
        () if days < 1 => format!("{hours} h ago"),
        () if days == 1 => "yesterday".to_owned(),
        () if days < 31 => format!("{days} days ago"),
        () => then.strftime("%Y-%m-%d").to_string(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn descriptions() {
        let now: Timestamp = "2026-11-16T12:00:00Z".parse().unwrap();
        let cases = [
            ("2026-11-16T12:00:00Z", "just now"),
            ("2026-11-16T11:59:30Z", "just now"),
            ("2026-11-16T12:05:00Z", "just now"),
            ("2026-11-16T11:55:00Z", "5 min ago"),
            ("2026-11-16T11:00:01Z", "59 min ago"),
            ("2026-11-16T09:00:00Z", "3 h ago"),
            ("2026-11-15T11:00:00Z", "yesterday"),
            ("2026-11-12T12:00:00Z", "4 days ago"),
            ("2026-10-17T12:00:00Z", "30 days ago"),
            ("2026-03-01T08:00:00Z", "2026-03-01"),
        ];
        for (then, expected) in cases {
            assert_eq!(ago(then.parse().unwrap(), now), expected, "{then}");
        }
    }
}
