//! [`RuleEditor`]: the form for one port-forwarding rule (FWD-01…05, FWD-08): its kind, name,
//! listening port and address, destination, and how it starts.
//!
//! It doesn't save anything itself: it hands the finished rule to whoever opened it, which saves
//! it with its host. Turning the typed text into a [`Forward`] is [`build_forward`], tested
//! without gpui.

use std::collections::HashMap;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Window, div,
};
use ssherlock_core::forwarding::{
    Bind, Destination, Forward, ForwardRule, ForwardRuleError, Listen,
};
use ssherlock_core::hosts::Host;
use ssherlock_core::{Address, Id, Port, Timestamp};

use super::actions::{CancelRuleEditor, SaveRuleEditor};
use crate::components::{
    Checkbox, Form, FormField, FormSection, Segment, SegmentedControl, TextInput, sheet,
};
use crate::ssh::status::capitalise;
use crate::theme::ActiveTheme as _;

/// What the form tells its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleEditorEvent {
    /// Closed: with the finished rule, or `None` if cancelled.
    Done(Option<ForwardRule>),
}

/// The fields that can show an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RuleField {
    /// The name.
    Label,
    /// The port it listens on.
    Port,
    /// The destination's host.
    DestinationHost,
    /// The destination's port.
    DestinationPort,
}

/// The kinds, in the order the form offers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `ssh -L`.
    Local,
    /// `ssh -R`.
    Remote,
    /// `ssh -D`.
    Dynamic,
}

const KINDS: [Kind; 3] = [Kind::Local, Kind::Remote, Kind::Dynamic];
const BINDS: [Bind; 2] = [Bind::Localhost, Bind::AllInterfaces];

/// The destination host when the field is left empty: something on the far end itself.
const DEFAULT_DESTINATION: &str = "localhost";

impl Kind {
    fn of(forward: &Forward) -> Self {
        match forward {
            Forward::Local { .. } => Self::Local,
            Forward::Remote { .. } => Self::Remote,
            Forward::Dynamic { .. } => Self::Dynamic,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::Remote => "Remote",
            Self::Dynamic => "SOCKS proxy",
        }
    }

    fn explanation(self) -> &'static str {
        match self {
            Self::Local => {
                "A port on this machine leads to a host and port the server can reach (ssh -L)"
            }
            Self::Remote => {
                "A port on the server leads to a host and port this machine can reach (ssh -R)"
            }
            Self::Dynamic => {
                "A SOCKS5 proxy on this machine; connections through it leave from the server \
                 (ssh -D)"
            }
        }
    }
}

/// The forward the form's values describe, or every problem with them. An empty destination
/// host means [`DEFAULT_DESTINATION`]; a SOCKS proxy ignores the destination fields.
///
/// # Errors
///
/// One message per field that's wrong.
pub fn build_forward(
    kind: Kind,
    bind: Bind,
    port: &str,
    destination_host: &str,
    destination_port: &str,
) -> Result<Forward, Vec<(RuleField, String)>> {
    let mut errors = Vec::new();
    let mut port_of = |text: &str, field| match text.trim().parse::<Port>() {
        Ok(port) => Some(port),
        Err(error) => {
            errors.push((field, error.to_string()));
            None
        }
    };
    let listen = port_of(port, RuleField::Port).map(|port| Listen { bind, port });
    let destination = if kind == Kind::Dynamic {
        None
    } else {
        let port = port_of(destination_port, RuleField::DestinationPort);
        let host = destination_host.trim();
        let host = if host.is_empty() {
            DEFAULT_DESTINATION
        } else {
            host
        };
        let host = match host.parse::<Address>() {
            Ok(host) => Some(host),
            Err(error) => {
                errors.push((RuleField::DestinationHost, error.to_string()));
                None
            }
        };
        host.zip(port)
            .map(|(host, port)| Destination { host, port })
    };
    match (kind, listen, destination) {
        (Kind::Local, Some(listen), Some(destination)) if errors.is_empty() => Ok(Forward::Local {
            listen,
            destination,
        }),
        (Kind::Remote, Some(listen), Some(destination)) if errors.is_empty() => {
            Ok(Forward::Remote {
                listen,
                destination,
            })
        }
        (Kind::Dynamic, Some(listen), _) if errors.is_empty() => Ok(Forward::Dynamic { listen }),
        _ => Err(errors),
    }
}

