//! The keys Vim understands, independent of GPUI's `Keystroke`.

/// One key press, as Vim reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A typed character, Shift included: `A`, `$`, `:`.
    Char(char),
    /// A letter with Ctrl held: `Ctrl('r')`.
    Ctrl(char),
    Escape,
    Enter,
    Backspace,
    Left,
    Right,
    Up,
    Down,
    /// Any other key without ⌘ or Ctrl: Tab, Delete, F1.
    /// Normal mode ignores it rather than letting it type.
    Other,
}

impl Key {
    /// Appends the key as typed, for the status bar: `d`, `^R`.
    pub fn write(self, out: &mut String) {
        match self {
            Self::Char(c) => out.push(c),
            Self::Ctrl(c) => {
                out.push('^');
                out.push(c.to_ascii_uppercase());
            }
            _ => {}
        }
    }
}

/// Keys written the way Vim's documentation writes them: `d2w`, `<Esc>`,
/// `<C-r>`. For tests.
#[cfg(test)]
pub fn keys(typed: &str) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut rest = typed;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(end) = rest.find('>')
        {
            let key = match &rest[1..end] {
                "Esc" => Key::Escape,
                "CR" => Key::Enter,
                "BS" => Key::Backspace,
                "Left" => Key::Left,
                "Right" => Key::Right,
                "Up" => Key::Up,
                "Down" => Key::Down,
                ctrl if ctrl.starts_with("C-") => Key::Ctrl(ctrl[2..].chars().next().unwrap()),
                other => panic!("unknown key <{other}>"),
            };
            keys.push(key);
            rest = &rest[end + 1..];
        } else {
            keys.push(Key::Char(c));
            rest = &rest[c.len_utf8()..];
        }
    }
    keys
}
