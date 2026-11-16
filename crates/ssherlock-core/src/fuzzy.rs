//! Fuzzy matching: does typed text appear, in order, inside a longer name?
//!
//! **Model** layer. Shared by host search and every picker, so `pdb` finds `prod-db` the same
//! way everywhere.

/// Scores how well `term` matches `haystack` as a subsequence: every character of `term`
/// appears in `haystack` in order. Both should already be lower-case. `None` if it doesn't
/// match; otherwise 10 to 40, higher when matched characters sit together or start words.
pub fn score(haystack: &str, term: &str) -> Option<u32> {
    let mut wanted = term.chars().peekable();
    let mut bonus = 0u32;
    let mut previous: Option<char> = None;
    let mut last_matched = false;
    for c in haystack.chars() {
        let Some(&want) = wanted.peek() else { break };
        if c == want {
            wanted.next();
            if last_matched {
                bonus += 2;
            }
            if previous.is_none_or(|p| !p.is_alphanumeric()) {
                bonus += 3;
            }
            last_matched = true;
        } else {
            last_matched = false;
        }
        previous = Some(c);
    }
    wanted.peek().is_none().then(|| 10 + bonus.min(30))
}

/// Sorts `items` by how well their text (from `text`, lower-cased) matches `query`, best
/// first, dropping those that don't match. Ties keep their original order. Every
/// whitespace-separated word of `query` must match; an empty query keeps everything.
pub fn filter<T>(items: Vec<T>, query: &str, text: impl Fn(&T) -> String) -> Vec<T> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if terms.is_empty() {
        return items;
    }
    let mut scored: Vec<(u32, T)> = items
        .into_iter()
        .filter_map(|item| {
            let haystack = text(&item).to_lowercase();
            let total = terms.iter().try_fold(0, |total, term| {
                let points = if haystack.starts_with(term.as_str()) {
                    100
                } else if haystack.contains(term.as_str()) {
                    60
                } else {
                    score(&haystack, term)?
                };
                Some(total + points)
            })?;
            Some((total, item))
        })
        .collect();
    // Stable, so equal scores stay in their original order.
    scored.sort_by(|(a, _), (b, _)| b.cmp(a));
    scored.into_iter().map(|(_, item)| item).collect()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn subsequence_scores() {
        assert!(score("prod-db", "pdb").is_some());
        assert!(score("prod-db", "bdp").is_none());
        assert!(score("prod-db", "pdb") > score("xpxrxoxdxdxb", "pdb"));
        assert_eq!(score("anything", ""), Some(10));
        assert_eq!(score("", "a"), None);
    }

    #[test]
    fn filtering_ranks_prefixes_then_substrings_then_fuzzy() {
        let items = vec!["Staging", "prod / db", "Production", "web-prod"];
        let found = filter(items.clone(), "prod", |s| (*s).to_owned());
        assert_eq!(found, ["prod / db", "Production", "web-prod"]);
        assert_eq!(filter(items.clone(), "  ", |s| (*s).to_owned()), items);
        assert_eq!(
            filter(items.clone(), "pdb", |s| (*s).to_owned()),
            ["prod / db"]
        );
        assert_eq!(filter(items, "prod db", |s| (*s).to_owned()), ["prod / db"]);
    }
}
