//! [`PortAuthority`]: the Port forwarding page (FWD-06, FWD-07). Every host's rules, grouped by
//! host, with what each is doing and its traffic, and a way to start and stop them.
//!
//! Starting a rule here opens a forwarding-only connection to its host through
//! [`ForwardsModel`]; the questions that connection asks (host key, password) are shown here as
//! the same dialogs a terminal tab shows. The page draws them through [`overlay`], over the
//! whole hosts page.
//!
//! [`overlay`]: PortAuthority::overlay

use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    ScrollHandle, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div,
};
use ssherlock_core::Id;
use ssherlock_core::forwarding::ForwardRule;
use ssherlock_core::hosts::Host;

use super::actions::{EditRuleHost, StopAllForwards, ToggleForward};
use super::item;
use super::model::ForwardsModel;
use crate::components::{Button, ButtonStyle};
use crate::hosts::HostsModel;
use crate::hosts::actions::{SelectFirst, SelectLast, SelectNext, SelectPrevious};
use crate::notify;
use crate::ssh::status::{self, Tone};
use crate::ssh::{SessionPrompts, SessionPromptsEvent};
use crate::theme::{ActiveTheme as _, Theme};

/// How often the traffic counters are redrawn while something runs.
const REFRESH: Duration = Duration::from_secs(1);

/// What the page asks of the hosts page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortAuthorityEvent {
    /// Open the editor of this host, where its rules are changed.
    EditHost(Id<Host>),
}

/// Hosts that have rules, sorted by title, each with its rules in the order they were added.
pub fn grouped<'a>(
    hosts: &'a [Host],
    rules: &'a [ForwardRule],
) -> Vec<(&'a Host, Vec<&'a ForwardRule>)> {
    let mut groups: Vec<(&Host, Vec<&ForwardRule>)> = hosts
        .iter()
        .map(|host| (host, rules.iter().filter(|r| r.host == host.id).collect()))
        .filter(|(_, rules): &(&Host, Vec<&ForwardRule>)| !rules.is_empty())
        .collect();
    groups.sort_by_cached_key(|(host, _)| host.title().to_lowercase());
    groups
}

/// The Port forwarding page.
pub struct PortAuthority {
    hosts: Entity<HostsModel>,
    forwards: Entity<ForwardsModel>,
    cursor: Option<Id<ForwardRule>>,
    /// The dialog for the question a forwarding-only connection is waiting on.
    prompts: Entity<SessionPrompts>,
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
    /// Redraws the counters while something runs and the page is shown.
    ticker: Option<Task<()>>,
    last_drawn: Instant,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PortAuthorityEvent> for PortAuthority {}

impl Focusable for PortAuthority {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        let prompts = self.prompts.read(cx);
        if prompts.is_open() {
            prompts.focus_handle(cx)
        } else {
            self.focus_handle.clone()
        }
    }
}

impl PortAuthority {
    /// The page over `hosts`' rules and `forwards`' running forwards.
    pub fn new(
        hosts: Entity<HostsModel>,
        forwards: Entity<ForwardsModel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let prompts = cx.new(SessionPrompts::new);
        let subscriptions = vec![
            cx.observe(&hosts, |page, _, cx| page.keep_cursor(cx)),
            cx.observe_in(&forwards, window, |page, _, window, cx| {
                page.ask_next(window, cx);
                cx.notify();
            }),
            cx.subscribe_in(
                &prompts,
                window,
                |page, _, event: &SessionPromptsEvent, window, cx| {
                    let SessionPromptsEvent::Closed { had_focus } = event;
                    if *had_focus {
                        page.focus_handle.focus(window);
                    }
                    page.ask_next(window, cx);
                    cx.notify();
                },
            ),
        ];
        let mut page = Self {
            hosts,
            forwards,
            cursor: None,
            prompts,
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            ticker: None,
            last_drawn: Instant::now(),
            _subscriptions: subscriptions,
        };
        // The rules may be loaded already, in which case no change will come to set it.
        page.cursor = page.ordered_ids(cx).first().copied();
        page
    }

