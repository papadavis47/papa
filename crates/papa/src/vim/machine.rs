//! Vim's modes and what each key does in them: a state machine.
//!
//! [`Vim::handle`] takes a key and a snapshot of the text and returns
//! [`Effect`]s: edits, where the cursor goes, undo, save. It never touches
//! the text field itself, so every command is tested on a plain string; the
//! editor applies the effects to the real field.

use std::ops::Range;

use super::key::Key;
use super::motion::{self, Kind, Motion};
use super::parser::{Command, InsertAt, Operator, Parsed, Parser, Target};
use super::text::{CharClass, Text};

/// The mode decides what a key means. Visual mode carries its selection's
/// two ends and the command line carries what's typed after `:`, so that
/// state exists only while the mode does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Keys are commands: move, delete, put.
    #[default]
    Normal,
    /// Keys type text, until Esc.
    Insert,
    /// Motions grow a selection from `anchor` to `head` (byte offsets,
    /// both included); an operator then acts on it.
    Visual { anchor: usize, head: usize },
    /// A command after `:`, until Enter.
    CommandLine(String),
}

/// A change for the text field to make. Byte offsets, like the field's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Replace `range` of the text as it was before with `text`, as one
    /// step of undo history.
    Edit {
        range: Range<usize>,
        text: String,
    },
    /// Select `range` of the text after the edits. An empty range is a
    /// caret: in Normal mode, in front of the character the cursor is on.
    Select(Range<usize>),
    Undo(usize),
    Redo(usize),
    /// `:w`
    Save,
    /// `:q`: done with the note.
    Leave,
}

/// What the status bar shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// `NORMAL`, `INSERT` or `VISUAL`; `None` while typing a command line.
    pub mode: Option<&'static str>,
    /// The keys of an incomplete command (`2d`), the command line (`:wq`)
    /// or a message (`Saved`).
    pub detail: String,
}

/// What `d`, `c` and `y` keep, for `p` and `P` to put back.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Register {
    text: String,
    /// Whole lines, each ending in a newline. They're put on lines of
    /// their own, above or below the cursor's line.
    linewise: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Vim {
    mode: Mode,
    parser: Parser,
    register: Option<Register>,
    /// The column `j` and `k` aim for, kept across short lines.
    /// `usize::MAX` after `$`: the end of every line.
    column: Option<usize>,
    /// Shown in the status bar until the next key.
    message: Option<String>,
}

impl Vim {
    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    /// Back to Normal mode with nothing typed, as when a note opens. The
    /// register is kept: yank in one note, put in another.
    pub fn reset(&mut self) {
        self.mode = Mode::Normal;
        self.parser = Parser::default();
        self.column = None;
        self.message = None;
    }

    pub fn status(&self) -> Status {
        if let Some(message) = &self.message {
            return Status {
                mode: None,
                detail: message.clone(),
            };
        }
        let mode = match &self.mode {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual { .. } => "VISUAL",
            Mode::CommandLine(line) => {
                return Status {
                    mode: None,
                    detail: format!(":{line}"),
                };
            }
        };
        Status {
            mode: Some(mode),
            detail: self.parser.typed().to_owned(),
        }
    }

    /// Where the field's selection belongs in this mode, for a cursor at
    /// byte `cursor`: on a character in Normal mode, the whole selection in
    /// Visual mode. `None` in Insert mode, where the field's caret is right.
    pub fn selection(&self, text: &str, cursor: usize) -> Option<Range<usize>> {
        let text = Text::new(text);
        match self.mode {
            Mode::Normal | Mode::CommandLine(_) => Some(text.bytes(text.caret(text.ix(cursor)))),
            Mode::Visual { anchor, head } => Some(text.bytes(visual_range(&text, anchor, head))),
            Mode::Insert => None,
        }
    }

