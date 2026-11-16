//! A confirmation dialog over a view: a message, optional detail, and Confirm / Cancel.

use gpui::{
    AnyElement, App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, actions, div,
};

use super::button::{Button, ButtonStyle};
use crate::theme::ActiveTheme as _;

actions!(
    modal,
    [
        /// Accepts the dialog (Enter).
        Confirm,
        /// Dismisses the dialog (Escape).
        Cancel,
    ]
);

/// Builds the dialog's detail element each frame.
type DetailBuilder = Box<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// What the user chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalEvent {
    /// The confirm button or Enter.
    Confirmed,
    /// The cancel button or Escape. A click outside the dialog does nothing: cancelling can
    /// mean rejecting a host key or abandoning a login, which a stray click shouldn't do.
    Cancelled,
}

/// A focused dialog asking the user to confirm something. Its owner renders it over the
/// content it is about and drops it after the first [`ModalEvent`].
pub struct Modal {
    title: SharedString,
    message: SharedString,
    detail: Option<DetailBuilder>,
    confirm_label: SharedString,
    is_danger: bool,
    focus_handle: FocusHandle,
}

impl EventEmitter<ModalEvent> for Modal {}

impl Focusable for Modal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Modal {
    /// A dialog titled `title` saying `message`, whose confirm button reads `confirm_label`.
    /// Takes focus so Enter and Escape work straight away.
    pub fn new(
        title: impl Into<SharedString>,
        message: impl Into<SharedString>,
        confirm_label: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        Self {
            title: title.into(),
            message: message.into(),
            detail: None,
            confirm_label: confirm_label.into(),
            is_danger: false,
            focus_handle,
        }
    }

    /// Marks confirming as risky: the confirm button turns red and Enter no longer confirms,
    /// so it takes a deliberate click. Escape still cancels.
    #[must_use]
    pub fn danger(mut self) -> Self {
        self.is_danger = true;
        self
    }

    /// Shows `detail` below the message, e.g. a preview of what will happen.
    #[must_use]
    pub fn detail(
        mut self,
        detail: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.detail = Some(Box::new(detail));
        self
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if !self.is_danger {
            cx.emit(ModalEvent::Confirmed);
        }
    }

    #[allow(clippy::unused_self, reason = "gpui action listeners take `&mut self`")]
    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(ModalEvent::Cancelled);
    }
}

