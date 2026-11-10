//! Key bindings: loads `keymap/default.json`, the one place keys are bound (CLAUDE.md §7).
//!
//! The file is a list of sections. Each has an optional `context` (matched against the
//! `key_context` of the focused view and its ancestors) and a map from keystrokes to action
//! names, e.g. `"mod-v": "terminal::Paste"`. `mod-` is expanded per platform by
//! [`platform::expand_keystrokes`].

use std::collections::BTreeMap;
use std::rc::Rc;

use gpui::{App, DummyKeyboardMapper, KeyBinding, KeyBindingContextPredicate};
use serde::Deserialize;

use crate::platform;

/// The bindings SSHerlock ships with.
const DEFAULT_KEYMAP: &str = include_str!("keymap/default.json");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Section {
    #[serde(default)]
    context: Option<String>,
    bindings: BTreeMap<String, String>,
}

/// Binds the default keymap. Call after every feature's `init`, so all actions exist.
///
/// Entries that don't parse are skipped and logged; the rest are still bound.
pub fn init(cx: &mut App) {
    let (bindings, problems) = parse(DEFAULT_KEYMAP, cx);
    for problem in &problems {
        tracing::error!(%problem, "skipping a default key binding");
    }
    tracing::debug!(count = bindings.len(), "bound default keymap");
    cx.bind_keys(bindings);
}

/// Turns keymap JSON into bindings, plus a description of each entry that couldn't be used.
fn parse(json: &str, cx: &App) -> (Vec<KeyBinding>, Vec<String>) {
    let sections: Vec<Section> = match serde_json::from_str(json) {
        Ok(sections) => sections,
        Err(error) => {
            return (
                Vec::new(),
                vec![format!("could not read the keymap: {error}")],
            );
        }
    };

    let mut bindings = Vec::new();
    let mut problems = Vec::new();
    for section in sections {
        let context = match section
            .context
            .as_deref()
            .map(KeyBindingContextPredicate::parse)
        {
            None => None,
            Some(Ok(predicate)) => Some(Rc::new(predicate)),
            Some(Err(error)) => {
                problems.push(format!(
                    "invalid context {:?}: {error}",
                    section.context.unwrap_or_default()
                ));
                continue;
            }
        };
        for (keystrokes, action_name) in section.bindings {
            let action = match cx.build_action(&action_name, None) {
                Ok(action) => action,
                Err(error) => {
                    problems.push(format!("unknown action {action_name:?}: {error}"));
                    continue;
                }
            };
            let expanded = platform::expand_keystrokes(&keystrokes);
            match KeyBinding::load(
                &expanded,
                action,
                context.clone(),
                false,
                None,
                &DummyKeyboardMapper,
            ) {
                Ok(binding) => bindings.push(binding),
                Err(error) => problems.push(format!("invalid keystrokes {keystrokes:?}: {error}")),
            }
        }
    }
    (bindings, problems)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use gpui::TestAppContext;
    use pretty_assertions::assert_eq;

    use super::*;

    #[gpui::test]
    fn the_default_keymap_binds_cleanly(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let (bindings, problems) = parse(DEFAULT_KEYMAP, cx);
            assert_eq!(problems, Vec::<String>::new());
            assert!(!bindings.is_empty());
        });
    }

    #[gpui::test]
    fn bad_entries_are_reported_and_skipped(cx: &mut TestAppContext) {
        let json = r#"[
            { "context": "Terminal", "bindings": {
                "mod-v": "terminal::Paste",
                "mod-x": "terminal::NoSuchAction",
                "q-w": "terminal::Paste"
            } },
            { "context": "(((", "bindings": { "a": "terminal::Paste" } }
        ]"#;
        cx.update(|cx| {
            let (bindings, problems) = parse(json, cx);
            assert_eq!(bindings.len(), 1, "{problems:?}");
            assert_eq!(problems.len(), 3, "{problems:?}");
        });
        cx.update(|cx| {
            let (bindings, problems) = parse("not json", cx);
            assert!(bindings.is_empty());
            assert!(
                problems
                    .first()
                    .unwrap()
                    .starts_with("could not read the keymap")
            );
        });
    }
}