    /// Handles one key. `text` and `selection` are the field's as they are
    /// now. Returns `None` if the key isn't Vim's to handle, so it should
    /// reach the text field: anything but Esc in Insert mode.
    pub fn handle(&mut self, key: Key, text: &str, selection: Range<usize>) -> Option<Vec<Effect>> {
        self.message = None;
        let text = Text::new(text);
        let effects = match &mut self.mode {
            Mode::Insert if key == Key::Escape => {
                // Vim steps back onto the last character typed.
                self.mode = Mode::Normal;
                let cursor = text.ix(selection.start);
                let cursor = if cursor > text.line_start(cursor) {
                    cursor - 1
                } else {
                    cursor
                };
                vec![select(&text, text.caret(cursor))]
            }
            Mode::Insert => return None,
            Mode::CommandLine(line) => match key {
                Key::Char(c) => {
                    line.push(c);
                    Vec::new()
                }
                Key::Backspace => {
                    if line.pop().is_none() {
                        self.mode = Mode::Normal;
                    }
                    Vec::new()
                }
                Key::Enter => {
                    let line = std::mem::take(line);
                    self.mode = Mode::Normal;
                    self.ex(line.trim())
                }
                Key::Escape => {
                    self.mode = Mode::Normal;
                    Vec::new()
                }
                _ => Vec::new(),
            },
            Mode::Normal | Mode::Visual { .. } => {
                let cursor = match self.mode {
                    Mode::Visual { head, .. } => text.ix(head),
                    _ => text.ix(selection.start),
                };
                let cursor = text.clamp_normal(cursor);
                let visual = matches!(self.mode, Mode::Visual { .. });
                match self.parser.feed(key, visual) {
                    Parsed::Pending | Parsed::Invalid => Vec::new(),
                    Parsed::Done(command) => self.run(command, &text, cursor),
                }
            }
        };
        Some(effects)
    }

    /// A command typed after `:`.
    fn ex(&mut self, command: &str) -> Vec<Effect> {
        match command {
            "" => Vec::new(),
            "w" => {
                self.message = Some("Saved".to_owned());
                vec![Effect::Save]
            }
            // Papa saves as you type, so there's never anything to lose:
            // `:q` saves too, rather than refusing as Vim does.
            "q" | "wq" | "x" => vec![Effect::Save, Effect::Leave],
            other => {
                self.message = Some(format!("Not a command: {other}"));
                Vec::new()
            }
        }
    }

    fn run(&mut self, command: Command, text: &Text, cursor: usize) -> Vec<Effect> {
        // Only `j` and `k` remember a column; everything else forgets it.
        let column = self.column.take();
        match command {
            Command::Move { motion, count } => {
                let column = column.unwrap_or_else(|| text.column(cursor));
                let target = motion.target(text, cursor, count, column);
                self.column = match motion {
                    Motion::LineEnd => Some(usize::MAX),
                    motion if motion.is_vertical() => Some(column),
                    _ => None,
                };
                self.move_to(text, target)
            }
            Command::Operate {
                operator,
                target,
                count,
            } => self.operate(operator, target, count, text, cursor),
            Command::DeleteChar { count } => {
                let end = (cursor + count).min(text.line_end(cursor));
                self.delete(text, cursor..end, false)
            }
            Command::Insert(at) => self.insert(at, text, cursor),
            Command::Put { before, count } => self.put(before, count, text, cursor),
            Command::Undo { count } => vec![Effect::Undo(count)],
            Command::Redo { count } => vec![Effect::Redo(count)],
            Command::ToggleVisual => match self.mode {
                Mode::Visual { .. } => self.normal_at(text, cursor),
                _ => {
                    let at = text.byte(cursor);
                    self.mode = Mode::Visual {
                        anchor: at,
                        head: at,
                    };
                    self.move_to(text, cursor)
                }
            },
            Command::StartCommandLine => {
                self.mode = Mode::CommandLine(String::new());
                Vec::new()
            }
            Command::Escape => self.normal_at(text, cursor),
        }
    }

    /// Moves the cursor in Normal mode, or the selection's head in Visual.
    fn move_to(&mut self, text: &Text, target: usize) -> Vec<Effect> {
        let target = text.clamp_normal(target);
        match &mut self.mode {
            Mode::Visual { anchor, head } => {
                *head = text.byte(target);
                vec![select(text, visual_range(text, *anchor, *head))]
            }
            _ => vec![select(text, text.caret(target))],
        }
    }

    fn normal_at(&mut self, text: &Text, cursor: usize) -> Vec<Effect> {
        self.mode = Mode::Normal;
        vec![select(text, text.caret(cursor))]
    }

    // --- Operators ---