impl Render for Modal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let detail = self.detail.as_ref().map(|detail| detail(window, cx));
        // The focused root carries the key context. The backdrop only dims and swallows clicks,
        // so nothing behind the dialog reacts while it's open, and a click outside it is
        // ignored rather than cancelling.
        div()
            .key_context("Modal")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .id("modal-backdrop")
                    .absolute()
                    .inset_0()
                    .bg(theme.colors.modal_scrim)
                    .occlude(),
            )
            .child(
                div()
                    .id("modal")
                    // Without this the backdrop underneath still counts as hovered, so a click
                    // on the panel (a label, the padding, a text field) reached it too.
                    .occlude()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.lg)
                    .w(theme.metrics.modal_width)
                    .p(theme.spacing.xl)
                    .rounded(theme.radius.md)
                    .bg(theme.colors.surface)
                    .border(theme.metrics.border_width)
                    .border_color(theme.colors.border)
                    .text_color(theme.colors.text)
                    .text_size(theme.text.ui_size)
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .text_color(theme.colors.text_muted)
                            .child(self.message.clone()),
                    )
                    .children(detail)
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(theme.spacing.md)
                            .child(Button::new("cancel", "Cancel").on_click(cx.listener(
                                |modal, _, window, cx| modal.cancel(&Cancel, window, cx),
                            )))
                            .child(
                                Button::new("confirm", self.confirm_label.clone())
                                    .style(if self.is_danger {
                                        ButtonStyle::Danger
                                    } else {
                                        ButtonStyle::Primary
                                    })
                                    // A click is deliberate, so it confirms even a danger dialog.
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(ModalEvent::Confirmed);
                                    })),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{AppContext as _, TestAppContext};
    use pretty_assertions::assert_eq;

    use super::*;

    /// Opens a dialog and returns the events it emits after `keys` are pressed.
    fn events_after(keys: &str, cx: &mut TestAppContext) -> Vec<ModalEvent> {
        events_after_in(keys, false, cx)
    }

    fn events_after_in(keys: &str, is_danger: bool, cx: &mut TestAppContext) -> Vec<ModalEvent> {
        cx.update(crate::keymap::init);
        let (modal, cx) = cx.add_window_view(|window, cx| {
            let modal = Modal::new("Title", "Message", "OK", window, cx);
            if is_danger { modal.danger() } else { modal }
        });
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&seen);
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&modal, move |_, event: &ModalEvent, _| {
                sink.borrow_mut().push(*event);
            })
        });
        cx.run_until_parked();
        cx.simulate_keystrokes(keys);
        cx.run_until_parked();
        seen.take()
    }

    #[gpui::test]
    fn enter_confirms_once(cx: &mut TestAppContext) {
        assert_eq!(events_after("enter", cx), [ModalEvent::Confirmed]);
    }

    /// Regression: gpui turns the *release* of Enter on a focused element with `on_click` into
    /// a click. The focused backdrop used to have one, so choosing "Paste" with Enter in the
    /// context menu opened the paste dialog and the key's release cancelled it at once.
    #[gpui::test]
    fn a_stray_enter_release_does_nothing(cx: &mut TestAppContext) {
        cx.update(crate::keymap::init);
        let (modal, cx) =
            cx.add_window_view(|window, cx| Modal::new("Title", "Message", "OK", window, cx));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&seen);
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&modal, move |_, event: &ModalEvent, _| {
                sink.borrow_mut().push(*event);
            })
        });
        cx.run_until_parked();
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
        });
        cx.run_until_parked();
        assert_eq!(seen.take(), Vec::<ModalEvent>::new());
    }

    /// Fills the window and shows the dialog over it, as a view that owns one does. The dialog
    /// is positioned against its parent, so as a window's root it wouldn't cover the window.
    struct Host {
        modal: gpui::Entity<Modal>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().relative().child(self.modal.clone())
        }
    }

    /// Opens a dialog, clicks at the point `at` picks from the window size, and returns the
    /// events.
    fn events_after_click(
        at: impl FnOnce(gpui::Size<gpui::Pixels>) -> gpui::Point<gpui::Pixels>,
        cx: &mut TestAppContext,
    ) -> Vec<ModalEvent> {
        cx.update(crate::keymap::init);
        let (host, cx) = cx.add_window_view(|window, cx| Host {
            modal: cx.new(|cx| Modal::new("Title", "Message", "OK", window, cx)),
        });
        let modal = cx.update(|_, cx| host.read(cx).modal.clone());
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&seen);
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&modal, move |_, event: &ModalEvent, _| {
                sink.borrow_mut().push(*event);
            })
        });
        cx.run_until_parked();
        let position = at(cx.update(|window, _| window.viewport_size()));
        cx.simulate_mouse_move(position, None, gpui::Modifiers::none());
        cx.simulate_click(position, gpui::Modifiers::none());
        cx.run_until_parked();
        seen.take()
    }

    /// Regression: the panel didn't block the mouse, so a click anywhere on it reached the
    /// backdrop and cancelled — the login dialog vanished when its field was clicked.
    #[gpui::test]
    fn a_click_on_the_panel_does_nothing(cx: &mut TestAppContext) {
        let centre =
            |size: gpui::Size<gpui::Pixels>| gpui::point(size.width / 2., size.height / 2.);
        assert_eq!(events_after_click(centre, cx), Vec::<ModalEvent>::new());
    }

    /// Regression: a click on the backdrop cancelled, so a stray click outside the login
    /// dialog abandoned the login, and outside the host key dialog rejected the key.
    #[gpui::test]
    fn a_click_outside_the_panel_does_nothing(cx: &mut TestAppContext) {
        let corner = |_| gpui::point(gpui::px(2.), gpui::px(2.));
        assert_eq!(events_after_click(corner, cx), Vec::<ModalEvent>::new());
    }

    #[gpui::test]
    fn escape_cancels(cx: &mut TestAppContext) {
        assert_eq!(events_after("escape", cx), [ModalEvent::Cancelled]);
    }

    #[gpui::test]
    fn enter_does_not_confirm_a_danger_dialog(cx: &mut TestAppContext) {
        assert_eq!(events_after_in("enter", true, cx), Vec::<ModalEvent>::new());
    }

    #[gpui::test]
    fn escape_cancels_a_danger_dialog(cx: &mut TestAppContext) {
        assert_eq!(events_after_in("escape", true, cx), [ModalEvent::Cancelled]);
    }
}