    /// The rule under the cursor.
    pub fn cursor(&self) -> Option<Id<ForwardRule>> {
        self.cursor
    }

    /// The dialog over the page, for the hosts page to draw above everything.
    pub fn overlay(&self, cx: &App) -> Option<AnyElement> {
        self.prompts
            .read(cx)
            .is_open()
            .then(|| self.prompts.clone().into_any_element())
    }

    /// Shows the next question a forwarding-only connection is waiting on, if no dialog is up.
    fn ask_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompts.read(cx).is_open() {
            return;
        }
        let Some((_, question)) = self.forwards.update(cx, |model, _| model.take_question()) else {
            return;
        };
        self.prompts.update(cx, |prompts, cx| {
            // Only questions are queued, so nothing comes back.
            let _ = prompts.ask(question, window, cx);
        });
    }

    /// Every rule in the order the page shows them.
    fn ordered_ids(&self, cx: &App) -> Vec<Id<ForwardRule>> {
        let model = self.hosts.read(cx);
        grouped(model.hosts(), model.forward_rules())
            .into_iter()
            .flat_map(|(_, rules)| rules.into_iter().map(|r| r.id))
            .collect()
    }

    fn rule(&self, id: Id<ForwardRule>, cx: &App) -> Option<ForwardRule> {
        self.hosts
            .read(cx)
            .forward_rules()
            .iter()
            .find(|r| r.id == id)
            .cloned()
    }

    /// Keeps the cursor on a rule that still exists.
    fn keep_cursor(&mut self, cx: &mut Context<Self>) {
        let ids = self.ordered_ids(cx);
        if self.cursor.is_none_or(|c| !ids.contains(&c)) {
            self.cursor = ids.first().copied();
        }
        cx.notify();
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let ids = self.ordered_ids(cx);
        let Some(last) = ids.len().checked_sub(1) else {
            return;
        };
        let index = self
            .cursor
            .and_then(|c| ids.iter().position(|id| *id == c))
            .map_or(0, |i| i.saturating_add_signed(delta).min(last));
        self.cursor = ids.get(index).copied();
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, cx);
    }

    fn select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.ordered_ids(cx).first().copied();
        cx.notify();
    }

    fn select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.ordered_ids(cx).last().copied();
        cx.notify();
    }

    /// Stops `rule` if it's running; otherwise starts it, or tries again after a failure.
    pub fn toggle(&mut self, rule: Id<ForwardRule>, cx: &mut Context<Self>) {
        let Some(rule) = self.rule(rule, cx) else {
            return;
        };
        if self.forwards.read(cx).is_running(rule.id) {
            self.forwards
                .update(cx, |model, cx| model.stop(rule.id, cx));
        } else if rule.is_enabled {
            self.forwards.update(cx, |model, cx| model.start(&rule, cx));
        } else {
            notify::info(
                format!(
                    "{} is disabled; enable it in its host's editor first",
                    rule.title()
                ),
                cx,
            );
        }
    }

    fn toggle_cursor(&mut self, _: &ToggleForward, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(rule) = self.cursor {
            self.toggle(rule, cx);
        }
    }

    #[allow(clippy::unused_self, reason = "gpui action listeners take `&mut self`")]
    fn stop_all(&mut self, _: &StopAllForwards, _: &mut Window, cx: &mut Context<Self>) {
        self.forwards.update(cx, ForwardsModel::stop_all);
    }

    fn edit_host(&mut self, _: &EditRuleHost, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(rule) = self.cursor.and_then(|id| self.rule(id, cx)) {
            cx.emit(PortAuthorityEvent::EditHost(rule.host));
        }
    }

    /// Redraws once a second while something runs, so the counters move; stops when nothing
    /// runs or the page hasn't been drawn lately (another page is showing).
    fn keep_counters_fresh(&mut self, cx: &mut Context<Self>) {
        self.last_drawn = Instant::now();
        if self.ticker.is_some() || !self.forwards.read(cx).has_running() {
            return;
        }
        self.ticker = Some(cx.spawn(async move |page, cx| {
            loop {
                cx.background_executor().timer(REFRESH).await;
                let keep_going = page.update(cx, |page, cx| {
                    let is_shown = page.last_drawn.elapsed() < REFRESH * 2;
                    let keep = is_shown && page.forwards.read(cx).has_running();
                    if keep {
                        cx.notify();
                    } else {
                        page.ticker = None;
                    }
                    keep
                });
                if !matches!(keep_going, Ok(true)) {
                    return;
                }
            }
        }));
    }
}