    fn operate(
        &mut self,
        operator: Operator,
        target: Target,
        count: Option<usize>,
        text: &Text,
        cursor: usize,
    ) -> Vec<Effect> {
        let (range, linewise, moved_to) = match target {
            Target::Selection => {
                let Mode::Visual { anchor, head } = self.mode else {
                    return Vec::new();
                };
                (visual_range(text, anchor, head), false, None)
            }
            Target::Lines => {
                let last = text.line(text.line_of(cursor) + count.unwrap_or(1) - 1);
                (text.line_start(cursor)..text.line_end(last), true, None)
            }
            Target::Motion(motion) => {
                let (range, linewise, to) = motion_range(operator, motion, count, text, cursor);
                (range, linewise, Some(to))
            }
        };
        let content = text.slice(range.clone());
        match (operator, linewise) {
            (Operator::Yank, _) => {
                self.yank(content, linewise);
                // The cursor goes to the start of what was yanked; for
                // whole lines, only if the motion went up (`yk`, `ygg`).
                let to = match (linewise, moved_to) {
                    (true, Some(to)) => to.min(cursor),
                    (true, None) => cursor,
                    (false, _) => range.start,
                };
                self.normal_at(text, to)
            }
            (Operator::Delete, false) => self.delete(text, range, false),
            (Operator::Delete, true) => self.delete(text, range, true),
            (Operator::Change, _) => {
                if !range.is_empty() {
                    self.yank(content, linewise);
                }
                self.edit_and_insert(text, range, "")
            }
        }
    }

    fn yank(&mut self, mut text: String, linewise: bool) {
        if linewise {
            text.push('\n');
        }
        self.register = Some(Register { text, linewise });
    }

    /// Deletes `range` into the register and returns to Normal mode. For
    /// whole lines, `range` is from the first line's start to the last
    /// line's end; the newline that separates them from the rest goes too.
    fn delete(&mut self, text: &Text, range: Range<usize>, linewise: bool) -> Vec<Effect> {
        self.mode = Mode::Normal;
        if range.is_empty() && !linewise {
            return vec![select(text, text.caret(range.start))];
        }
        self.yank(text.slice(range.clone()), linewise);
        let mut removed = range;
        if linewise {
            if removed.end < text.len() {
                removed.end += 1;
            } else if removed.start > 0 {
                // The last line has no newline of its own: take the one
                // before it, so no empty line is left behind.
                removed.start -= 1;
            }
        }
        let after = text.replace(removed.clone(), "");
        let cursor = removed.start.min(after.len());
        let cursor = if linewise {
            after.first_non_blank(cursor)
        } else {
            after.clamp_normal(cursor)
        };
        vec![edit(text, removed, ""), select(&after, after.caret(cursor))]
    }

    /// Replaces `range` with `with` and starts typing at its start.
    fn edit_and_insert(&mut self, text: &Text, range: Range<usize>, with: &str) -> Vec<Effect> {
        self.mode = Mode::Insert;
        let after = text.replace(range.clone(), with);
        let caret = after.byte(range.start);
        vec![edit(text, range, with), Effect::Select(caret..caret)]
    }

    fn insert(&mut self, at: InsertAt, text: &Text, cursor: usize) -> Vec<Effect> {
        let caret = match at {
            InsertAt::Cursor => cursor,
            InsertAt::AfterCursor => text.inclusive_end(cursor),
            InsertAt::LineStart => text.first_non_blank(cursor),
            InsertAt::LineEnd => text.line_end(cursor),
            InsertAt::LineBelow => {
                let end = text.line_end(cursor);
                let mut effects = self.edit_and_insert(text, end..end, "\n");
                let caret = text.byte(end) + 1;
                effects[1] = Effect::Select(caret..caret);
                return effects;
            }
            InsertAt::LineAbove => {
                let start = text.line_start(cursor);
                return self.edit_and_insert(text, start..start, "\n");
            }
        };
        self.mode = Mode::Insert;
        let caret = text.byte(caret);
        vec![Effect::Select(caret..caret)]
    }

