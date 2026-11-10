//! Key bindings: loads `keymap/default.json`, the one place keys are bound (CLAUDE.md §7).
//!
//! The file is a list of sections. Each has an optional `context` (matched against the
//! `key_context` of the focused view and its ancestors) and a map from keystrokes to action
//! names, e.g. `"mod-v": "terminal::Paste"`. `mod-` is expanded per platform by
//! [`platform::expand_keystrokes`]; gpui's `secondary-` (Cmd on macOS, Ctrl elsewhere) also works.
//! A section with `"os": ["macos"]` (or `"linux"`, `"windows"`) is bound only there, for keys that
//! are taken by the desktop on some platforms.

use std::collections::BTreeMap;
use std::rc::Rc;

use gpui::{App, DummyKeyboardMapper, KeyBinding, KeyBindingContextPredicate};
use serde::Deserialize;

use crate::platform::{self, Os};

/// The bindings SSHerlock ships with.
const DEFAULT_KEYMAP: &str = include_str!("keymap/default.json");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Section {
    #[serde(default)]
    context: Option<String>,
    /// The operating systems the section applies to; all of them if absent.
    #[serde(default)]
    os: Option<Vec<Os>>,
    bindings: BTreeMap<String, String>,
}

/// Binds the default keymap. Call after every feature's `init`, so all actions exist.
///
/// Entries that don't parse are skipped and logged; the rest are still bound.
pub fn init(cx: &mut App) {
    let (bindings, problems) = parse(DEFAULT_KEYMAP, platform::OS, cx);
    for problem in &problems {
        tracing::error!(%problem, "skipping a default key binding");
    }
    tracing::debug!(count = bindings.len(), "bound default keymap");
    cx.bind_keys(bindings);
}

/// Turns keymap JSON into the bindings for `os`, plus a description of each entry that couldn't
/// be used.
fn parse(json: &str, os: Os, cx: &App) -> (Vec<KeyBinding>, Vec<String>) {
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
        if section.os.as_ref().is_some_and(|list| !list.contains(&os)) {
            continue;
        }
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
            for os in [Os::Macos, Os::Linux, Os::Windows] {
                let (bindings, problems) = parse(DEFAULT_KEYMAP, os, cx);
                assert_eq!(problems, Vec::<String>::new(), "{os:?}");
                assert!(!bindings.is_empty(), "{os:?}");
            }
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
            let (bindings, problems) = parse(json, Os::Linux, cx);
            assert_eq!(bindings.len(), 1, "{problems:?}");
            assert_eq!(problems.len(), 3, "{problems:?}");
        });
        cx.update(|cx| {
            let (bindings, problems) = parse("not json", Os::Linux, cx);
            assert!(bindings.is_empty());
            assert!(
                problems
                    .first()
                    .unwrap()
                    .starts_with("could not read the keymap")
            );
        });
    }

    #[gpui::test]
    fn sections_can_be_limited_to_some_platforms(cx: &mut TestAppContext) {
        let json = r#"[
            { "context": "Terminal", "os": ["macos"], "bindings": { "alt-r": "terminal::ResetTerminal" } },
            { "context": "Terminal", "os": ["linux", "windows"], "bindings": { "ctrl-shift-delete": "terminal::ResetTerminal" } },
            { "context": "Terminal", "bindings": { "shift-insert": "terminal::Paste" } }
        ]"#;
        cx.update(|cx| {
            for (os, expected) in [
                (Os::Macos, ["alt-r", "shift-insert"]),
                (Os::Linux, ["ctrl-shift-delete", "shift-insert"]),
                (Os::Windows, ["ctrl-shift-delete", "shift-insert"]),
            ] {
                let (bindings, problems) = parse(json, os, cx);
                assert_eq!(problems, Vec::<String>::new());
                let keys: Vec<String> = bindings
                    .iter()
                    .map(|binding| {
                        binding
                            .keystrokes()
                            .iter()
                            .map(gpui::KeybindingKeystroke::unparse)
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .collect();
                assert_eq!(keys, expected, "{os:?}");
            }
        });
    }
}