/// The form for one rule, new or existing.
pub struct RuleEditor {
    /// The rule being edited; a new one is a placeholder until saved.
    rule: ForwardRule,
    is_new: bool,
    kind: Kind,
    bind: Bind,
    label: Entity<TextInput>,
    port: Entity<TextInput>,
    destination_host: Entity<TextInput>,
    destination_port: Entity<TextInput>,
    is_enabled: bool,
    auto_start: bool,
    kind_focus: FocusHandle,
    bind_focus: FocusHandle,
    enabled_focus: FocusHandle,
    auto_start_focus: FocusHandle,
    errors: HashMap<RuleField, SharedString>,
    focus_handle: FocusHandle,
}

impl EventEmitter<RuleEditorEvent> for RuleEditor {}

impl Focusable for RuleEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl RuleEditor {
    /// A form for `rule`, or for a new rule of `host` when `None`, with the kind focused.
    pub fn new(
        host: Id<Host>,
        rule: Option<ForwardRule>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let is_new = rule.is_none();
        let (kind, bind, port, destination_host, destination_port) = match &rule {
            Some(rule) => {
                let listen = rule.forward.listen();
                let destination = rule.forward.destination();
                (
                    Kind::of(&rule.forward),
                    listen.bind,
                    listen.port.to_string(),
                    destination.map_or_else(String::new, |d| d.host.to_string()),
                    destination.map_or_else(String::new, |d| d.port.to_string()),
                )
            }
            None => (
                Kind::Local,
                Bind::Localhost,
                String::new(),
                String::new(),
                String::new(),
            ),
        };
        let rule = rule.unwrap_or_else(|| {
            // Replaced by what the form says on save.
            let placeholder = Forward::Dynamic {
                listen: Listen::localhost(Port::SSH),
            };
            ForwardRule::new(host, placeholder, Timestamp::now())
        });
        let label = rule.label.clone();
        let editor = Self {
            kind,
            bind,
            label: cx.new(|cx| TextInput::field(label, cx).placeholder("e.g. Postgres")),
            port: cx.new(|cx| TextInput::field(port, cx).placeholder("e.g. 8080")),
            destination_host: cx
                .new(|cx| TextInput::field(destination_host, cx).placeholder(DEFAULT_DESTINATION)),
            destination_port: cx
                .new(|cx| TextInput::field(destination_port, cx).placeholder("e.g. 5432")),
            is_enabled: rule.is_enabled,
            auto_start: rule.auto_start,
            rule,
            is_new,
            kind_focus: cx.focus_handle().tab_stop(true),
            bind_focus: cx.focus_handle().tab_stop(true),
            enabled_focus: cx.focus_handle().tab_stop(true),
            auto_start_focus: cx.focus_handle().tab_stop(true),
            errors: HashMap::new(),
            focus_handle: cx.focus_handle(),
        };
        editor.kind_focus.focus(window);
        editor
    }

    /// The error shown for `field`, if any.
    pub fn error(&self, field: RuleField) -> Option<&str> {
        self.errors.get(&field).map(SharedString::as_ref)
    }

    fn fail(&mut self, field: RuleField, message: &str) {
        self.errors
            .entry(field)
            .or_insert_with(|| capitalise(message).into());
    }

    fn save(&mut self, _: &SaveRuleEditor, _: &mut Window, cx: &mut Context<Self>) {
        self.errors.clear();
        let forward = build_forward(
            self.kind,
            self.bind,
            self.port.read(cx).text(),
            self.destination_host.read(cx).text(),
            self.destination_port.read(cx).text(),
        );
        let label = self.label.read(cx).text().trim().to_owned();
        let forward = match forward {
            Ok(forward) => Some(forward),
            Err(errors) => {
                for (field, message) in errors {
                    self.fail(field, &message);
                }
                None
            }
        };
        let rule = forward.map(|forward| ForwardRule {
            label,
            forward,
            is_enabled: self.is_enabled,
            auto_start: self.auto_start,
            updated_at: Timestamp::now(),
            ..self.rule.clone()
        });
        if let Some(Err(error)) = rule.as_ref().map(ForwardRule::validate) {
            let field = match error {
                ForwardRuleError::LabelTooLong
                | ForwardRuleError::LabelControl
                | ForwardRuleError::Clash { .. } => RuleField::Label,
            };
            self.fail(field, &error.to_string());
        }
        match rule {
            Some(rule) if self.errors.is_empty() => cx.emit(RuleEditorEvent::Done(Some(rule))),
            _ => cx.notify(),
        }
    }