    /// `p` and `P`. Lines go on lines of their own; anything else goes
    /// right after (or before) the cursor.
    fn put(&mut self, before: bool, count: usize, text: &Text, cursor: usize) -> Vec<Effect> {
        let Some(register) = &self.register else {
            return Vec::new();
        };
        let mut content = register.text.repeat(count);
        let (at, caret) = if register.linewise {
            let at = if before {
                text.line_start(cursor)
            } else if text.line_end(cursor) < text.len() {
                text.line_end(cursor) + 1
            } else {
                // Below the last line, which has no newline to put after.
                content.pop();
                content.insert(0, '\n');
                text.len()
            };
            let first_line = if at == text.len() && !before {
                at + 1
            } else {
                at
            };
            (at, first_line)
        } else {
            let at = if before {
                cursor
            } else {
                text.inclusive_end(cursor)
            };
            // On the last character put, unless it spans lines.
            let length = content.chars().count();
            let caret = if content.contains('\n') || length == 0 {
                at
            } else {
                at + length - 1
            };
            (at, caret)
        };
        let after = text.replace(at..at, &content);
        let caret = if register.linewise {
            after.first_non_blank(caret)
        } else {
            caret
        };
        vec![
            edit(text, at..at, &content),
            select(&after, after.caret(caret)),
        ]
    }
}

/// The characters a Visual selection covers: both ends included, the
/// newline too if an end is on one.
fn visual_range(text: &Text, anchor: usize, head: usize) -> Range<usize> {
    let (anchor, head) = (text.ix(anchor), text.ix(head));
    let (start, end) = (anchor.min(head), anchor.max(head));
    start..(end + 1).min(text.len())
}

/// The characters an operator acts on for `motion`, whether that's whole
/// lines, and where the motion landed.
fn motion_range(
    operator: Operator,
    motion: Motion,
    count: Option<usize>,
    text: &Text,
    cursor: usize,
) -> (Range<usize>, bool, usize) {
    let on_word = text
        .char(cursor)
        .is_some_and(|c| CharClass::of(c) != CharClass::Blank);
    if operator == Operator::Change && motion == Motion::WordForward && on_word {
        // `cw` changes to the end of the word and keeps the blank after
        // it, as if it were `ce`.
        let first = motion::end_of_word(text, cursor);
        let more = count.unwrap_or(1) - 1;
        let to = match more {
            0 => first,
            more => Motion::WordEnd.target(text, first, Some(more), 0),
        };
        return (cursor..text.inclusive_end(to), false, to);
    }
    let to = motion.target(text, cursor, count, text.column(cursor));
    let (start, end) = (cursor.min(to), cursor.max(to));
    let range = match motion.kind() {
        Kind::Linewise => return (text.line_start(start)..text.line_end(end), true, to),
        Kind::Inclusive => start..text.inclusive_end(end),
        // `dw` on a line's last word stops at the end of the line, rather
        // than joining the next line on.
        Kind::Exclusive if motion == Motion::WordForward => {
            let line_end = text.line_end(start);
            start..if line_end > start {
                end.min(line_end)
            } else {
                end
            }
        }
        Kind::Exclusive => start..end,
    };
    (range, false, to)
}

fn edit(text: &Text, range: Range<usize>, with: &str) -> Effect {
    Effect::Edit {
        range: text.bytes(range),
        text: with.to_owned(),
    }
}