impl Render for PortAuthority {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.keep_counters_fresh(cx);
        let theme = cx.theme().clone();
        let is_focused = self.focus_handle.contains_focused(window, cx);
        let hosts_model = self.hosts.read(cx);
        let forwards = self.forwards.read(cx);
        let groups = grouped(hosts_model.hosts(), hosts_model.forward_rules());
        let running = forwards.has_running();
        let rule_count: usize = groups.iter().map(|(_, rules)| rules.len()).sum();

        let toolbar = div()
            .flex()
            .items_center()
            .gap(theme.spacing.md)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_baseline()
                    .gap(theme.spacing.md)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Port forwarding"),
                    )
                    .child(
                        div()
                            .text_size(theme.text.small_size)
                            .text_color(theme.colors.text_muted)
                            .child(match rule_count {
                                1 => "1 rule".to_owned(),
                                n => format!("{n} rules"),
                            }),
                    ),
            )
            .when(running, |d| {
                d.child(Button::new("stop-all-forwards", "Stop all").on_click(
                    cx.listener(|page, _, window, cx| page.stop_all(&StopAllForwards, window, cx)),
                ))
            });

        let mut rows: Vec<AnyElement> = Vec::new();
        for (host, rules) in &groups {
            let tunnel = forwards
                .tunnel(host.id)
                .and_then(|(_, state)| state.and_then(status::describe));
            rows.push(host_heading(host, tunnel, &theme).into_any_element());
            for rule in rules {
                let view = forwards.view(rule.id);
                let (tone, status_line) = item::status_text(rule, view.as_ref());
                let is_running = forwards.is_running(rule.id);
                let failed = matches!(
                    view.as_ref().map(|v| &v.status),
                    Some(ssherlock_ssh::ForwardStatus::Failed(_))
                );
                // A forward that never ran has no traffic to speak of.
                let detail = view.as_ref().filter(|_| !failed).map(|v| {
                    v.stats.last_error.as_deref().map_or_else(
                        || item::traffic_text(&v.stats),
                        |e| format!("Last connection failed: {e}"),
                    )
                });
                let is_cursor = self.cursor == Some(rule.id);
                let id = rule.id;
                let hover = theme.colors.control_hover;
                let label = match (is_running, failed) {
                    (true, _) => "Stop",
                    (false, true) => "Retry",
                    (false, false) => "Start",
                };
                rows.push(
                    div()
                        .id(gpui::ElementId::Name(format!("rule-{id}").into()))
                        .flex()
                        .items_center()
                        .gap(theme.spacing.md)
                        .px(theme.spacing.md)
                        .py(theme.spacing.sm)
                        .rounded(theme.radius.sm)
                        .cursor_pointer()
                        .when(is_cursor, |d| {
                            d.bg(if is_focused {
                                theme.colors.row_selected
                            } else {
                                theme.colors.control_hover
                            })
                        })
                        .when(!is_cursor, |d| d.hover(move |s| s.bg(hover)))
                        .on_click(cx.listener(move |page, event: &ClickEvent, window, cx| {
                            page.cursor = Some(id);
                            page.focus_handle.focus(window);
                            if event.click_count() == 2 {
                                page.toggle(id, cx);
                            }
                            cx.notify();
                        }))
                        .child(dot(tone, &theme))
                        .child(item::summary(rule, &theme))
                        .child(status_column(status_line, detail, tone, &theme))
                        .child(
                            Button::new(
                                gpui::ElementId::Name(format!("toggle-{id}").into()),
                                label,
                            )
                            .when(!is_running && rule.is_enabled, |b| {
                                b.style(ButtonStyle::Primary)
                            })
                            .on_click(cx.listener(move |page, _, _, cx| page.toggle(id, cx))),
                        )
                        .into_any_element(),
                );
            }
        }

        let list = div()
            .id("port-authority")
            .key_context("PortAuthority")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_first))
            .on_action(cx.listener(Self::select_last))
            .on_action(cx.listener(Self::toggle_cursor))
            .on_action(cx.listener(Self::stop_all))
            .on_action(cx.listener(Self::edit_host))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .gap(theme.spacing.xs)
            .when(groups.is_empty(), |d| d.child(empty_state(&theme)))
            .children(rows);

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(theme.spacing.lg)
            .p(theme.spacing.xl)
            .child(toolbar)
            .child(list)
    }
}

