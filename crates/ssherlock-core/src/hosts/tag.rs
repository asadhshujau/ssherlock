//! Tags: short words attached to hosts so the list can be filtered by them (HOST-03).

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A tag such as `prod` or `db`. Always lower-case, 1 to [`Tag::MAX_LEN`] characters, with no
/// spaces or commas (those separate tags when typed as a list), so two tags that look the same
/// are the same.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Tag(String);

/// Why text isn't a valid [`Tag`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TagError {
    /// Nothing was given.
    #[error("a tag cannot be empty")]
    Empty,
    /// Longer than [`Tag::MAX_LEN`].
    #[error("the tag {0:?} is longer than 32 characters")]
    TooLong(String),
    /// Contains a character tags can't have.
    #[error("the tag {0:?} contains {1:?}; use letters, digits and - _ . : / +")]
    InvalidCharacter(String, char),
}

impl Tag {
    /// The longest a tag may be, in characters.
    pub const MAX_LEN: usize = 32;

    /// The tag's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parses tags typed as a list, separated by commas and/or spaces: `"prod, db web"`.
    /// Duplicates collapse; the result is sorted.
    ///
    /// # Errors
    ///
    /// The first tag that isn't valid.
    pub fn parse_list(text: &str) -> Result<BTreeSet<Self>, TagError> {
        text.split(|c: char| c == ',' || c.is_whitespace())
            .filter(|part| !part.is_empty())
            .map(str::parse)
            .collect()
    }

    /// Tags written back as a list that [`Tag::parse_list`] reads: `"db, prod"`.
    pub fn format_list<'a>(tags: impl IntoIterator<Item = &'a Self>) -> String {
        tags.into_iter()
            .map(Self::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl FromStr for Tag {
    type Err = TagError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        if text.is_empty() {
            return Err(TagError::Empty);
        }
        if text.chars().count() > Self::MAX_LEN {
            return Err(TagError::TooLong(text.to_owned()));
        }
        if let Some(bad) = text
            .chars()
            .find(|&c| !(c.is_alphanumeric() || "-_.:/+".contains(c)))
        {
            return Err(TagError::InvalidCharacter(text.to_owned(), bad));
        }
        Ok(Self(text.to_lowercase()))
    }
}

impl TryFrom<String> for Tag {
    type Error = TagError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<Tag> for String {
    fn from(tag: Tag) -> Self {
        tag.0
    }
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn parsing() {
        let cases: Vec<(&str, Result<&str, TagError>)> = vec![
            ("prod", Ok("prod")),
            ("  Prod ", Ok("prod")),
            ("eu-west-1", Ok("eu-west-1")),
            ("k8s:node", Ok("k8s:node")),
            ("données", Ok("données")),
            ("", Err(TagError::Empty)),
            ("   ", Err(TagError::Empty)),
            (
                "two words",
                Err(TagError::InvalidCharacter("two words".into(), ' ')),
            ),
            ("a,b", Err(TagError::InvalidCharacter("a,b".into(), ','))),
            ("#x", Err(TagError::InvalidCharacter("#x".into(), '#'))),
        ];
        for (text, expected) in cases {
            assert_eq!(
                text.parse::<Tag>().as_ref().map(Tag::as_str),
                expected.as_ref().copied(),
                "{text:?}"
            );
        }
        let long = "a".repeat(33);
        assert_eq!(long.parse::<Tag>(), Err(TagError::TooLong(long.clone())));
        assert!("a".repeat(32).parse::<Tag>().is_ok());
    }

    #[test]
    fn lists() {
        let tags = Tag::parse_list("prod, DB  web,,prod").unwrap();
        assert_eq!(Tag::format_list(&tags), "db, prod, web");
        assert_eq!(Tag::parse_list("").unwrap().len(), 0);
        assert_eq!(Tag::parse_list(" , ").unwrap().len(), 0);
        assert_eq!(
            Tag::parse_list("ok, b@d"),
            Err(TagError::InvalidCharacter("b@d".into(), '@'))
        );
        let reparsed = Tag::parse_list(&Tag::format_list(&tags)).unwrap();
        assert_eq!(reparsed, tags);
    }

    #[test]
    fn serde_refuses_invalid_tags() {
        let tag: Tag = "db".parse().unwrap();
        assert_eq!(serde_json::to_string(&tag).unwrap(), r#""db""#);
        assert!(serde_json::from_str::<Tag>(r#""a b""#).is_err());
        // Old or hand-edited files with upper case still load, normalised.
        assert_eq!(serde_json::from_str::<Tag>(r#""DB""#).unwrap(), tag);
    }
}