fn select(text: &Text, range: Range<usize>) -> Effect {
    Effect::Select(text.bytes(range))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vim::key::keys;

    /// A text field in miniature: plays the effects on a string and types
    /// the keys Vim lets through.
    struct Field {
        text: String,
        selection: Range<usize>,
        vim: Vim,
        /// Effects other than edits and selections, in order.
        requests: Vec<Effect>,
    }

    impl Field {
        /// `text` with the cursor written as `|`.
        fn new(text: &str) -> Self {
            let cursor = text.find('|').expect("a cursor");
            Self {
                text: text.replace('|', ""),
                selection: cursor..cursor,
                vim: Vim::default(),
                requests: Vec::new(),
            }
        }

        fn type_keys(&mut self, typed: &str) -> &mut Self {
            for key in keys(typed) {
                match self.vim.handle(key, &self.text, self.selection.clone()) {
                    Some(effects) => self.apply(effects),
                    None => {
                        let Key::Char(c) = key else {
                            panic!("{key:?} isn't typed in these tests")
                        };
                        let at = self.selection.start;
                        self.text
                            .replace_range(self.selection.clone(), &c.to_string());
                        let caret = at + c.len_utf8();
                        self.selection = caret..caret;
                    }
                }
            }
            self
        }

        fn apply(&mut self, effects: Vec<Effect>) {
            for effect in effects {
                match effect {
                    Effect::Edit { range, text } => self.text.replace_range(range, &text),
                    Effect::Select(range) => self.selection = range,
                    other => self.requests.push(other),
                }
            }
        }

        /// The text with the cursor, the selection's start, as `|`.
        fn show(&self) -> String {
            let at = self.selection.start;
            format!("{}|{}", &self.text[..at], &self.text[at..])
        }
    }

    /// Types `typed` into `text` and returns the result.
    fn after(text: &str, typed: &str) -> String {
        Field::new(text).type_keys(typed).show()
    }

    #[test]
    fn the_cursor_stays_on_a_character() {
        assert_eq!(after("ab|c", "l"), "ab|c");
        assert_eq!(after("|abc\nd", "$"), "ab|c\nd");
        assert_eq!(after("o|ne", "w"), "on|e");
        assert_eq!(after("abc|", "<Esc>"), "ab|c", "after a click past the end");
    }

    #[test]
    fn up_and_down_remember_the_column() {
        assert_eq!(after("abc|d\nx\nabcd", "jj"), "abcd\nx\nabc|d");
        assert_eq!(after("|ab\nabcd", "$j"), "ab\nabc|d", "$ sticks to the end");
        assert_eq!(
            after("abc|d\nx\nabcd", "jlj"),
            "abcd\nx\n|abcd",
            "l forgets it"
        );
    }

    #[test]
    fn x_deletes_characters_on_the_line() {
        assert_eq!(after("he|llo", "x"), "he|lo");
        assert_eq!(after("he|llo", "2x"), "he|o");
        assert_eq!(after("a|bc\nd", "5x"), "|a\nd", "not the newline");
        assert_eq!(after("ab\n|\ncd", "x"), "ab\n|\ncd", "an empty line");
        assert_eq!(after("é|é", "x"), "|é", "characters, not bytes");
    }

    #[test]
    fn delete_with_a_motion() {
        assert_eq!(after("|one two", "dw"), "|two");
        assert_eq!(
            after("one |two\nthree", "dw"),
            "one| \nthree",
            "dw stops at the line's end"
        );
        assert_eq!(after("|one two three", "2dw"), "|three");
        assert_eq!(after("|one two", "de"), "| two");
        assert_eq!(after("one t|wo", "db"), "one |wo");
        assert_eq!(after("on|e two\nx", "d$"), "o|n\nx");
        assert_eq!(after("one t|wo", "d0"), "|wo");
        assert_eq!(after("|a", "dh"), "|a", "a motion that goes nowhere");
    }

    #[test]
    fn delete_whole_lines() {
        assert_eq!(after("a\n|b\nc", "dd"), "a\n|c");
        assert_eq!(after("a\nb\n|c", "dd"), "a\n|b", "the last line");
        assert_eq!(after("|a\nb\nc", "2dd"), "|c");
        assert_eq!(after("|a\nb\nc", "dj"), "|c");
        assert_eq!(after("a\nb\n|c", "dk"), "|a");
        assert_eq!(after("|  only", "dd"), "|");
        assert_eq!(after("|a\n  b", "dd"), "  |b", "to the first non-blank");
    }

    #[test]
    fn change_deletes_then_types() {
        assert_eq!(
            after("|one two", "cwONE<Esc>"),
            "ON|E two",
            "cw keeps the blank"
        );
        assert_eq!(after("|one two three", "c2wX<Esc>"), "|X three");
        assert_eq!(after("o|ne two", "c$X<Esc>"), "o|X");
        assert_eq!(after("a\n|bc\nd", "ccX<Esc>"), "a\n|X\nd");
        let mut field = Field::new("|one");
        field.type_keys("cw");
        assert_eq!(field.vim.mode(), &Mode::Insert);
    }

    #[test]
    fn yank_and_put() {
        assert_eq!(after("|a\nb", "yyp"), "a\n|a\nb");
        assert_eq!(after("|a\nb", "yyP"), "|a\na\nb");
        assert_eq!(after("a\n|b", "yyp"), "a\nb\n|b", "below the last line");
        assert_eq!(after("|a\nb", "yy2p"), "a\n|a\na\nb");
        assert_eq!(after("|a\nb", "ddp"), "b\n|a");
        assert_eq!(after("|ab", "xp"), "b|a");
        assert_eq!(after("|one two", "ywP"), "one| one two");
        assert_eq!(
            after("one |two", "yb"),
            "|one two",
            "yank moves to the start"
        );
        assert_eq!(after("a\nb\n|c", "ykP"), "a\n|b\nc\nb\nc");
        assert_eq!(after("|ab", "p"), "|ab", "nothing yanked yet");
    }

    #[test]
    fn insert_commands() {
        assert_eq!(after("a|b", "iX<Esc>"), "a|Xb");
        assert_eq!(after("a|b", "aX<Esc>"), "ab|X");
        assert_eq!(after("  a|b", "IX<Esc>"), "  |Xab");
        assert_eq!(after("a|b\nc", "AX<Esc>"), "ab|X\nc");
        assert_eq!(after("a|b\nc", "oX<Esc>"), "ab\n|X\nc");
        assert_eq!(after("ab\n|c", "OX<Esc>"), "ab\n|X\nc");
        assert_eq!(
            after("|ab", "i<Esc>"),
            "|ab",
            "Esc at the line's start stays"
        );
        assert_eq!(after("|", "aX<Esc>"), "|X", "in an empty note");
    }

    #[test]
    fn insert_mode_lets_keys_through_except_escape() {
        let mut vim = Vim::default();
        vim.handle(Key::Char('i'), "ab", 0..0);
        assert_eq!(vim.handle(Key::Char('x'), "ab", 0..0), None);
        assert_eq!(vim.handle(Key::Enter, "ab", 0..0), None);
        assert!(vim.handle(Key::Escape, "ab", 0..0).is_some());
        assert_eq!(vim.mode(), &Mode::Normal);
    }

    #[test]
    fn visual_mode_selects_then_operates() {
        let mut field = Field::new("a|bc d");
        field.type_keys("vl");
        assert_eq!(field.selection, 1..3, "both ends included");
        assert_eq!(field.vim.mode(), &Mode::Visual { anchor: 1, head: 2 });
        field.type_keys("h<Esc>");
        assert_eq!(field.vim.mode(), &Mode::Normal);

        assert_eq!(after("|abc", "vld"), "|c");
        assert_eq!(after("a|b\ncd", "vjd"), "|a", "across lines");
        assert_eq!(after("ab|cd", "vhx"), "a|d");
        assert_eq!(after("|one two", "veyP"), "on|eone two");
        assert_eq!(after("|abc", "vlcX<Esc>"), "|Xc");
        assert_eq!(after("ab|c", "vhv"), "a|bc", "v again leaves at the head");
    }

    #[test]
    fn the_command_line() {
        let mut field = Field::new("|a");
        field.type_keys(":w");
        assert_eq!(field.vim.status().detail, ":w");
        field.type_keys("<CR>");
        assert_eq!(field.requests, [Effect::Save]);
        assert_eq!(field.vim.status().detail, "Saved");

        let mut field = Field::new("|a");
        field.type_keys(":q<CR>");
        assert_eq!(field.requests, [Effect::Save, Effect::Leave]);
        let mut field = Field::new("|a");
        field.type_keys(":wq<CR>");
        assert_eq!(field.requests, [Effect::Save, Effect::Leave]);

        let mut field = Field::new("|a");
        field.type_keys(":nope<CR>");
        assert!(field.requests.is_empty());
        assert_eq!(field.vim.status().detail, "Not a command: nope");
        field.type_keys("l");
        assert_eq!(
            field.vim.status().mode,
            Some("NORMAL"),
            "messages last a key"
        );

        let mut field = Field::new("|a");
        field.type_keys(":<BS>x");
        assert_eq!(field.show(), "|", "Backspace on an empty line leaves it");
    }

    #[test]
    fn undo_and_redo_are_the_fields() {
        let mut field = Field::new("|a");
        field.type_keys("u3u<C-r>");
        assert_eq!(
            field.requests,
            [Effect::Undo(1), Effect::Undo(3), Effect::Redo(1)]
        );
    }

    #[test]
    fn the_status_shows_the_mode_and_pending_keys() {
        let mut field = Field::new("|a");
        assert_eq!(field.vim.status().mode, Some("NORMAL"));
        field.type_keys("2d");
        assert_eq!(field.vim.status().detail, "2d");
        field.type_keys("<Esc>i");
        assert_eq!(field.vim.status().mode, Some("INSERT"));
        assert_eq!(field.vim.status().detail, "");
        field.type_keys("<Esc>v");
        assert_eq!(field.vim.status().mode, Some("VISUAL"));
    }
}