/// A host's name over its rules, with its forwarding-only connection's state if one is open.
fn host_heading(host: &Host, tunnel: Option<(Tone, String)>, theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(theme.spacing.sm)
        .pt(theme.spacing.md)
        .px(theme.spacing.md)
        .text_size(theme.text.small_size)
        .text_color(theme.colors.text_muted)
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(host.title().to_uppercase()),
        )
        .when_some(tunnel, |d, (tone, text)| {
            d.child(dot(Some(tone), theme)).child(text)
        })
}

/// A status dot: green, amber or red, or an empty ring when nothing runs.
fn dot(tone: Option<Tone>, theme: &Theme) -> gpui::Div {
    let base = div()
        .flex_none()
        .size(theme.metrics.status_dot)
        .rounded_full();
    match tone {
        Some(Tone::Good) => base.bg(theme.colors.success),
        Some(Tone::Busy) => base.bg(theme.colors.warning),
        Some(Tone::Bad) => base.bg(theme.colors.danger),
        None => base
            .border(theme.metrics.border_width)
            .border_color(theme.colors.text_disabled),
    }
}

/// What a rule is doing, over its traffic or last error.
fn status_column(
    status_line: String,
    detail: Option<String>,
    tone: Option<Tone>,
    theme: &Theme,
) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(theme.metrics.detail_width)
        .overflow_hidden()
        .items_end()
        .text_size(theme.text.small_size)
        .child(
            div()
                .max_w_full()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_color(if tone == Some(Tone::Bad) {
                    theme.colors.danger
                } else {
                    theme.colors.text
                })
                .child(status_line),
        )
        .when_some(detail, |d, detail| {
            d.child(
                div()
                    .max_w_full()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_color(theme.colors.text_muted)
                    .child(detail),
            )
        })
}

