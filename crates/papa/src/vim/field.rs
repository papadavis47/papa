//! Vim on gpui-kit's text field: the one part of the Vim layer that knows
//! GPUI. It catches keystrokes before the field sees them, and plays the
//! machine's effects on the field.

use std::ops::Range;

use gpui_kit::component::input::{Redo, TextareaState, Undo};
use gpui_kit::{App, Bounds, Context, Entity, Keystroke, Pixels, Subscription, Window};

use super::key::Key;
use super::machine::{Effect, Mode, Vim};

/// What a keystroke is, as far as Vim cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// A key Vim reads.
    Key(Key),
    /// A shortcut with ⌘ (or Ctrl, on Linux): ⌘K, ⌘E, ⌘V. It goes on to the
    /// app and the field, as it would without Vim.
    Shortcut,
    /// Shift, Ctrl and the like, on their own.
    Modifier,
}

pub fn input(keystroke: &Keystroke) -> Input {
    let modifiers = &keystroke.modifiers;
    let key = keystroke.key.as_str();
    if keystroke.key_char.is_none()
        && matches!(key, "shift" | "control" | "alt" | "platform" | "function")
    {
        return Input::Modifier;
    }
    if modifiers.platform {
        return Input::Shortcut;
    }
    if modifiers.control {
        return match key {
            "r" => Input::Key(Key::Ctrl('r')),
            // Ctrl-[ is Esc to a terminal, and to Vim's fingers.
            "[" => Input::Key(Key::Escape),
            _ => Input::Shortcut,
        };
    }
    Input::Key(match key {
        "escape" => Key::Escape,
        "enter" => Key::Enter,
        "backspace" => Key::Backspace,
        "left" => Key::Left,
        "right" => Key::Right,
        "up" => Key::Up,
        "down" => Key::Down,
        _ => match keystroke.key_char.as_deref().map(single_char) {
            Some(Some(c)) => Key::Char(c),
            _ => Key::Other,
        },
    })
}

fn single_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

/// Vim, attached to an editor: the machine, and the hook that feeds it
/// keystrokes. Dropping it detaches Vim completely.
pub struct VimLayer {
    pub vim: Vim,
    /// Dropping it removes the hook.
    _keystrokes: Subscription,
}

impl VimLayer {
    /// Attaches Vim to the entity `cx` belongs to. `on_key` runs for every
    /// keystroke in the app, before key bindings and before any text field
    /// sees it, and returns whether it handled it. A handled keystroke
    /// stops there: no binding fires, no text is typed.
    pub fn attach<T: 'static>(
        cx: &mut Context<T>,
        on_key: impl Fn(&mut T, &Keystroke, &mut Window, &mut Context<T>) -> bool + 'static,
    ) -> Self {
        let this = cx.entity().downgrade();
        let keystrokes = cx.intercept_keystrokes(move |event, window, cx| {
            let handled = this
                .update(cx, |this, cx| on_key(this, &event.keystroke, window, cx))
                .unwrap_or(false);
            if handled {
                cx.stop_propagation();
            }
        });
        Self {
            vim: Vim::default(),
            _keystrokes: keystrokes,
        }
    }

    /// Reads one keystroke for `field`. Returns whether Vim handled it, and
    /// the effects the field can't carry out itself (save, leave) for the
    /// caller.
    pub fn keystroke(
        &mut self,
        keystroke: &Keystroke,
        field: &Entity<TextareaState>,
        window: &mut Window,
        cx: &mut App,
    ) -> (bool, Vec<Effect>) {
        let key = match input(keystroke) {
            Input::Key(key) => key,
            Input::Modifier | Input::Shortcut => return (false, Vec::new()),
        };
        let (text, selection) = {
            let field = field.read(cx);
            (field.value(), field.selected_range())
        };
        match self.vim.handle(key, &text, selection) {
            Some(effects) => (true, apply(effects, field, window, cx)),
            None => (false, Vec::new()),
        }
    }

    /// Puts the field's cursor where the mode wants it, after the text
    /// changed or the field was focused: onto a character in Normal mode.
    pub fn show_cursor(&self, field: &Entity<TextareaState>, cx: &mut App) {
        let (text, cursor) = {
            let field = field.read(cx);
            (field.value(), field.selected_range().start)
        };
        if let Some(range) = self.vim.selection(&text, cursor) {
            select(field, range, cx);
        }
    }

    /// Where to draw Normal mode's block cursor: over the character after
    /// the field's caret, or a narrow block where there's none (an empty
    /// line, the end of the text). Window coordinates, from the layout the
    /// field drew this frame; `None` outside Normal mode, while text is
    /// selected, or when the line is out of view.
    pub fn block(&self, field: &TextareaState) -> Option<Bounds<Pixels>> {
        if !matches!(self.vim.mode(), Mode::Normal | Mode::CommandLine(_)) {
            return None;
        }
        let selection = field.selected_range();
        if !selection.is_empty() {
            return None;
        }
        let cursor = selection.start;
        let width = field
            .text()
            .chars_at(cursor)
            .next()
            .filter(|&c| c != '\n')
            .map_or(0, char::len_utf8);
        let mut bounds = field.range_to_bounds(&(cursor..cursor + width))?;
        if width == 0 {
            bounds.size.width = bounds.size.height / 2.;
        }
        Some(bounds)
    }
}