    #[allow(clippy::unused_self, reason = "gpui action listeners take `&mut self`")]
    fn cancel(&mut self, _: &CancelRuleEditor, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(RuleEditorEvent::Done(None));
    }
}

impl Render for RuleEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let error = |field: RuleField| self.errors.get(&field).cloned();
        let title = if self.is_new {
            "New port forwarding rule".to_owned()
        } else {
            format!("Edit {}", self.rule.title())
        };
        let kind_index = KINDS.iter().position(|k| *k == self.kind).unwrap_or(0);
        let kind = SegmentedControl::new(
            "rule-kind",
            KINDS.iter().map(|k| Segment::new(k.label())).collect(),
            kind_index,
            &self.kind_focus,
        )
        .on_select(cx.listener(|editor, index: &usize, _, cx| {
            if let Some(kind) = KINDS.get(*index) {
                editor.kind = *kind;
                cx.notify();
            }
        }));
        let on_server = self.kind == Kind::Remote;
        let bind_index = BINDS.iter().position(|b| *b == self.bind).unwrap_or(0);
        let bind = SegmentedControl::new(
            "rule-bind",
            vec![
                Segment::new(if on_server {
                    "The server only"
                } else {
                    "This machine only"
                }),
                Segment::new("All interfaces"),
            ],
            bind_index,
            &self.bind_focus,
        )
        .on_select(cx.listener(|editor, index: &usize, _, cx| {
            if let Some(bind) = BINDS.get(*index) {
                editor.bind = *bind;
                cx.notify();
            }
        }));
        let bind_hint = match (self.bind, on_server) {
            (Bind::Localhost, false) => "Only programs on this machine can connect",
            (Bind::Localhost, true) => "Only programs on the server can connect",
            (Bind::AllInterfaces, false) => "Other machines on your network can connect too",
            (Bind::AllInterfaces, true) => {
                "Other machines can connect too, if the server allows it (GatewayPorts)"
            }
        };
        let form = Form::new()
            .child(
                FormSection::new("Rule")
                    .child(FormField::new("Kind", kind).hint(self.kind.explanation()))
                    .child(
                        FormField::new("Name", self.label.clone())
                            .hint("Optional; what it forwards if empty")
                            .error(error(RuleField::Label)),
                    ),
            )
            .child(
                FormSection::new(if on_server {
                    "Listen on the server"
                } else {
                    "Listen on this machine"
                })
                .child(FormField::new("Port", self.port.clone()).error(error(RuleField::Port)))
                .child(FormField::new("Address", bind).hint(bind_hint)),
            )
            .when(self.kind != Kind::Dynamic, |form| {
                form.child(
                    FormSection::new(if on_server {
                        "Destination, as this machine sees it"
                    } else {
                        "Destination, as the server sees it"
                    })
                    .child(
                        FormField::new("Host", self.destination_host.clone())
                            .error(error(RuleField::DestinationHost)),
                    )
                    .child(
                        FormField::new("Port", self.destination_port.clone())
                            .error(error(RuleField::DestinationPort)),
                    ),
                )
            })
            .child(
                FormSection::new("Starting")
                    .child(FormField::new(
                        "",
                        Checkbox::new(
                            "rule-enabled",
                            "Enabled",
                            self.is_enabled,
                            &self.enabled_focus,
                        )
                        .on_toggle(cx.listener(
                            |editor, value: &bool, _, cx| {
                                editor.is_enabled = *value;
                                cx.notify();
                            },
                        )),
                    ))
                    .child(
                        FormField::new(
                            "",
                            Checkbox::new(
                                "rule-auto-start",
                                "Start when connecting to the host",
                                self.auto_start,
                                &self.auto_start_focus,
                            )
                            .on_toggle(cx.listener(
                                |editor, value: &bool, _, cx| {
                                    editor.auto_start = *value;
                                    cx.notify();
                                },
                            )),
                        )
                        .hint("Otherwise start it from Port forwarding"),
                    ),
            );
        sheet(
            &theme,
            title,
            "Done",
            div().child(form).into_any_element(),
            cx.listener(|editor, _, window, cx| editor.cancel(&CancelRuleEditor, window, cx)),
            cx.listener(|editor, _, window, cx| editor.save(&SaveRuleEditor, window, cx)),
        )
        .key_context("RuleEditor")
        .track_focus(&self.focus_handle)
        .on_action(cx.listener(Self::save))
        .on_action(cx.listener(Self::cancel))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;

    use super::*;

    /// Kind, bind, the port, destination host and destination port typed, and the outcome.
    type Case = (
        Kind,
        Bind,
        [&'static str; 3],
        Result<Forward, Vec<RuleField>>,
    );

    fn port(n: u16) -> Port {
        Port::new(n).unwrap()
    }

    #[test]
    fn building_forwards() {
        let to = |host: &str, p| Destination {
            host: host.parse().unwrap(),
            port: port(p),
        };
        let all = |p| Listen {
            bind: Bind::AllInterfaces,
            port: port(p),
        };
        let cases: Vec<Case> = vec![
            (
                Kind::Local,
                Bind::Localhost,
                ["8080", "db.internal", "5432"],
                Ok(Forward::Local {
                    listen: Listen::localhost(port(8080)),
                    destination: to("db.internal", 5432),
                }),
            ),
            (
                Kind::Remote,
                Bind::AllInterfaces,
                [" 9000 ", "", "3000"],
                Ok(Forward::Remote {
                    listen: all(9000),
                    destination: to("localhost", 3000),
                }),
            ),
            (
                Kind::Dynamic,
                Bind::Localhost,
                ["1080", "ignored host", "ignored"],
                Ok(Forward::Dynamic {
                    listen: Listen::localhost(port(1080)),
                }),
            ),
            (
                Kind::Local,
                Bind::Localhost,
                ["", "bad host", "0"],
                Err(vec![
                    RuleField::Port,
                    RuleField::DestinationPort,
                    RuleField::DestinationHost,
                ]),
            ),
            (
                Kind::Dynamic,
                Bind::Localhost,
                ["70000", "", ""],
                Err(vec![RuleField::Port]),
            ),
        ];
        for (kind, bind, [p, host, dp], expected) in cases {
            let got = build_forward(kind, bind, p, host, dp)
                .map_err(|errors| errors.into_iter().map(|(field, _)| field).collect());
            assert_eq!(got, expected, "{kind:?} {p:?} {host:?} {dp:?}");
        }
        let messages = build_forward(Kind::Local, Bind::Localhost, "x", "a b", "1").unwrap_err();
        assert_eq!(
            messages,
            vec![
                (
                    RuleField::Port,
                    "the port \"x\" is not a number from 1 to 65535".to_owned()
                ),
                (
                    RuleField::DestinationHost,
                    "the address \"a b\" contains ' ', which is not allowed in a host name"
                        .to_owned()
                ),
            ]
        );
    }

    #[gpui::test]
    fn problems_show_under_their_fields_and_escape_cancels(cx: &mut gpui::TestAppContext) {
        use std::cell::RefCell;
        use std::rc::Rc;

        cx.update(crate::keymap::init);
        let (editor, cx) =
            cx.add_window_view(|window, cx| RuleEditor::new(Id::generate(), None, window, cx));
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&events);
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&editor, move |_, event: &RuleEditorEvent, _| {
                sink.borrow_mut().push(event.clone());
            })
        });

        // SOCKS needs only a port: the destination fields aren't asked for.
        cx.simulate_keystrokes("right right enter");
        cx.run_until_parked();
        assert_eq!(
            editor.read_with(cx, |e, _| e.error(RuleField::Port).map(str::to_owned)),
            Some("The port \"\" is not a number from 1 to 65535".to_owned())
        );
        assert!(!editor.read_with(cx, |e, _| e.error(RuleField::DestinationPort).is_some()));
        assert!(events.borrow().is_empty());

        cx.simulate_keystrokes("escape");
        assert_eq!(*events.borrow(), [RuleEditorEvent::Done(None)]);
    }
}