fn empty_state(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(theme.spacing.sm)
        .p(theme.spacing.xl)
        .text_color(theme.colors.text_muted)
        .child(
            div()
                .text_color(theme.colors.text)
                .child("No port forwarding rules yet"),
        )
        .child(
            "Add rules in a host's editor, under Port forwarding: a local port to a database \
             behind the server, a port on the server back to this machine, or a SOCKS proxy.",
        )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use pretty_assertions::assert_eq;
    use ssherlock_core::forwarding::{Forward, Listen};
    use ssherlock_core::{Port, Timestamp};

    use super::*;
    use crate::forwarding::test_support;

    #[test]
    fn rules_are_grouped_by_host_title() {
        let now = Timestamp::UNIX_EPOCH;
        let host = |label: &str| {
            let mut h = Host::new("example.com".parse().unwrap(), now);
            label.clone_into(&mut h.label);
            h
        };
        let rule = |host: &Host, port: u16| {
            ForwardRule::new(
                host.id,
                Forward::Dynamic {
                    listen: Listen::localhost(Port::new(port).unwrap()),
                },
                now,
            )
        };
        let (web, db, idle) = (host("web"), host("DB"), host("idle"));
        let rules = vec![rule(&web, 1), rule(&db, 2), rule(&web, 3)];
        let hosts = vec![web.clone(), db.clone(), idle];
        let got: Vec<(String, Vec<u16>)> = grouped(&hosts, &rules)
            .into_iter()
            .map(|(h, rules)| {
                (
                    h.label.clone(),
                    rules
                        .iter()
                        .map(|r| r.forward.listen().port.get())
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![("DB".to_owned(), vec![2]), ("web".to_owned(), vec![1, 3])]
        );
    }

    #[gpui::test]
    fn a_rule_started_here_asks_for_the_host_key_then_forwards(cx: &mut gpui::TestAppContext) {
        use ssherlock_core::forwarding::Destination;
        use ssherlock_store::{Repository as _, Store};
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let (ssh_port, echo_port) = runtime.block_on(test_support::servers());
        let local_port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let dir =
            std::env::temp_dir().join(format!("ssherlock-port-authority-{}", Id::<()>::generate()));
        cx.update(|cx| {
            crate::keymap::init(cx);
            crate::notify::init(cx);
            cx.set_global(crate::runtime::TokioHandle::new(runtime.handle().clone()));
            cx.set_global(crate::ssh::SshEnvironment {
                known_hosts: dir.join("known_hosts"),
                read_only_known_hosts: Vec::new(),
                ssh_dir: None,
                local_user: Some("tester".into()),
            });
        });
        let forwards = cx.update(ForwardsModel::install);

        let mut host = Host::new("127.0.0.1".parse().unwrap(), Timestamp::UNIX_EPOCH);
        host.settings.port = Port::new(ssh_port);
        let rule = ForwardRule::new(
            host.id,
            Forward::Local {
                listen: Listen::localhost(Port::new(local_port).unwrap()),
                destination: Destination {
                    host: "127.0.0.1".parse().unwrap(),
                    port: Port::new(echo_port).unwrap(),
                },
            },
            Timestamp::UNIX_EPOCH,
        );
        let store = std::sync::Arc::new(Store::open_in_memory().unwrap());
        store.upsert(&host).unwrap();
        store.upsert(&rule).unwrap();
        let hosts = cx.update(|cx| HostsModel::install(store, cx));
        cx.run_until_parked();
        // The whole hosts page, which draws the page's dialogs over itself.
        let (list, cx) = cx.add_window_view(|window, cx| {
            let mut list = crate::hosts::HostList::new(hosts.clone(), window, cx);
            list.show(crate::hosts::nav::Section::Forwarding, cx);
            list
        });
        cx.update(|window, cx| list.focus_handle(cx).focus(window));
        cx.run_until_parked();
        let page = list
            .read_with(cx, |l, _| l.port_authority.clone())
            .expect("the Port forwarding page");
        assert_eq!(page.read_with(cx, |p, _| p.cursor()), Some(rule.id));

        // Enter starts the rule under the cursor; the new server's key is asked about first.
        cx.simulate_keystrokes("enter");
        test_support::wait_until(cx, "the host key dialog", |cx| {
            page.read_with(cx, |p, cx| p.prompts.read(cx).is_open())
        });
        cx.simulate_keystrokes("enter");
        test_support::wait_until(cx, "the forward to start", |cx| {
            forwards.read_with(cx, |f, _| {
                f.view(rule.id)
                    .is_some_and(|v| v.status == ssherlock_ssh::ForwardStatus::Active)
            })
        });
        assert!(forwards.read_with(cx, |f, _| f.tunnel(host.id).is_some()));

        let echoed = runtime.block_on(async move {
            let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", local_port))
                .await
                .unwrap();
            socket.write_all(b"through -L").await.unwrap();
            let mut answer = [0_u8; 10];
            tokio::time::timeout(Duration::from_secs(10), socket.read_exact(&mut answer))
                .await
                .unwrap()
                .unwrap();
            answer
        });
        assert_eq!(&echoed, b"through -L");
        test_support::wait_until(cx, "the counters", |cx| {
            forwards.read_with(cx, |f, _| {
                f.view(rule.id).is_some_and(|v| v.stats.uploaded == 10)
            })
        });

        // Enter again stops it, and the connection with nothing left to carry closes.
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        forwards.read_with(cx, |f, _| {
            assert!(f.view(rule.id).is_none());
            assert!(f.tunnel(host.id).is_none());
        });
        let _ = std::fs::remove_dir_all(&dir);
    }
}