/// Plays `effects` on `field`, and returns the ones it can't play.
fn apply(
    effects: Vec<Effect>,
    field: &Entity<TextareaState>,
    window: &mut Window,
    cx: &mut App,
) -> Vec<Effect> {
    let mut rest = Vec::new();
    for effect in effects {
        match effect {
            // `replace` records one step of undo history and emits a
            // change event, so the note autosaves as if typed.
            Effect::Edit { range, text } => field.update(cx, |field, cx| {
                field.set_selected_range(range, cx);
                field.replace(text, window, cx);
            }),
            Effect::Select(range) => select(field, range, cx),
            // The field's own undo history, through the same actions as
            // ⌘Z. The field is focused, so they reach it.
            Effect::Undo(times) => {
                (0..times).for_each(|_| window.dispatch_action(Box::new(Undo), cx))
            }
            Effect::Redo(times) => {
                (0..times).for_each(|_| window.dispatch_action(Box::new(Redo), cx))
            }
            effect @ (Effect::Save | Effect::Leave) => rest.push(effect),
        }
    }
    rest
}

fn select(field: &Entity<TextareaState>, range: Range<usize>, cx: &mut App) {
    field.update(cx, |field, cx| {
        if field.selected_range() != range {
            field.set_selected_range(range, cx);
        }
    });
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::input::{InputEvent, Textarea};
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Bounds, Focusable as _, IntoElement, ParentElement as _,
        Render, Styled as _, TestAppContext, WindowBounds, WindowOptions, div, point, px, size,
    };

    use super::*;

    /// A text field with Vim attached, the way the note editor does it.
    struct Host {
        field: Entity<TextareaState>,
        layer: VimLayer,
        requests: Vec<Effect>,
        _changes: Subscription,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(Textarea::new(&self.field).h_full())
        }
    }

    impl Host {
        fn on_key(
            &mut self,
            keystroke: &Keystroke,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> bool {
            if !self.field.focus_handle(cx).is_focused(window) {
                return false;
            }
            let (handled, requests) = self.layer.keystroke(keystroke, &self.field, window, cx);
            self.requests.extend(requests);
            handled
        }
    }

    /// A window with the field focused, holding `text`, in Normal mode.
    fn host(text: &str, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Host>) {
        cx.update(gpui_kit::init);
        let text = text.to_owned();
        let (window, host) = cx.update(|cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(600.), px(400.)),
                })),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    let field = cx.new(|cx| {
                        let mut field = TextareaState::new(window, cx);
                        field.set_value(text, window, cx);
                        field
                    });
                    field.update(cx, |field, cx| field.focus(window, cx));
                    let layer = VimLayer::attach(cx, Host::on_key);
                    layer.show_cursor(&field, cx);
                    // As the editor does: after undo, back onto a character.
                    let changes =
                        cx.subscribe(&field, |host: &mut Host, field, _: &InputEvent, cx| {
                            host.layer.show_cursor(&field, cx)
                        });
                    Host {
                        field,
                        layer,
                        requests: Vec::new(),
                        _changes: changes,
                    }
                })
            })
            .expect("open the test window")
        });
        (window, host)
    }

    fn type_keys(window: AnyWindowHandle, typed: &str, cx: &mut TestAppContext) {
        cx.update_window(window, |_, window, cx| window.input(typed, cx))
            .unwrap();
        cx.run_until_parked();
    }

    fn press(window: AnyWindowHandle, key: &str, cx: &mut TestAppContext) {
        cx.update_window(window, |_, window, cx| window.press(key, cx))
            .unwrap();
        cx.run_until_parked();
    }

    /// The field's text, and its selection.
    fn field(host: &Entity<Host>, cx: &mut TestAppContext) -> (String, Range<usize>) {
        cx.update(|cx| {
            let field = host.read(cx).field.read(cx);
            (field.value().to_string(), field.selected_range())
        })
    }

    #[gpui_kit::test]
    fn normal_mode_keys_are_commands_not_text(cx: &mut TestAppContext) {
        let (window, host) = host("one two", cx);
        assert_eq!(field(&host, cx), ("one two".into(), 0..0));

        type_keys(window, "x", cx);
        assert_eq!(field(&host, cx), ("ne two".into(), 0..0));
        type_keys(window, "dw", cx);
        assert_eq!(field(&host, cx), ("two".into(), 0..0));
    }

    #[gpui_kit::test]
    fn insert_mode_types_until_escape(cx: &mut TestAppContext) {
        let (window, host) = host("two", cx);

        type_keys(window, "ione ", cx);
        assert_eq!(
            field(&host, cx),
            ("one two".into(), 4..4),
            "typed by the field"
        );
        press(window, "escape", cx);
        assert_eq!(
            field(&host, cx),
            ("one two".into(), 3..3),
            "back on the space"
        );
    }

    #[gpui_kit::test]
    fn u_undoes_with_the_fields_history(cx: &mut TestAppContext) {
        let (window, host) = host("abc", cx);

        type_keys(window, "x", cx);
        assert_eq!(field(&host, cx).0, "bc");
        type_keys(window, "u", cx);
        assert_eq!(
            field(&host, cx),
            ("abc".into(), 0..0),
            "a caret, not the selection"
        );
        press(window, "ctrl-r", cx);
        assert_eq!(field(&host, cx).0, "bc");
    }

    #[gpui_kit::test]
    fn a_command_line_reaches_the_editor(cx: &mut TestAppContext) {
        let (window, host) = host("abc", cx);

        type_keys(window, ":wq", cx);
        press(window, "enter", cx);
        assert_eq!(field(&host, cx).0, "abc", "nothing typed");
        let requests = cx.update(|cx| host.read(cx).requests.clone());
        assert_eq!(requests, [Effect::Save, Effect::Leave]);
    }

    fn keystroke(source: &str, key_char: Option<&str>) -> Keystroke {
        let mut keystroke = Keystroke::parse(source).unwrap();
        keystroke.key_char = key_char.map(str::to_owned);
        keystroke
    }

    #[test]
    fn keystrokes_become_keys() {
        assert_eq!(
            input(&keystroke("x", Some("x"))),
            Input::Key(Key::Char('x'))
        );
        assert_eq!(
            input(&keystroke("shift-4", Some("$"))),
            Input::Key(Key::Char('$'))
        );
        assert_eq!(input(&keystroke("escape", None)), Input::Key(Key::Escape));
        assert_eq!(
            input(&keystroke("ctrl-r", None)),
            Input::Key(Key::Ctrl('r'))
        );
        assert_eq!(input(&keystroke("ctrl-[", None)), Input::Key(Key::Escape));
        assert_eq!(input(&keystroke("tab", None)), Input::Key(Key::Other));
    }

    #[test]
    fn shortcuts_and_modifiers_go_through() {
        assert_eq!(input(&keystroke("cmd-k", None)), Input::Shortcut);
        assert_eq!(input(&keystroke("ctrl-k", None)), Input::Shortcut);
        assert_eq!(input(&keystroke("shift", None)), Input::Modifier);
    }
}
